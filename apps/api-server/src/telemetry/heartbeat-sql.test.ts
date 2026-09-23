/**
 * The `/heartbeat` INSERTs, run against a REAL SQLite with the real migrations. The route tests
 * mock D1 and can only prove which values were bound; only an engine can prove the `json_each`
 * unpacking puts each value in the right column. `node:sqlite` ships with Node, so this costs no dependency.
 */
/* eslint-disable-next-line no-restricted-imports -- Reads the real migration files, Node-side; what's under test is the SQL string imported from `heartbeat.ts`. */
import { readFileSync } from 'node:fs'
/* eslint-disable-next-line no-restricted-imports -- The engine lives on the Node side of the test and never reaches the Worker bundle. */
import { DatabaseSync } from 'node:sqlite'
import { describe, expect, it } from 'vitest'
import { insertEventsSql, insertHeartbeatSql } from './heartbeat'

function openDb(): DatabaseSync {
  const db = new DatabaseSync(':memory:')
  for (const file of ['0005_heartbeat.sql', '0020_heartbeat_uptime_and_events.sql']) {
    db.exec(readFileSync(new URL(`../../migrations/${file}`, import.meta.url), 'utf8'))
  }
  return db
}

const analId = 'anal_0123456789abcdef0123456789abcdef0123'

describe('the heartbeat INSERTs against real SQLite', () => {
  it('stores the beat with its uptime', () => {
    const db = openDb()
    db.prepare(insertHeartbeatSql).run(analId, '1.2.3', 'macOS 26.0', 'aarch64', 'release', '{"a":1}', 5_400)
    expect(db.prepare(`SELECT anal_id, uptime_seconds, config_json FROM heartbeat`).get()).toEqual({
      anal_id: analId,
      uptime_seconds: 5_400,
      config_json: '{"a":1}',
    })
    db.close()
  })

  it('unpacks every event from the one JSON parameter into its own row and columns', () => {
    const db = openDb()
    const rows = [
      ['search_used', '2026-09-24T10:00:00.000Z', '{"mode":"ai","count":3}'],
      ['app_launched', '2026-09-24T09:00:00.000Z', '{}'],
    ]
    db.prepare(insertEventsSql).run(analId, '1.2.3', JSON.stringify(rows))

    const stored = db
      .prepare(
        `SELECT anal_id, app_version, event, occurred_at, properties_json, received_at FROM analytics_event ORDER BY id`,
      )
      .all() as Record<string, unknown>[]
    expect(stored).toHaveLength(2)
    expect(stored[0]).toMatchObject({
      anal_id: analId,
      app_version: '1.2.3',
      event: 'search_used',
      occurred_at: '2026-09-24T10:00:00.000Z',
      properties_json: '{"mode":"ai","count":3}',
    })
    expect(stored[1]).toMatchObject({ event: 'app_launched', properties_json: '{}' })
    // Ours, not the client's: the retention sweep keys on it.
    expect(stored[0].received_at).toMatch(/^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}$/)
    db.close()
  })

  it('keeps a property string that looks like JSON as a string', () => {
    const db = openDb()
    db.prepare(insertEventsSql).run(
      analId,
      '1.2.3',
      JSON.stringify([['e', '2026-09-24T10:00:00.000Z', '{"label":"[1,2]"}']]),
    )
    const row = db.prepare(`SELECT properties_json FROM analytics_event`).get() as { properties_json: string }
    expect(JSON.parse(row.properties_json)).toEqual({ label: '[1,2]' })
    db.close()
  })
})
