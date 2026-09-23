# PostHog (Cmdr-specific)

Used for session replay, heatmaps, and click tracking on getcmdr.com, and for anonymous desktop-app feature events (beta
usage analytics).

- **Project**: https://eu.posthog.com/project/136072
- **Project settings**: https://eu.posthog.com/project/136072/settings/project-details
- **Host**: EU cloud (`https://eu.i.posthog.com`).
- **Project API key** (`phc_...`): public ingest token. Safe to include in client-side code (PostHog designed it this
  way). The website bakes it via `PUBLIC_POSTHOG_KEY`.

## Desktop feature events

The desktop app records anonymous, PII-free product events (the beta usage analytics) and never calls PostHog itself:
events wait in an on-disk spool and ride the next heartbeat to `api.getcmdr.com`, and the Worker forwards them here with
the `anal_` install id as `distinct_id`, the build and platform identity, `source: "desktop"` (so the dashboard can
split them from website events), and the allowlisted config-shape as `$set`. The desktop build carries no PostHog key.
Desktop side: `apps/desktop/src-tauri/src/analytics/CLAUDE.md`.
