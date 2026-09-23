/**
 * A real SFTP server through the UI: added, browsed, written to both ways, and
 * let go of. The scenarios live in `server-ops-suite.ts`, shared with the WebDAV
 * spec; this file only picks the protocol, so the two can't test different
 * things. Needs the SFTP fixture's `e2e` mode, which both Playwright lanes lease
 * (`apps/desktop/test/sftp-servers/README.md`).
 */

import { defineServerOpsSuite } from './server-ops-suite.js'

defineServerOpsSuite('sftp')
