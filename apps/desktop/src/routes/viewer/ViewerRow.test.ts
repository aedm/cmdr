/**
 * What one rendered ROW shows, and what it must never let reach the clipboard.
 *
 * The two rules under test are the ones a future edit is most likely to undo: the gutter
 * numbers LINES while the coordinate counts ROWS, and the continuation marker is
 * decoration in `::after` rather than a character in the DOM.
 */

import { describe, it, expect, afterEach, beforeEach } from 'vitest'
import { mount, tick, unmount } from 'svelte'

import ViewerRow from './ViewerRow.svelte'
import { _setLocaleForTests } from '$lib/intl/locale'
import type { LineSegment } from './line-segments'

beforeEach(() => {
  document.body.innerHTML = ''
  _setLocaleForTests('en-US')
})

afterEach(() => {
  _setLocaleForTests(null)
})

interface MountOpts {
  rowNumber?: number
  lineNumber?: number | null
  continues?: boolean
  text?: string
  gutterWidth?: number
  wordWrap?: boolean
}

function plain(text: string): LineSegment[] {
  return text === '' ? [] : [{ text, highlight: false, active: false, selected: false }]
}

function mountRow(opts: MountOpts = {}) {
  const target = document.createElement('div')
  document.body.appendChild(target)
  const instance = mount(ViewerRow, {
    target,
    props: {
      rowNumber: opts.rowNumber ?? 0,
      lineNumber: opts.lineNumber === undefined ? 0 : opts.lineNumber,
      continues: opts.continues ?? false,
      segments: plain(opts.text ?? 'hello'),
      gutterWidth: opts.gutterWidth ?? 3,
      wordWrap: opts.wordWrap ?? false,
    },
  })
  return { target, instance }
}

describe('ViewerRow gutter', () => {
  it('prints the 1-based PHYSICAL line number on the row that starts a line', async () => {
    const { target, instance } = mountRow({ rowNumber: 7, lineNumber: 2 })
    await tick()

    // Row 7, line 2: the gutter shows the line, so a wrapped file keeps counting lines
    // while the scroll coordinate counts rows.
    expect(target.querySelector('.line-number')?.textContent).toBe('3')
    await unmount(instance)
  })

  it('prints nothing on a continuation row, the usual editor convention', async () => {
    const { target, instance } = mountRow({ rowNumber: 8, lineNumber: null, continues: true })
    await tick()

    expect(target.querySelector('.line-number')?.textContent).toBe('')
    await unmount(instance)
  })

  it('exposes the row index as `data-row`, which the pointer layer reads back', async () => {
    const { target, instance } = mountRow({ rowNumber: 8, lineNumber: null })
    await tick()

    expect(target.querySelector('[data-row="8"]')).not.toBeNull()
    await unmount(instance)
  })
})

describe('ViewerRow continuation marker', () => {
  it('marks a row Cmdr broke, and leaves a row the FILE broke unmarked', async () => {
    const broken = mountRow({ continues: true })
    await tick()
    expect(broken.target.querySelector('.row-continues')).not.toBeNull()
    await unmount(broken.instance)

    const whole = mountRow({ continues: false })
    await tick()
    expect(whole.target.querySelector('.row-continues')).toBeNull()
    await unmount(whole.instance)
  })

  it('draws the marker identically with word wrap on and off', async () => {
    const off = mountRow({ continues: true, wordWrap: false })
    await tick()
    const offMark = off.target.querySelector('.row-continues')?.outerHTML
    await unmount(off.instance)

    const on = mountRow({ continues: true, wordWrap: true })
    await tick()
    const onMark = on.target.querySelector('.row-continues')?.outerHTML
    await unmount(on.instance)

    expect(offMark).toBe(onMark)
  })

  it('keeps the glyph out of the row text, so it can never reach the clipboard', async () => {
    const { target, instance } = mountRow({ text: 'abc', continues: true })
    await tick()

    // ❗ A Cmdr break is not a character in the file. `content` on a `::after` is
    // decoration: it is not selectable, and `textContent` (the only thing any copy path
    // could read off the DOM) sees the row's own text and nothing else.
    expect(target.querySelector('.line-text')?.textContent).toBe('abc')
    expect(target.querySelector('.row-continues')?.textContent).toBe('')
    await unmount(instance)
  })

  it('still says what it means to a screen reader, since the glyph itself is decoration', async () => {
    const { target, instance } = mountRow({ continues: true })
    await tick()

    expect(target.querySelector('.row-continues')?.getAttribute('aria-hidden')).toBe('true')
    expect(target.querySelector('.sr-only')?.textContent).toBe(
      'Cmdr split this line here to keep the viewer fast. The line continues on the next row.',
    )
    await unmount(instance)
  })

  it('never says a line break is here, which is the opposite of the truth', async () => {
    const { target, instance } = mountRow({ continues: true })
    await tick()

    // ❌ `⏎` means "there is a line break here". There isn't; that's the whole point.
    expect(target.innerHTML).not.toContain('⏎')
    await unmount(instance)
  })
})
