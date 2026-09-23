//! The heartbeat: the one request analytics makes, at most once per three hours.
//!
//! A loop wakes every [`WAKE_TICK`], adds the time since the last tick to the unreported uptime,
//! and sends `POST /heartbeat` when [`CADENCE`] says one is due. A beat carries the install's
//! identity, the PII-free config shape, `uptimeSeconds` (app runtime no earlier beat reported), and
//! up to [`MAX_EVENTS_PER_BEAT`] events from the front of the spool. Wire contract:
//! `docs/specs/network-chatter-plan.md`.
//!
//! What a beat accounts for leaves local state only when the server acknowledged it with a 2xx:
//! the spool drops exactly that batch, and the unreported uptime drops by exactly what was sent. The
//! schedule and the uptime live in `analytics-heartbeat.json`, so a relaunch neither resets the
//! three hours nor loses a short session's time.

use super::spool::{Spool, SpooledEvent};
use super::{SendPermission, config_shape};
use crate::send_schedule::{SendCadence, SendRecord, now_unix_ms};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Heartbeat ingestion endpoint. Debug builds hit the local Worker; release hits production.
#[cfg(debug_assertions)]
const HEARTBEAT_URL: &str = "http://localhost:8787/heartbeat";
#[cfg(not(debug_assertions))]
const HEARTBEAT_URL: &str = "https://api.getcmdr.com/heartbeat";

/// At most one acknowledged beat per three hours; a beat that didn't land is retried no sooner than
/// 15 minutes later.
pub(super) const CADENCE: SendCadence = SendCadence {
    interval: Duration::from_secs(3 * 60 * 60),
    retry_floor: Duration::from_secs(15 * 60),
};

/// How often the loop wakes to count uptime and ask whether a beat is due.
const WAKE_TICK: Duration = Duration::from_secs(5 * 60);

/// Network timeout for one beat. A beat can carry a couple of hundred KB of events.
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(20);

/// The server keeps at most this many events per beat, so the client never sends more.
pub(super) const MAX_EVENTS_PER_BEAT: usize = 500;

/// Serialized event bytes per beat. The server caps the body at 256 KB and the config at 16 KB, so
/// this leaves room for both plus the JSON around them.
const EVENT_BYTES_PER_BEAT: usize = 192 * 1024;

/// The loop's persisted state, in the app data dir.
const STATE_FILE_NAME: &str = "analytics-heartbeat.json";

/// The `/heartbeat` request body. camelCase on the wire, matching the Worker's validator.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HeartbeatPayload {
    /// `anal_` + a lowercase hyphenated v4 UUID. Matches `^anal_[0-9a-f-]{36}$`.
    anal_id: String,
    /// Semver `x.y.z` from `CARGO_PKG_VERSION`.
    app_version: String,
    /// Human-readable OS version, always non-empty.
    os_version: String,
    /// `aarch64` / `x86_64`.
    arch: String,
    /// `"release"` / `"debug"`.
    build_mode: Option<String>,
    /// The PII-free config-shape snapshot, stored verbatim by the server.
    config: serde_json::Value,
    /// App runtime this beat accounts for that no earlier acknowledged beat reported.
    uptime_seconds: u64,
    /// Spooled feature events, oldest first, at most [`MAX_EVENTS_PER_BEAT`].
    events: Vec<SpooledEvent>,
}

/// What `analytics-heartbeat.json` holds.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HeartbeatState {
    #[serde(flatten)]
    schedule: SendRecord,
    /// Whole seconds of runtime no acknowledged beat has reported yet.
    #[serde(default)]
    unreported_uptime_seconds: u64,
}

/// How a beat ended, as far as local state cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BeatOutcome {
    /// 2xx: the server stored the beat and its events.
    Acknowledged,
    /// The server refused these exact bytes (400, 413, 422), so sending them again can't work.
    Refused,
    /// Anything else: no answer, a timeout, a 5xx, a 429. Worth retrying unchanged.
    Failed,
}

impl BeatOutcome {
    fn from_status(status: u16) -> Self {
        match status {
            200..=299 => Self::Acknowledged,
            400 | 413 | 422 => Self::Refused,
            _ => Self::Failed,
        }
    }
}

/// Applies a beat's outcome to the persisted state. Returns whether the spool should drop the batch
/// the beat carried.
///
/// A refused batch is dropped rather than retried: the same bytes would be refused every 15 minutes
/// forever, and the heartbeat under them (the daily-active signal) with them. Its uptime is kept,
/// since uptime alone can't be what the server objected to.
fn settle(state: &mut HeartbeatState, outcome: BeatOutcome, sent_uptime: u64, now_ms: i64) -> bool {
    match outcome {
        BeatOutcome::Acknowledged => {
            state.schedule.record_success(now_ms);
            state.unreported_uptime_seconds = state.unreported_uptime_seconds.saturating_sub(sent_uptime);
            true
        }
        BeatOutcome::Refused => {
            state.schedule.record_failure(now_ms);
            true
        }
        BeatOutcome::Failed => {
            state.schedule.record_failure(now_ms);
            false
        }
    }
}

/// Adds `elapsed` to the unreported uptime in whole seconds, keeping the sub-second remainder in
/// `carry` so five-minute ticks don't lose a second each.
fn add_uptime(state: &mut HeartbeatState, carry: &mut Duration, elapsed: Duration) {
    let total = *carry + elapsed;
    state.unreported_uptime_seconds = state.unreported_uptime_seconds.saturating_add(total.as_secs());
    *carry = Duration::from_nanos(u64::from(total.subsec_nanos()));
}

/// Starts the loop. Call once from setup, after `analytics::init`.
pub(super) fn start() {
    let Some(data_dir) = super::data_dir() else {
        log::warn!(target: "analytics", "Heartbeat not started: no app data dir");
        return;
    };
    tauri::async_runtime::spawn(run(data_dir.join(STATE_FILE_NAME)));
}

async fn run(state_path: PathBuf) {
    let mut state = load_state(&state_path);
    let mut last_tick = Instant::now();
    let mut carry = Duration::ZERO;
    let mut logged_suppression = false;
    loop {
        // `Instant` doesn't advance while the machine sleeps (macOS and Linux both), so a closed
        // lid isn't counted as runtime.
        let elapsed = last_tick.elapsed();
        last_tick = Instant::now();

        match super::send_permission() {
            SendPermission::Suppressed(reason) => {
                if !logged_suppression {
                    log::debug!(target: "analytics", "Heartbeat suppressed ({reason}, no force override)");
                    logged_suppression = true;
                }
            }
            SendPermission::OptedOut => {
                // Fully silent, and nothing collected while opted in is sent later either.
                if state.unreported_uptime_seconds > 0 {
                    state.unreported_uptime_seconds = 0;
                    save_state(&state_path, &state);
                }
                carry = Duration::ZERO;
                if let Some(spool) = super::spool() {
                    spool.clear();
                }
            }
            SendPermission::Granted => {
                add_uptime(&mut state, &mut carry, elapsed);
                if state.schedule.due_in(now_unix_ms(), CADENCE).is_zero()
                    && let Some(spool) = super::spool()
                {
                    beat(&mut state, spool).await;
                }
                save_state(&state_path, &state);
            }
        }

        tokio::time::sleep(WAKE_TICK).await;
    }
}

async fn beat(state: &mut HeartbeatState, spool: &Spool) {
    let mut batch = spool.take_batch(MAX_EVENTS_PER_BEAT, EVENT_BYTES_PER_BEAT);
    let sent_uptime = state.unreported_uptime_seconds;
    let events = crate::pluralize::pluralize(batch.events.len() as u64, "event");
    let payload = build_payload(sent_uptime, std::mem::take(&mut batch.events));

    let outcome = send_payload(&payload).await;
    if settle(state, outcome, sent_uptime, now_unix_ms()) {
        spool.acknowledge(&batch);
    }
    match outcome {
        BeatOutcome::Acknowledged => {
            log::debug!(target: "analytics", "Heartbeat sent ({sent_uptime} s uptime, {events})");
        }
        BeatOutcome::Refused => {
            log::warn!(
                target: "analytics",
                "Heartbeat refused; dropped the {events} it carried. Check the Worker's heartbeat validator"
            );
        }
        BeatOutcome::Failed => {}
    }
}

fn build_payload(uptime_seconds: u64, events: Vec<SpooledEvent>) -> HeartbeatPayload {
    let fda_granted = !crate::fda_gate::is_fda_pending_runtime();
    let config = config_shape::build_config_shape(&super::read_raw_settings(), fda_granted);

    HeartbeatPayload {
        anal_id: crate::install_id::analytics_id(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        os_version: crate::platform::os_version(),
        arch: std::env::consts::ARCH.to_string(),
        build_mode: Some(current_build_mode().to_string()),
        config,
        uptime_seconds,
        events,
    }
}

fn current_build_mode() -> &'static str {
    if cfg!(debug_assertions) { "debug" } else { "release" }
}

async fn send_payload(payload: &HeartbeatPayload) -> BeatOutcome {
    let client = match reqwest::Client::builder().timeout(HEARTBEAT_TIMEOUT).build() {
        Ok(c) => c,
        Err(e) => {
            log::warn!(target: "analytics", "Couldn't build heartbeat HTTP client: {e}");
            return BeatOutcome::Failed;
        }
    };

    match client.post(HEARTBEAT_URL).json(payload).send().await {
        Ok(response) => {
            let outcome = BeatOutcome::from_status(response.status().as_u16());
            if outcome == BeatOutcome::Failed {
                log::debug!(target: "analytics", "Heartbeat server returned {}", response.status());
            }
            outcome
        }
        Err(e) => {
            // A beat that didn't land is fine: the next one after the retry floor carries it all.
            log::debug!(target: "analytics", "Heartbeat send failed: {e}");
            BeatOutcome::Failed
        }
    }
}

fn load_state(path: &Path) -> HeartbeatState {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|contents| serde_json::from_str(&contents).ok())
        .unwrap_or_default()
}

fn save_state(path: &Path, state: &HeartbeatState) {
    let Ok(content) = serde_json::to_string_pretty(state) else {
        return;
    };
    let tmp = path.with_extension("json.tmp");
    if let Err(e) = crate::config::durable_write_json(path, &tmp, &content) {
        log::debug!(target: "analytics", "Couldn't persist heartbeat state: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn payload(events: Vec<SpooledEvent>) -> HeartbeatPayload {
        HeartbeatPayload {
            anal_id: "anal_178c8e27-511f-4f0e-a1fc-6a44f2ab7341".to_string(),
            app_version: "1.2.3".to_string(),
            os_version: "macOS 26.0".to_string(),
            arch: "aarch64".to_string(),
            build_mode: Some("release".to_string()),
            config: json!({ "theme.mode": "dark", "fdaGranted": true }),
            uptime_seconds: 5400,
            events,
        }
    }

    fn event() -> SpooledEvent {
        SpooledEvent {
            event: "pane_navigated".to_string(),
            timestamp: "2026-09-24T10:00:00.123Z".to_string(),
            id: "0f8fad5b-d9cb-469f-a165-70867728950e".to_string(),
            app_version: "1.2.2".to_string(),
            properties: json!({ "volume_kind": "local" })
                .as_object()
                .cloned()
                .unwrap_or_default(),
        }
    }

    /// The field names are the wire contract with the Worker (`docs/specs/network-chatter-plan.md`
    /// § Wire contract). A rename here silently loses the field on the server.
    #[test]
    fn payload_field_names_match_the_wire_contract() {
        let value = serde_json::to_value(payload(vec![event()])).expect("serialize");
        let mut keys: Vec<&str> = value.as_object().expect("object").keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "analId",
                "appVersion",
                "arch",
                "buildMode",
                "config",
                "events",
                "osVersion",
                "uptimeSeconds"
            ]
        );
        let mut event_keys: Vec<&str> = value["events"][0]
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        event_keys.sort_unstable();
        assert_eq!(event_keys, ["appVersion", "event", "id", "properties", "timestamp"]);
    }

    #[test]
    fn payload_carries_values_verbatim() {
        let value = serde_json::to_value(payload(vec![event()])).expect("serialize");
        assert_eq!(value["analId"], json!("anal_178c8e27-511f-4f0e-a1fc-6a44f2ab7341"));
        assert_eq!(value["appVersion"], json!("1.2.3"));
        assert_eq!(value["osVersion"], json!("macOS 26.0"));
        assert_eq!(value["arch"], json!("aarch64"));
        assert_eq!(value["buildMode"], json!("release"));
        assert_eq!(value["config"]["theme.mode"], json!("dark"));
        assert_eq!(value["uptimeSeconds"], json!(5400));
        assert_eq!(value["events"][0]["event"], json!("pane_navigated"));
        assert_eq!(value["events"][0]["timestamp"], json!("2026-09-24T10:00:00.123Z"));
        assert_eq!(value["events"][0]["id"], json!("0f8fad5b-d9cb-469f-a165-70867728950e"));
        // The event keeps the version that produced it, whatever the beat's own version is.
        assert_eq!(value["events"][0]["appVersion"], json!("1.2.2"));
        assert_eq!(value["events"][0]["properties"], json!({ "volume_kind": "local" }));
    }

    #[test]
    fn a_beat_with_nothing_spooled_still_sends_an_empty_events_array() {
        let mut p = payload(vec![]);
        p.build_mode = None;
        let value = serde_json::to_value(p).expect("serialize");
        assert_eq!(value["events"], json!([]));
        assert_eq!(value["buildMode"], json!(null));
    }

    #[test]
    fn statuses_map_to_outcomes() {
        assert_eq!(BeatOutcome::from_status(200), BeatOutcome::Acknowledged);
        assert_eq!(BeatOutcome::from_status(204), BeatOutcome::Acknowledged);
        assert_eq!(BeatOutcome::from_status(400), BeatOutcome::Refused);
        assert_eq!(BeatOutcome::from_status(413), BeatOutcome::Refused);
        assert_eq!(BeatOutcome::from_status(422), BeatOutcome::Refused);
        assert_eq!(BeatOutcome::from_status(429), BeatOutcome::Failed);
        assert_eq!(BeatOutcome::from_status(500), BeatOutcome::Failed);
        assert_eq!(BeatOutcome::from_status(503), BeatOutcome::Failed);
    }

    fn state(unreported: u64) -> HeartbeatState {
        HeartbeatState {
            schedule: SendRecord::default(),
            unreported_uptime_seconds: unreported,
        }
    }

    /// Uptime counted while a beat was on the wire isn't what the beat reported, so only the sent
    /// amount comes off.
    #[test]
    fn an_acknowledged_beat_subtracts_only_what_it_sent() {
        let mut s = state(700);
        assert!(settle(&mut s, BeatOutcome::Acknowledged, 600, 1_000));
        assert_eq!(s.unreported_uptime_seconds, 100);
        assert_eq!(s.schedule.last_success_ms, Some(1_000));
    }

    #[test]
    fn a_failed_beat_keeps_everything_for_the_retry() {
        let mut s = state(600);
        assert!(!settle(&mut s, BeatOutcome::Failed, 600, 1_000));
        assert_eq!(s.unreported_uptime_seconds, 600);
        assert_eq!(s.schedule.last_failure_ms, Some(1_000));
        assert_eq!(s.schedule.last_success_ms, None);
    }

    #[test]
    fn a_refused_beat_drops_its_events_and_keeps_its_uptime() {
        let mut s = state(600);
        assert!(settle(&mut s, BeatOutcome::Refused, 600, 1_000));
        assert_eq!(s.unreported_uptime_seconds, 600);
        assert_eq!(s.schedule.last_failure_ms, Some(1_000));
    }

    #[test]
    fn uptime_accumulates_whole_seconds_and_carries_the_rest() {
        let mut s = state(0);
        let mut carry = Duration::ZERO;
        add_uptime(&mut s, &mut carry, Duration::from_millis(1_600));
        add_uptime(&mut s, &mut carry, Duration::from_millis(1_600));
        assert_eq!(s.unreported_uptime_seconds, 3);
        assert_eq!(carry, Duration::from_millis(200));
    }

    #[test]
    fn state_round_trips_and_a_missing_file_is_a_fresh_start() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(STATE_FILE_NAME);
        assert_eq!(load_state(&path), HeartbeatState::default());

        let mut s = state(42);
        s.schedule.record_success(7);
        save_state(&path, &s);
        assert_eq!(load_state(&path), s);
        let on_disk: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
        assert_eq!(
            on_disk,
            json!({ "lastSuccessMs": 7, "lastFailureMs": null, "unreportedUptimeSeconds": 42 })
        );
    }

    #[test]
    fn the_cadence_is_three_hours_with_a_fifteen_minute_retry_floor() {
        assert_eq!(CADENCE.interval, Duration::from_secs(10_800));
        assert_eq!(CADENCE.retry_floor, Duration::from_secs(900));
    }
}
