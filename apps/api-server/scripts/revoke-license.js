/**
 * Kill a hand-issued Cmdr license: a leaked key, a mistaken issue, an evaluation that turned into
 * something else.
 *
 * Run: node scripts/revoke-license.js --code CMDR-XXXX-XXXX-XXXX
 *
 * The app drops to Personal at its next revalidation, within seven days. The activation code stops
 * working immediately. Reinstating means minting a new license.
 *
 * ❌ Paddle licenses can't be revoked here. Cancel or refund those in Paddle: `/validate` resolves
 * a `txn_` id against Paddle and never looks at our ledger.
 */

import { defaultApiUrl, die, parseArgs, postAdmin, readAdminToken } from './admin-api.js'

const usage = `Revoke a hand-issued Cmdr license.

Usage:
  node scripts/revoke-license.js --code <CMDR-XXXX-XXXX-XXXX>
  node scripts/revoke-license.js --transaction-id <manual-XXXXXXXXXXXX>

Options:
  --api <url>   API base URL (default: ${defaultApiUrl})
  --help        This text
`

const args = parseArgs(process.argv.slice(2), {
  booleans: ['help'],
  known: ['code', 'transaction-id', 'api', 'help'],
})

if (args.help) {
  console.log(usage)
  process.exit(0)
}

if (!args.code && !args['transaction-id']) die(`Name the license to revoke.\n\n${usage}`)
if (args.code && args['transaction-id']) die(`Pass either --code or --transaction-id, not both.\n\n${usage}`)

const request = args.code ? { code: args.code } : { transactionId: args['transaction-id'] }
const apiUrl = args.api ?? defaultApiUrl

const result = await postAdmin('/admin/revoke', request, { apiUrl, token: readAdminToken() })

if (result.status === 'already_revoked') {
  console.log(
    `\nAlready revoked${result.revokedAt ? ` on ${result.revokedAt.slice(0, 10)}` : ''}: ${result.transactionId}\n`,
  )
  process.exit(0)
}

console.log(`\nRevoked ${result.transactionId}\n`)
console.log(`  Codes:       ${result.codes.join(', ')}`)
if (result.organizationName) console.log(`  Licensed to: ${result.organizationName}`)
if (result.note) console.log(`  Note:        ${result.note}`)
console.log(`\nThe code no longer activates. Any machine running on it falls back to Personal within seven days.\n`)
