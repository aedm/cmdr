/**
 * The `/admin/heartbeat-dau` query against a REAL SQLite with the real migrations, since what it has
 * to get right is arithmetic over NULLs: an old client's beat carries no uptime and stands for an
 * hour, a new one carries its own, and both land in one app-hours series.
 */
/* eslint-disable-next-line no-restricted-imports -- Reads the real migration files, Node-side; what's under test is the SQL string imported from `admin.ts`. */
import { readFileSync } from 'node:fs'
/* eslint-disable-next-line no-restricted-imports -- The engine lives on the Node side of the test and never reaches the Worker bundle. */
import { DatabaseSync } from 'node:sqlite'
import { describe, expect, it } from 'vitest'
import { heartbeatDauSql } from './admin'

function openDb(): DatabaseSync {
  const db = new DatabaseSync(':memory:')
  for (const file of ['0005_heartbeat.sql', '0020_heartbeat_uptime_and_events.sql']) {
    db.exec(readFileSync(new URL(`../../migrations/${file}`, import.meta.url), 'utf8'))
  }
  return db
}

describe('the engagement query against real SQLite', () => {
  it('reads an old hourly beat as one hour and a new beat as its uptime, in one series', () => {
    const db = openDb()
    const insert = db.prepare(
      `INSERT INTO heartbeat (anal_id, created_at, app_version, os_version, arch, uptime_seconds)
         VALUES (?, ?, '1.2.3', 'macOS 26.0', 'aarch64', ?)`,
    )
    // Two old-client beats (NULL uptime, an hour each) and one new beat covering 3 h, same day.
    insert.run('anal_a', '2026-09-24 08:00:00', null)
    insert.run('anal_a', '2026-09-24 09:00:00', null)
    insert.run('anal_b', '2026-09-24 12:00:00', 3 * 3_600)
    insert.run('anal_b', '2026-09-25 12:00:00', 1_800)

    const rows = db.prepare(heartbeatDauSql('')).all()
    expect(rows).toEqual([
      { date: '2026-09-24', dau: 2, appHours: 5 },
      { date: '2026-09-25', dau: 1, appHours: 0.5 },
    ])
    db.close()
  })
})
