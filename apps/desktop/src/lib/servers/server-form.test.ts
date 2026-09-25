/**
 * The add form's two translations: a typed address in, a dial target out.
 *
 * These are the rules a wrong reading turns into a password sent to the wrong
 * port, so they live here rather than inside the component.
 */

import { describe, expect, it } from 'vitest'
import type { ServerProtocol } from '$lib/ipc/bindings'
import { parseServerAddress } from './address-parser'
import {
  applyParsedAddress,
  emptyServerForm,
  formFromPrefill,
  formFromSftpServer,
  isStartFolderUnderRoot,
  nextcloudAddress,
  serverTargetFrom,
  smbAddressFrom,
} from './server-form'

/** A form as the sheet would hold it after someone picked `protocol` and typed `address`. */
function typed(address: string, protocol: ServerProtocol = 'smb') {
  const form = { ...emptyServerForm(), protocol, address }
  return applyParsedAddress(form, parseServerAddress(address))
}

describe('applyParsedAddress', () => {
  /**
   * ❗ The toggle is the person's, and typing never moves it. `user@host` once
   * flipped it to SFTP under someone typing their SMB NAS's address, and the
   * sheet dialed SSH without them clicking SFTP (cmdr-reports#8).
   */
  it('never moves the protocol toggle, whatever the address says', () => {
    expect(typed('sven@192.168.0.153')).toMatchObject({ protocol: 'smb' })
    expect(typed('sftp://ada@nas.local')).toMatchObject({ protocol: 'smb' })
    expect(typed('https://cloud.example.com', 'sftp')).toMatchObject({ protocol: 'sftp' })
    expect(typed('smb://naspolya', 'webdav')).toMatchObject({ protocol: 'webdav' })
  })

  it('picks up the account the address carried', () => {
    expect(typed('ada@nas.local:2222', 'sftp')).toMatchObject({ username: 'ada' })
    // Kept on SMB too, where the field is hidden, so switching the toggle later
    // shows what was typed.
    expect(typed('ada@nas.local:2222')).toMatchObject({ username: 'ada' })
  })

  it('fills the SFTP root folder from a path, and only when the address means SFTP too', () => {
    expect(typed('ada@nas.local:/srv/data', 'sftp')).toMatchObject({ remoteRoot: '/srv/data' })
    expect(typed('sftp://ada@nas.local/srv/data', 'sftp')).toMatchObject({ remoteRoot: '/srv/data' })
    // An SMB path is a share, and another protocol's path is not this one's folder.
    expect(typed('ada@nas.local/media')).toMatchObject({ remoteRoot: '' })
    expect(typed('smb://nas.local/media', 'sftp')).toMatchObject({ remoteRoot: '' })
  })

  it('leaves everything alone when the address says nothing yet', () => {
    // A half-typed address is the normal state of a field someone is typing
    // into, so it must not wipe what they already put in the other fields.
    const form = { ...emptyServerForm(), username: 'ada', protocol: 'sftp' as const }
    expect(applyParsedAddress(form, parseServerAddress('ada@'))).toEqual(form)
  })

  /**
   * ❗ A username the ADDRESS filled follows the address: `smb://x@nas/share` changed
   * to `nas.local:2222` kept "x" (QA round 2, m6). One the person typed stays.
   */
  it('drops a username the address filled once the address stops naming it', () => {
    const filled = typed('smb://x@nas/share')
    expect(filled.username).toBe('x')
    const next = applyParsedAddress({ ...filled, address: 'nas.local:2222' }, parseServerAddress('nas.local:2222'))
    expect(next.username).toBe('')
  })

  it('keeps a username the address did not carry', () => {
    const form = { ...emptyServerForm(), username: 'ada' }
    expect(applyParsedAddress(form, parseServerAddress('nas.local')).username).toBe('ada')
  })
})

describe('emptyServerForm', () => {
  it('starts Remember ON, because someone typing a password into a NEW server means to come back to it', () => {
    // ❗ Add mode is the one place the box proposes rather than reports: there is
    // no stored secret to report yet. Sign-in mode seeds from the store instead
    // (`open-sign-in.ts`), where a default-on box would seed one the user
    // already declined.
    expect(emptyServerForm().remember).toBe(true)
  })
})

describe('formFromPrefill', () => {
  it('opens on the protocol a pasted URL spells out, since that is what the person asked to open', () => {
    // Go to path hands over only addresses with a scheme, and opening a sheet on
    // `sftp://…` with SMB selected would make the person say it twice.
    expect(formFromPrefill('sftp://ada@nas.local/srv')).toMatchObject({
      protocol: 'sftp',
      address: 'sftp://ada@nas.local/srv',
      username: 'ada',
      remoteRoot: '/srv',
    })
    expect(formFromPrefill('https://cloud.example.com')).toMatchObject({ protocol: 'webdav' })
  })

  it('stays on the default for an address that names no protocol', () => {
    expect(formFromPrefill('ada@nas.local')).toMatchObject({ protocol: 'smb', username: 'ada' })
  })
})

describe('serverTargetFrom', () => {
  /** ❗ cmdr-reports#8: what gets dialed is the toggle's protocol, and the toggle is the person's. */
  it('never dials a protocol the person did not select', () => {
    // SMB is the default and has no target here (its connect is a share mount),
    // so an address that looks like SFTP still dials nothing over SSH.
    expect(serverTargetFrom(typed('sven@192.168.0.153'))).toBeNull()
    expect(serverTargetFrom(typed('sftp://ada@nas.local'))).toBeNull()
    expect(serverTargetFrom(typed('ssh ada@nas.local'))).toBeNull()
    expect(serverTargetFrom(typed('https://cloud.example.com', 'sftp'))).toMatchObject({ protocol: 'sftp' })
  })

  it('builds an SFTP target from the address and the advanced fields', () => {
    const form = {
      ...typed('ada@nas.local:2222/srv/data', 'sftp'),
      keyFile: ' ~/.ssh/id_ed25519 ',
      useAgent: false,
    }
    expect(serverTargetFrom(form)).toEqual({
      protocol: 'sftp',
      displayName: '',
      host: 'nas.local',
      port: 2222,
      username: 'ada',
      remoteRoot: '/srv/data',
      startFolder: null,
      keyFile: '~/.ssh/id_ed25519',
      useAgent: false,
      autoReconnect: true,
    })
  })

  it('builds a WebDAV target whose URL keeps the pasted path and drops a default port', () => {
    const form = { ...typed('https://cloud.example.com/remote.php/dav/files/ada/', 'webdav'), username: 'ada' }
    expect(serverTargetFrom(form)).toMatchObject({
      protocol: 'webdav',
      url: 'https://cloud.example.com/remote.php/dav/files/ada',
      username: 'ada',
      remoteRoot: '/',
    })
    expect(serverTargetFrom({ ...typed('http://nas:8080/dav', 'webdav'), username: 'ada' })).toMatchObject({
      url: 'http://nas:8080/dav',
    })
  })

  it('dials a port the address named with no scheme on whichever protocol is selected', () => {
    expect(serverTargetFrom(typed('ada@nas.local:2222', 'sftp'))).toMatchObject({ port: 2222 })
    expect(serverTargetFrom(typed('nas:5006/dav', 'webdav'))).toMatchObject({ url: 'https://nas:5006/dav' })
  })

  it('falls back to the protocol’s own port, and never carries another protocol’s over', () => {
    // ❗ A scheme's port belongs to that scheme: SMB's 445 in an SFTP dial opens a
    // socket nothing answers SSH on.
    expect(serverTargetFrom(typed('naspolya', 'sftp'))).toMatchObject({
      protocol: 'sftp',
      host: 'naspolya',
      port: 22,
    })
    expect(serverTargetFrom(typed('smb://naspolya:1445/media', 'sftp'))).toMatchObject({ port: 22 })
    expect(serverTargetFrom(typed('naspolya', 'webdav'))).toMatchObject({ url: 'https://naspolya' })
    expect(serverTargetFrom(typed('sftp://naspolya:2222/srv', 'webdav'))).toMatchObject({ url: 'https://naspolya' })
  })

  it('names no target for SMB or for an address that says nothing', () => {
    expect(serverTargetFrom(typed('naspolya'))).toBeNull()
    expect(serverTargetFrom(typed('not a server!!', 'sftp'))).toBeNull()
  })

  it('reads all three root spellings as the volume root', () => {
    for (const remoteRoot of ['', ' ', '.']) {
      expect(serverTargetFrom({ ...typed('ada@nas.local', 'sftp'), remoteRoot })).toMatchObject({
        remoteRoot: '/',
      })
    }
  })

  it('keeps an empty name empty, so the server is called by its account and host', () => {
    // ❗ Pre-fix an empty name fell back to the whole typed address, path and
    // all, which left the edit sheet with a name that looked exactly like the
    // address and sent a person to widen the root through the wrong field.
    expect(serverTargetFrom(typed('sftp://david@192.168.1.111:22/share/naspi/tmp', 'sftp'))).toMatchObject({
      displayName: '',
    })
    expect(serverTargetFrom({ ...typed('https://cloud.example.com/dav', 'webdav'), username: 'ada' })).toMatchObject({
      displayName: '',
    })
  })

  it('trims a typed name', () => {
    expect(serverTargetFrom({ ...typed('ada@nas.local', 'sftp'), displayName: '  Naspolya ' })).toMatchObject({
      displayName: 'Naspolya',
    })
  })

  it('carries a typed start folder trimmed, and none when the field is empty', () => {
    const form = typed('ada@nas.local/srv/data', 'sftp')
    expect(serverTargetFrom({ ...form, startFolder: ' /srv/data/photos ' })).toMatchObject({
      remoteRoot: '/srv/data',
      startFolder: '/srv/data/photos',
    })
    expect(serverTargetFrom({ ...form, startFolder: '  ' })).toMatchObject({ startFolder: null })
  })
})

/**
 * What SMB's add hands `connect_to_server`. The backend reads a bare host,
 * `host:port`, or an `smb://` URL, and refuses everything else.
 */
describe('smbAddressFrom', () => {
  it('spells an address with no scheme as an SMB URL, so `user@host` and a share path reach the backend', () => {
    // ❗ cmdr-reports#8's shape: the backend's bare-host reader refuses the `@`,
    // so `sven@192.168.0.153` has to travel as the SMB URL it means.
    expect(smbAddressFrom('sven@192.168.0.153')).toBe('smb://sven@192.168.0.153')
    expect(smbAddressFrom('  naspolya:1445/media ')).toBe('smb://naspolya:1445/media')
  })

  it('passes an SMB URL through as typed', () => {
    expect(smbAddressFrom('smb://Ada@NAS/media')).toBe('smb://Ada@NAS/media')
  })

  it('keeps only the host of an address that names another protocol', () => {
    // SMB is selected, so SMB is what gets dialed; another scheme's port and
    // path mean nothing to it.
    expect(smbAddressFrom('sftp://ada@nas.local:2222/srv')).toBe('smb://nas.local')
    expect(smbAddressFrom('ssh -p 2222 ada@nas.local')).toBe('smb://nas.local')
  })

  it('leaves an address it can’t read to the backend, which says what is wrong with it', () => {
    expect(smbAddressFrom('not a server!!')).toBe('not a server!!')
    expect(smbAddressFrom('[2001:db8::1]')).toBe('[2001:db8::1]')
  })
})

describe('formFromSftpServer', () => {
  it('opens an unnamed server with an empty name field, never its label or address', () => {
    const form = formFromSftpServer({
      host: 'nas.local',
      port: 22,
      username: 'ada',
      displayName: '',
      remoteRoot: '/srv/data',
      startFolder: '/srv/data/photos',
      keyFile: null,
      useAgent: true,
      autoReconnect: true,
      pinned: true,
      lastConnectedAt: '2026-09-06T00:00:00Z',
    })
    expect(form.displayName).toBe('')
    expect(form.remoteRoot).toBe('/srv/data')
    expect(form.startFolder).toBe('/srv/data/photos')
  })
})

/**
 * The sheet's inline mirror of the backend's "at or under the root" rule
 * (`saved_server_fields::start_folder_under_root`). The backend stays
 * authoritative; this only answers before a round-trip.
 */
describe('isStartFolderUnderRoot', () => {
  it('accepts an empty start folder, which means the root', () => {
    expect(isStartFolderUnderRoot('/srv/data', '')).toBe(true)
    expect(isStartFolderUnderRoot('/srv/data', '   ')).toBe(true)
  })

  it('accepts the root itself and anything below it', () => {
    expect(isStartFolderUnderRoot('/srv/data', '/srv/data')).toBe(true)
    expect(isStartFolderUnderRoot('/srv/data', '/srv/data/photos/2024')).toBe(true)
    expect(isStartFolderUnderRoot('/srv/data/', '/srv/data/photos/')).toBe(true)
  })

  it('refuses a sibling that shares the root as a string prefix', () => {
    expect(isStartFolderUnderRoot('/srv/data', '/srv/data-1')).toBe(false)
    expect(isStartFolderUnderRoot('/srv/data', '/srv/database/x')).toBe(false)
  })

  it('refuses a folder above or beside the root', () => {
    expect(isStartFolderUnderRoot('/srv/data', '/srv')).toBe(false)
    expect(isStartFolderUnderRoot('/srv/data', '/home/ada')).toBe(false)
  })

  it('resolves `.` and `..` the way the backend does before comparing', () => {
    expect(isStartFolderUnderRoot('/srv/data', '/srv/data/../etc')).toBe(false)
    expect(isStartFolderUnderRoot('/srv/data', '/srv/./data/photos')).toBe(true)
    expect(isStartFolderUnderRoot('/srv/data/tmp/..', '/srv/data/photos')).toBe(true)
  })

  it('reads a relative path from `/`, the way the backend reads a relative root', () => {
    expect(isStartFolderUnderRoot('/srv/data', 'photos')).toBe(false)
    expect(isStartFolderUnderRoot('/srv/data', 'srv/data/photos')).toBe(true)
  })

  it('treats every root spelling of the server root as holding everything', () => {
    for (const root of ['', '.', '/']) {
      expect(isStartFolderUnderRoot(root, '/home/ada')).toBe(true)
    }
  })
})

describe('nextcloudAddress', () => {
  it('appends the collection path nobody knows', () => {
    expect(nextcloudAddress('https://cloud.example.com/', 'ada')).toBe(
      'https://cloud.example.com/remote.php/dav/files/ada/',
    )
  })

  it('changes nothing without an account, because the path is per-account', () => {
    expect(nextcloudAddress('https://cloud.example.com', '  ')).toBe('https://cloud.example.com')
  })
})
