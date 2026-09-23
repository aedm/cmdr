-- The heartbeat stops being counted and starts carrying what it measures, and it brings the app's
-- feature events with it.
--
-- `uptime_seconds`: app runtime this beat accounts for that no earlier successful beat reported.
-- Engagement is `SUM(COALESCE(uptime_seconds, 3600)) / 3600` app hours per day, which makes the
-- metric independent of how often the app beats. NULL on every older row and on beats from clients
-- older than the field: those beat hourly, so each one stands for an hour, which is what the
-- COALESCE says.
ALTER TABLE heartbeat ADD COLUMN uptime_seconds INTEGER;

-- One row per feature event (`app_launched`, `search_used`, ...), relayed on the heartbeat that
-- carried it so the app never talks to a third party directly. Keyed by the same random `anal_<uuid>`
-- as `heartbeat`, and like it, no IP. `occurred_at` is the client's own clock (events can ride a beat
-- hours after they fired); `received_at` is ours, and is what retention sweeps on, since a client
-- clock can't be trusted to age a row out. `properties_json` holds the event's own PII-free
-- properties only: identity (version, OS, arch) is on the row, and the config snapshot is on the
-- heartbeat.
CREATE TABLE analytics_event (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    anal_id TEXT NOT NULL,
    event TEXT NOT NULL,
    occurred_at TEXT NOT NULL,       -- RFC 3339 UTC, normalized server-side (`YYYY-MM-DDTHH:MM:SS.sssZ`)
    received_at TEXT NOT NULL DEFAULT (datetime('now')),
    app_version TEXT NOT NULL,
    properties_json TEXT NOT NULL    -- a JSON object, `{}` when the event has none
);

CREATE INDEX idx_analytics_event_occurred ON analytics_event(occurred_at);
CREATE INDEX idx_analytics_event_event_occurred ON analytics_event(event, occurred_at);
CREATE INDEX idx_analytics_event_received ON analytics_event(received_at);
CREATE INDEX idx_analytics_event_anal ON analytics_event(anal_id);
