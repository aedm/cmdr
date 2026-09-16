# Licensing details

Pull-tier docs for `src/licensing/`. Must-know invariants live in `CLAUDE.md`; app-wide configuration, secrets, and
deploy runbooks live in `../../DETAILS.md`.

Read this before any non-trivial work here: editing, planning, reorganizing, or advising.

## Files

- **`licensing.ts`**: routes `/webhook/paddle`, `/activate`, `/validate`, and the mount for `manual-licenses.ts`.
- **`manual-licenses.ts`**: `/admin/generate` and `/admin/revoke`, the licenses we hand out rather than sell.
- **`license.ts`**: short-code and license-key generation, the `LicenseType` enum, `isPaddleTransactionId` /
  `generateManualTransactionId` (the id namespaces `/validate` dispatches on), and `generateShortId(prefix, len)` (also
  used for the `ERR-XXXXX` error-report ids).
- **`license-issuance.ts`**: the D1 ledger (`license_issuance`) behind both kinds of license: claim, take-over, code
  storage, and delivery marking for a Paddle fulfillment; row writing, lookup, and revocation for a manual one; plus the
  pure `classifyIssuance` and `classifyManualLicense`.
- **`paddle.ts`**: HMAC-SHA256 webhook verification and `constantTimeEqual` (the timing-safe compare every bearer-token
  check in the Worker uses).
- **`paddle-api.ts`**: Paddle REST client (transaction / subscription / customer fetch, `getLicenseTypeFromPriceId`).
- **`device-tracking.ts`**: device-set helpers — prune stale devices, alert threshold.
- Tests: `license.test.ts`, `paddle.test.ts`, `license-issuance.test.ts` (the two pure classifiers),
  `device-tracking.test.ts`, `webhook-paddle.test.ts` (first delivery, duplicate, retry after a failed email, concurrent
  delivery, Resend rejection), and two real-runtime suites that run the built Worker in workerd (`../../DETAILS.md` §
  Test runtimes): `production-runtime.test.ts` (minting, manual validation, revocation) and `webhook-runtime.test.ts`
  (the purchase path end to end, with Paddle and Resend stubbed at the socket).

## Data flow

```
Paddle webhook → HMAC verify (tries both live + sandbox secrets)
  → claim the transaction (D1 license_issuance, conditional INSERT; see Fulfillment below)
  → Paddle API: fetch customer details
  → per seat: generateLicenseKey() → generateShortCode() → KV.put(code, {fullKey, orgName})
  → store the codes on the row (short_codes, issued_at)
  → sendLicenseEmail() via Resend
  → mark the row delivered (emailed_at)

App activation: POST /activate → KV.get(shortCode) → return fullKey

Validation: POST /validate → dispatch on the transaction id's namespace
  `txn_...` → Paddle API transactions + subscriptions
    → HTTP 200 + ValidationResponse on success or invalid transaction (Paddle 404)
    → HTTP 502 + { error: "upstream_error" } if Paddle API unreachable or returns server error
    → if deviceId present: track device in KV (devices:{seatTransactionId}), log to Analytics Engine
    → if device count >= 6 and not recently alerted: send alert email to legal@getcmdr.com
  anything else → D1 license_issuance row where source = 'manual' (see Manual licenses below)
    → HTTP 200 + ValidationResponse, or HTTP 502 if the ledger read throws
```

## Key formats

- **Short code:** `CMDR-XXXX-XXXX-XXXX` using 31 unambiguous chars (excludes 0/O/1/I/L). Rejection sampling avoids
  modulo bias (max unbiased byte = `256 - (256 % 31)`).
- **License key:** `base64(JSON payload).base64(Ed25519 signature)`. Payload: email, transactionId, issuedAt, type,
  organizationName.
- **License types:** `commercial_subscription` | `commercial_perpetual`.
- **Short ids:** `generateShortId(prefix, len)` produces `ERR-A2345`-shaped ids from the same unambiguous alphabet
  (`23456789ABCDEFGHJKMNPQRSTUVWXYZ`), rejection-sampled. The error-report route consumes it (`../telemetry/`).

## Fulfillment (exactly-once issuance, at-least-once delivery)

A purchase must yield ONE set of license codes, but the email carrying them is safe to repeat. `license-issuance.ts`
keeps those apart, on the D1 table `license_issuance` (migration `0012`), one row per Paddle transaction:

1. **Claim**: `INSERT ... ON CONFLICT(transaction_id) DO NOTHING RETURNING transaction_id`, before any side effect. Two
   concurrent deliveries race on one primary key and SQLite hands the row to exactly one of them. This is the whole
   atomicity guarantee, and the reason the record lives in D1 rather than KV: KV has no conditional write, and its reads
   are eventually consistent (a redelivery within the propagation window would read a stale "not processed yet").
2. **Mint**: one signed license per seat, each stored in KV under its short code, then `short_codes` + `issued_at` on
   the row. Storing before sending is what makes a later redelivery reuse the SAME codes.
3. **Deliver**: `sendLicenseEmail`, then `emailed_at`. Only now is the purchase fulfilled.

A delivery that loses the claim reads the row and classifies it (pure `classifyIssuance`, unit-tested):

- `delivered` (`emailed_at` set) → 200 `already_processed`, forever.
- `in_flight` (claimed under `issuanceStaleAfterMs`, 5 min) → **503**, so Paddle redelivers instead of us running a
  second issuance beside the first. Live retries are 60 attempts over 3 days (20 in the first hour), so a transient 503
  costs minutes, and the buyer's licenses are never at stake.
- `resend` (stale claim, codes stored) → take over and re-send those codes. A duplicate email is the worst case.
- `remint` (stale claim, no codes: the delivery died before minting) → take over and mint. Any codes a dead attempt
  wrote to KV before failing are orphaned, which is harmless: nobody has seen them.

Take-over is `UPDATE ... WHERE claimed_at = <the value we read>`, so when two deliveries both find a stale claim, only
one wins and the other gets the 503.

**Rows never expire.** "This purchase was fulfilled" has no useful end date, and an expiring marker is exactly how a
late redelivery or a replayed webhook mints a second set of usable perpetual licenses. The table also doubles as the
support/audit trail (who got which codes, when).

**Decision, why not the Paddle `event_id` as the key:** one purchase must yield one set of licenses however many events
carry it, so the transaction id is the unit of fulfillment. `event_id` is stored on the row for debugging only.

**Gotcha: Resend reports failures in its response, it doesn't throw.** All four senders go through `sendViaResend`
(`../email/send.ts`), which turns an `error` into a thrown one. The app-wide rule is in `../../CLAUDE.md`; the money
consequence is here: an unchecked `await` marks the purchase delivered and stops Paddle retrying, so the buyer pays and
gets nothing.

**Known gap: no webhook timestamp tolerance.** `verifyPaddleWebhook` signs over `ts:body` but doesn't reject an old
`ts`, so a captured webhook stays replayable forever. The fulfillment row is what actually blocks the damage (a replay
finds `emailed_at` and does nothing). Paddle recommends a five-second window, but their docs don't say whether a retry
is re-signed with a fresh `ts` or replays the original signature, and rejecting legitimate retries would lose a
delivery, which is worse than the replay. So: log the observed `now - ts` on live deliveries first (including one forced
retry), then enable rejection with a tolerance the data supports.

## Manual licenses

Licenses we hand out rather than sell: an evaluation for a prospect on a work machine, free seats for partner companies,
thank-yous for testimonials and bug reports, customer-service recovery. They're issued constantly, so the process is two
scripts rather than a runbook of `curl` calls.

**The ledger row IS the license.** There's no Paddle transaction to resolve against, so `license_issuance` carries
everything `/validate` needs: `source = 'manual'`, `license_type`, `organization_name`, `expires_at` (NULL = perpetual),
`revoked_at`, and `note`. Migration `0017` added those columns; existing rows backfilled to `source = 'paddle'`.

**Dispatch is by id namespace, in `handleValidation`.** Paddle ids start with `txn_` in both environments, so anything
else is one of ours. A manual id is `manual-` plus 12 unambiguous characters (`generateManualTransactionId`), random
rather than time-based because the id travels inside the signed payload and is what `/validate` looks up: a guessable
one invites probing for other people's licenses. Random also keeps it clear of the `-\d+$` seat suffix the Paddle path
strips.

`classifyManualLicense` (pure, unit-tested) decides: `revoked_at` set → `invalid`, `expires_at` past → `expired`,
otherwise `active`. A missing row is `invalid`; a ledger read that THROWS is 502 `upstream_error`, the same answer a
Paddle outage gets, so a D1 blip can't drop a working license to Personal.

**Minting** (`/admin/generate`) writes the ledger row before the KV entry, then emails only if asked:

```bash
node apps/api-server/scripts/mint-license.js --email sven@mbition.io --org MBition \
  --note "Sven Kopetzki, MBition, evaluation" --expires 90d --send
```

`--expires` takes a date (`2027-01-31`, meaning through the end of that day) or a span (`30d`, `6m`, `1y`); omit it for
a perpetual license. `--dry-run` prints the request. Without `--send` the code is only printed, which is what you want
when you're pasting it into a reply yourself.

- **A note is required**, in the script and in the route. A free license nobody can explain later is worse than no
  record, and this table is the only place that explanation can live.
- **The type follows the expiry**, exactly as it does for a purchase: dated → `commercial_subscription`, undated →
  `commercial_perpetual`. An explicit `type` is accepted only when it agrees, so "perpetual until March" is
  unrepresentable.
- **A rejected email answers 502 with the code in the body.** The license exists and works; only delivery failed, so the
  script prints the code to hand over rather than inviting a second mint.
- **The email says what's true of THIS license.** `sendLicenseEmail` takes `expiresAt` and `issuedManually`, which pick
  between three validity sentences (perpetual, dated, renewing) and drop the thanks-for-your-purchase opener for a
  license nobody bought. ❌ Never let the subscription wording reach a dated license: a prospect forwards this mail to
  their IT department, and "will auto-renew" on an evaluation that simply stops is the sentence that ends a deal. Pinned
  by `../email/license.test.ts`.

**Revoking** (`/admin/revoke`) sets `revoked_at` and deletes the short codes from KV:

```bash
node apps/api-server/scripts/revoke-license.js --code CMDR-XXXX-XXXX-XXXX
```

The code stops activating immediately; machines already running on it fall back to Personal at their next revalidation
(within seven days). Reinstating means minting a new license, there's no un-revoke. It takes `--transaction-id` too, and
❌ refuses a `txn_` id rather than pretending: `/validate` resolves those against Paddle and never reads `revoked_at`,
so cancel or refund a real purchase in Paddle instead.

**Both scripts read `ADMIN_API_TOKEN`** from sops (`secret CMDR_ADMIN_API_TOKEN`) or the env var of the same name, never
from an argument, so the credential stays out of shell history.

**Gotcha: device tracking doesn't run on the manual path.** The fair-use alert resolves the customer through the Paddle
API, and a manual license has no Paddle customer. A shared hand-issued key is therefore invisible to the 6-device alert;
revocation is the lever.

## Webhook verification

`verifyPaddleWebhookMulti` tries both `PADDLE_WEBHOOK_SECRET_LIVE` and `PADDLE_WEBHOOK_SECRET_SANDBOX` when verifying
incoming webhooks. This is a safety net; in practice the sandbox dashboard sends webhooks only to the sandbox
destination (ngrok for local dev), and the live dashboard only to `api.getcmdr.com`. If a sandbox webhook somehow
reaches the production endpoint (or vice versa), it still verifies rather than silently failing. Costs one extra HMAC
check on mismatch.

**Price ID → license type mapping:** `getLicenseTypeFromPriceId()` (`paddle-api.ts`) maps Paddle price IDs (from
`PRICE_ID_*` env vars) to license types. Unknown price IDs fall back to `commercial_subscription` for backwards
compatibility.

**Validation error granularity:** `paddle-api.ts` throws `PaddleApiError` on network/5xx errors and returns `null` on
404 (transaction not found). That's what lets `/validate` answer 200-invalid versus 502-upstream, and the desktop app
keep a valid cached "active" through a transient Paddle outage instead of overwriting it with "invalid".

**Activation counter:** `/activate` increments a KV counter at `_meta:activation_count` on each successful activation,
read by `/admin/stats`. Read-then-write, so it races under concurrent activations (KV has no atomic increment); the
count is approximate by design. It starts from zero when deployed; initialize via the CF API if a historical count is
needed.

## Device tracking (fair use)

On each `/validate` call with a `deviceId`, the server tracks the device in KV (`devices:{seatTransactionId}`) and logs
to Analytics Engine (binding `DEVICE_COUNTS`, dataset `cmdr_device_counts`). Devices older than 90 days are pruned on
each write. If 6+ devices are active and no alert was sent in the past 30 days, an internal email goes to
`legal@getcmdr.com` via Resend. The KV value stores a `DeviceSet` of device hashes → last-seen timestamps plus an
optional `lastAlertedAt`. Tracking is per SEAT: each seat in a multi-seat purchase has its own transaction id and its
own 6-device allowance.

**Decision: no hard enforcement of device limits.** The server never rejects a validation because of device count.
Suspension is a manual decision after human review; the goal is detecting obvious key sharing (one key on 6+ devices),
not restricting legitimate power users. The threshold is 6 because 3-4 Macs is normal, 5 is plausible, 6 is hard to
explain as one person, and it isn't published in the ToS to avoid gaming.

## Decisions

**`PADDLE_ENVIRONMENT` controls sandbox versus live routing**, rather than inferring from transaction ids. Both
environments use the same `txn_` prefix, so an explicit env var is the only unambiguous signal. `wrangler.toml` defaults
to `"sandbox"` for local dev; the deployed worker overrides to `"live"` via a wrangler secret.

**Price IDs live in env vars (`PRICE_ID_*`)**, not code: sandbox and live Paddle accounts have different ids for the
same products. `.dev.vars` carries sandbox ids, wrangler secrets carry live ids.

**Paddle as Merchant of Record** (not Stripe, Gumroad, LemonSqueezy, or Polar): all-inclusive pricing (5% +
$0.50, no
hidden non-US or EU payout fees), aggregate monthly payouts (one invoice for the accountant instead of per-transaction),
global VAT/GST calculation and remittance, established reputation (Sketch, etc.). On a $29
sale: $1.95 fee → $27.05 net. At 30k sales that saves ~$7k/year versus LemonSqueezy. Stripe was rejected because a solo
dev handling VAT in 27+ EU countries is impractical (Stripe is a payment processor, not an MoR).

**Commercial prices use `external` tax mode**, so commercial customers pay tax on top of the listed price. Configured
per-price in the Paddle dashboard (both sandbox and live).

## Sandbox runbooks

**Expose the local worker via ngrok** (for Paddle sandbox webhooks):

```bash
ngrok http 8787 --url unsickerly-acclivitous-lala.ngrok-free.dev
```

The ngrok domain is stable across restarts, and the Paddle sandbox notification destination already points to
`https://unsickerly-acclivitous-lala.ngrok-free.dev/webhook/paddle`.

**Generate a test license key.** Point the mint script at the local worker:

```bash
node apps/api-server/scripts/mint-license.js --email test@example.com --org "Test Corp" \
  --note "local testing" --api http://localhost:8787
```

It returns a short code like `CMDR-ABCD-EFGH-1234`. Local `/validate` needs the local D1 to have the table, so run
`wrangler d1 migrations apply cmdr-telemetry --local` once. These keys validate for real (against the local ledger),
unlike a Paddle purchase; for an end-to-end run through Paddle itself, use the sandbox checkout flow described in
[testing Paddle checkout](../../README.md#testing-paddle-checkout).

Frontend counterpart: `apps/desktop/src/lib/licensing/CLAUDE.md`.
