//! The inbox's edge: mapping the pure rows onto `agent_inbox` and back.
//!
//! Everything else under `wake/` is values in and values out. This file is the one place that
//! takes a `Connection`, and it holds no policy of its own: it maps, it reads, it writes. The
//! decisions (when a row is due, what a restart drops) stay in `inbox.rs` where they can be
//! tested without a database.
//!
//! The store keeps its own flat row type and this maps onto it, rather than the store
//! importing the wake vocabulary — the same direction `proposals/` takes with `NewGroup`.

use rusqlite::Connection;

use super::{ChangeCounters, EventBundle, FolderImportance, Inbox, InboxRow, Interest, WakeReadiness};
use crate::agent::store::{
    AgentStoreError, StoredInboxRow, clear_inbox, load_inbox, load_inbox_row, replace_inbox, upsert_inbox_row,
};

/// Read the whole inbox back, for a launch to reconcile.
pub fn load(conn: &Connection) -> Result<Inbox, AgentStoreError> {
    Ok(Inbox::from_rows(load_inbox(conn)?.iter().map(to_row).collect()))
}

/// One bundle the store took in: the row as it now waits, and whether it merged into a row
/// that was already waiting.
pub struct Admitted {
    pub row: InboxRow,
    pub was_waiting: bool,
}

/// Admit one bundle straight into the table: read the row already waiting for its
/// folder-window, merge through the pure [`Inbox::admit_if_permitted`] (so every gate and
/// every merge rule is the in-memory inbox's own), and write the result back. `None` when
/// the gates refused it, in which case nothing was written.
///
/// This is what keeps the rows out of memory between wakes: see `InboxSummary`.
pub fn admit(
    conn: &Connection,
    readiness: WakeReadiness,
    bundle: EventBundle,
    importance: FolderImportance,
    hot_delay: std::time::Duration,
    now: u64,
) -> Result<Option<Admitted>, AgentStoreError> {
    let waiting =
        load_inbox_row(conn, &bundle.folder, clamp_to_i64(bundle.window_start))?.map(|stored| to_row(&stored));
    let was_waiting = waiting.is_some();
    let mut one = Inbox::from_rows(waiting.into_iter().collect());
    if !one.admit_if_permitted(readiness, bundle, importance, hot_delay, now) {
        return Ok(None);
    }
    let Some(row) = one.rows().first().cloned() else {
        return Ok(None);
    };
    upsert_inbox_row(conn, &to_stored(&row))?;
    Ok(Some(Admitted { row, was_waiting }))
}

/// Write the whole inbox, replacing what was there. What a restart writes back once it has
/// dropped the stale rows and deferred the overdue ones.
pub fn save_all(conn: &Connection, inbox: &Inbox) -> Result<(), AgentStoreError> {
    let rows: Vec<StoredInboxRow> = inbox.rows().iter().map(to_stored).collect();
    replace_inbox(conn, &rows)
}

/// Empty the table, which is what a wake does once it has drained the rows.
pub fn clear(conn: &Connection) -> Result<(), AgentStoreError> {
    clear_inbox(conn)
}

/// Times are unsigned seconds here and signed in SQLite, so each crossing saturates rather
/// than wrapping: a clock that produced something absurd must not turn a waiting row into one
/// that is overdue by an epoch.
fn to_stored(row: &InboxRow) -> StoredInboxRow {
    StoredInboxRow {
        folder: row.bundle.folder.clone(),
        window_start: clamp_to_i64(row.bundle.window_start),
        created: row.bundle.counters.created,
        modified: row.bundle.counters.modified,
        removed: row.bundle.counters.removed,
        renamed: row.bundle.counters.renamed,
        last_event_at: clamp_to_i64(row.bundle.last_event_at),
        interest: row.interest.value(),
        deliver_by: row.deliver_by.map(clamp_to_i64),
    }
}

fn to_row(stored: &StoredInboxRow) -> InboxRow {
    InboxRow {
        bundle: EventBundle {
            folder: stored.folder.clone(),
            counters: ChangeCounters {
                created: stored.created,
                modified: stored.modified,
                removed: stored.removed,
                renamed: stored.renamed,
            },
            window_start: clamp_to_u64(stored.window_start),
            last_event_at: clamp_to_u64(stored.last_event_at),
        },
        interest: Interest::of(stored.interest),
        deliver_by: stored.deliver_by.map(clamp_to_u64),
    }
}

fn clamp_to_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn clamp_to_u64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::store::{MIGRATIONS, run_migrations};
    use crate::agent::wake::{ChangeCounters, DEFAULT_HOT_DELAY, EventBundle, FolderImportance};

    fn migrated_conn() -> Connection {
        let conn = crate::sqlite_util::open_in_memory().expect("in-memory db");
        run_migrations(&conn, MIGRATIONS).expect("migrate");
        conn
    }

    fn bundle(folder: &str, created: u32, window_start: u64) -> EventBundle {
        EventBundle {
            folder: folder.to_string(),
            counters: ChangeCounters {
                created,
                ..ChangeCounters::default()
            },
            window_start,
            last_event_at: window_start + 5,
        }
    }

    /// An inbox that goes through the table comes back the same inbox: same rows, same
    /// counters, same deadlines. If any of that drifted, a restart would change what the agent
    /// is waiting for, silently.
    #[test]
    fn an_inbox_survives_a_round_trip_through_the_table() {
        let conn = migrated_conn();
        let mut inbox = Inbox::default();
        inbox.admit(
            bundle("/Users/someone/Downloads", 4, 100),
            FolderImportance::Scored(0.9),
            DEFAULT_HOT_DELAY,
            1_000,
        );
        inbox.admit(
            bundle("/tmp/log", 2, 100),
            FolderImportance::Unknown,
            DEFAULT_HOT_DELAY,
            1_000,
        );

        save_all(&conn, &inbox).expect("write");
        let loaded = load(&conn).expect("read");

        assert_eq!(loaded, inbox);
    }

    /// A cold row waits with NO deadline, and it has to come back that way. Reloaded with one, it
    /// would come due on its own after the next launch, which is exactly what the null prevents.
    #[test]
    fn a_cold_row_reloads_without_a_deadline() {
        let conn = migrated_conn();
        let mut inbox = Inbox::default();
        inbox.admit(
            bundle("/tmp/junk", 2, 100),
            FolderImportance::Floored,
            DEFAULT_HOT_DELAY,
            1_000,
        );
        save_all(&conn, &inbox).expect("write");

        let loaded = load(&conn).expect("read");

        assert_eq!(loaded, inbox);
        assert_eq!(loaded.next_deadline(), None, "and it still causes no wake of its own");
    }

    /// The merge key survives the round trip: two writes for one folder-window are one row,
    /// the same answer the in-memory inbox gives.
    #[test]
    fn two_writes_for_one_folder_window_load_as_one_row() {
        let conn = migrated_conn();
        let mut inbox = Inbox::default();
        inbox.admit(
            bundle("/Users/someone/Downloads", 4, 100),
            FolderImportance::Scored(0.9),
            DEFAULT_HOT_DELAY,
            1_000,
        );
        save_all(&conn, &inbox).expect("write");
        inbox.admit(
            bundle("/Users/someone/Downloads", 3, 100),
            FolderImportance::Scored(0.9),
            DEFAULT_HOT_DELAY,
            1_100,
        );
        save_all(&conn, &inbox).expect("write again");

        let loaded = load(&conn).expect("read");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded.rows()[0].bundle.counters.created, 7);
    }

    /// The wake loop keeps no rows in memory between wakes: each rollup merges into its
    /// folder-window's row IN THE TABLE, and only a small summary stays resident. That has to
    /// land exactly where admitting into one whole in-memory inbox would, merges, refusals,
    /// and deadlines included, or the backlog a wake finally reads would differ from the
    /// one the pure core promises.
    #[test]
    fn admitting_through_the_table_builds_the_inbox_admitting_in_memory_would() {
        use crate::agent::wake::InboxSummary;

        let conn = migrated_conn();
        let mut in_memory = Inbox::default();
        let mut summary = InboxSummary::default();
        let arrivals = [
            (
                bundle("/Users/someone/Downloads", 4, 100),
                FolderImportance::Scored(0.9),
                1_000,
            ),
            (bundle("/tmp/log", 2, 100), FolderImportance::Unknown, 1_000),
            (
                bundle("/Users/someone/Downloads", 3, 100),
                FolderImportance::Scored(0.9),
                1_100,
            ),
            (
                bundle("/Library/Caches/junk", 50, 100),
                FolderImportance::Floored,
                1_100,
            ),
            (
                bundle("/Users/someone/Downloads", 1, 40_000),
                FolderImportance::Scored(0.9),
                40_010,
            ),
            (bundle("/tmp/log", 9, 100), FolderImportance::Unknown, 1_200),
        ];
        for (arrival, importance, now) in arrivals {
            in_memory.admit_if_permitted(
                WakeReadiness::Ready,
                arrival.clone(),
                importance,
                DEFAULT_HOT_DELAY,
                now,
            );
            if let Some(admitted) =
                admit(&conn, WakeReadiness::Ready, arrival, importance, DEFAULT_HOT_DELAY, now).expect("admit")
            {
                summary.admitted(&admitted.row, admitted.was_waiting);
            }
        }

        let stored = load(&conn).expect("read");
        assert_eq!(
            sorted(stored.rows()),
            sorted(in_memory.rows()),
            "the table holds what the whole inbox would"
        );
        assert_eq!(
            summary,
            InboxSummary::of(&in_memory),
            "and the summary agrees with both"
        );
        assert_eq!(
            summary.len(),
            3,
            "three folder-windows wait; the floored one never got in"
        );
    }

    #[test]
    fn a_bundle_the_gates_refuse_never_reaches_the_table() {
        let conn = migrated_conn();
        let refused = admit(
            &conn,
            WakeReadiness::NeedsConsent,
            bundle("/Users/someone/Downloads", 4, 100),
            FolderImportance::Scored(0.9),
            DEFAULT_HOT_DELAY,
            1_000,
        )
        .expect("admit");
        assert!(refused.is_none());
        assert!(load(&conn).expect("read").is_empty());
    }

    fn sorted(rows: &[InboxRow]) -> Vec<InboxRow> {
        let mut rows = rows.to_vec();
        rows.sort_by(|a, b| (a.bundle.window_start, &a.bundle.folder).cmp(&(b.bundle.window_start, &b.bundle.folder)));
        rows
    }

    #[test]
    fn clearing_leaves_nothing_to_load() {
        let conn = migrated_conn();
        let mut inbox = Inbox::default();
        inbox.admit(
            bundle("/x", 1, 100),
            FolderImportance::Unknown,
            DEFAULT_HOT_DELAY,
            1_000,
        );
        save_all(&conn, &inbox).expect("write");

        clear(&conn).expect("clear");

        assert!(load(&conn).expect("read").is_empty());
    }
}
