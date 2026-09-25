/**
 * The example table behind add mode's address field.
 *
 * People paste what they have: an `ssh` line off a wiki, a `user@host` from a
 * colleague, a Nextcloud URL out of a browser bar, an `smb://` off a Finder
 * dialog. Every one of those has to land on an endpoint, and a shape nobody
 * recognizes has to say so rather than guess (a guess sends a password to the
 * wrong port).
 *
 * ❗ The protocol is the TOGGLE's, never the address's. The parser reports a
 * protocol only where the string spells one out (a scheme, an `ssh` line), and
 * the sheet uses that for a warning and nothing else.
 *
 * ❗ There is no property-testing library on the frontend, so this table IS the
 * contract. A shape that reaches the field and isn't here is a shape nobody
 * decided.
 */
import { describe, expect, it } from 'vitest'
import { addressLooksLike, parseServerAddress } from './address-parser'

describe('parseServerAddress: the SFTP shapes', () => {
  it('reads a full sftp URL', () => {
    expect(parseServerAddress('sftp://ada@nas.local:2222/srv/data')).toEqual({
      kind: 'parsed',
      protocol: 'sftp',
      host: 'nas.local',
      port: 2222,
      username: 'ada',
      path: '/srv/data',
    })
  })

  it('falls back to port 22 and no account', () => {
    expect(parseServerAddress('sftp://nas.local')).toEqual({
      kind: 'parsed',
      protocol: 'sftp',
      host: 'nas.local',
      port: 22,
      username: undefined,
      path: undefined,
    })
  })

  it('reads `ssh://` as the same thing', () => {
    const parsed = parseServerAddress('ssh://ada@nas.local:2222/srv')
    expect(parsed).toMatchObject({ protocol: 'sftp', port: 2222, username: 'ada', path: '/srv' })
  })

  it('reads a pasted `ssh` command line, with and without its port flag', () => {
    expect(parseServerAddress('ssh ada@nas.local')).toMatchObject({
      protocol: 'sftp',
      host: 'nas.local',
      port: 22,
      username: 'ada',
    })
    expect(parseServerAddress('ssh -p 2222 ada@nas.local')).toMatchObject({
      protocol: 'sftp',
      port: 2222,
      username: 'ada',
    })
    expect(parseServerAddress('ssh -p2222 ada@nas.local')).toMatchObject({ port: 2222 })
  })
})

describe('parseServerAddress: no scheme names no protocol', () => {
  /**
   * ❗ `user@host` with no scheme names an account and a machine, and NOTHING
   * about the protocol. It is SMB's natural spelling for a NAS share that needs
   * a user just as much as it is SFTP's. Reading it as SFTP once flipped the
   * toggle on a person typing `sven@192.168.0.153` for their SMB NAS and dialed
   * SSH on port 22 without them ever clicking SFTP (cmdr-reports#8). So it
   * answers no protocol and no port: the toggle, which the person set, decides.
   */
  it('reads a bare `user@host` as an account on a host, with no protocol and no port', () => {
    expect(parseServerAddress('sven@192.168.0.153')).toEqual({
      kind: 'parsed',
      host: '192.168.0.153',
      username: 'sven',
    })
    expect(parseServerAddress('ada@nas.local')).not.toHaveProperty('protocol')
  })

  it('reads a bare hostname with no protocol and no port', () => {
    // Which protocol a NAS name off a sticker speaks is the toggle's call.
    expect(parseServerAddress('naspolya')).toEqual({ kind: 'parsed', host: 'naspolya' })
    expect(parseServerAddress('192.168.1.111')).toEqual({ kind: 'parsed', host: '192.168.1.111' })
  })

  it('keeps a port and a path the address named, for whichever protocol the toggle says', () => {
    expect(parseServerAddress('naspolya.local:1445')).toEqual({
      kind: 'parsed',
      host: 'naspolya.local',
      port: 1445,
    })
    expect(parseServerAddress('ada@naspolya/media')).toEqual({
      kind: 'parsed',
      host: 'naspolya',
      username: 'ada',
      path: '/media',
    })
  })

  it('reads `user@host:port` as a port, and `user@host:/path` as scp syntax', () => {
    expect(parseServerAddress('ada@nas.local:2222')).toMatchObject({ port: 2222 })
    expect(parseServerAddress('ada@nas.local:2222')).not.toHaveProperty('path')
    const scp = parseServerAddress('ada@nas.local:/srv/data')
    expect(scp).toMatchObject({ path: '/srv/data' })
    expect(scp).not.toHaveProperty('port')
  })
})

describe('parseServerAddress: the WebDAV shapes', () => {
  it('reads a bare https origin as WebDAV on 443', () => {
    expect(parseServerAddress('https://nas:5006/')).toEqual({
      kind: 'parsed',
      protocol: 'webdav',
      host: 'nas',
      port: 5006,
      username: undefined,
      path: undefined,
      secure: true,
    })
    expect(parseServerAddress('https://cloud.example.com')).toMatchObject({
      protocol: 'webdav',
      port: 443,
      secure: true,
    })
  })

  it('keeps a Nextcloud URL whole, path and all', () => {
    // ❗ Nobody can tell where the base URL ends and the collection begins, and
    // the backend resolves the remote root relative to the base anyway. So the
    // whole path stays with the address and the remote folder starts at the root.
    expect(parseServerAddress('https://cloud.example.com/remote.php/dav/files/ada/')).toMatchObject({
      protocol: 'webdav',
      host: 'cloud.example.com',
      port: 443,
      path: '/remote.php/dav/files/ada',
    })
  })

  it('reads plain http, and says it is not secure', () => {
    expect(parseServerAddress('http://nas:8080/dav')).toMatchObject({
      protocol: 'webdav',
      port: 8080,
      secure: false,
      path: '/dav',
    })
  })

  it('reads the four WebDAV schemes, `davs` secure and `dav` not', () => {
    expect(parseServerAddress('webdav://ada@nas:5006/dav')).toMatchObject({
      protocol: 'webdav',
      username: 'ada',
      port: 5006,
      secure: true,
    })
    expect(parseServerAddress('davs://nas/dav')).toMatchObject({ port: 443, secure: true })
    expect(parseServerAddress('dav://nas/dav')).toMatchObject({ port: 80, secure: false })
  })
})

describe('parseServerAddress: the SMB shapes', () => {
  it('reads an `smb://` address, account and share and all', () => {
    expect(parseServerAddress('smb://naspolya')).toEqual({
      kind: 'parsed',
      protocol: 'smb',
      host: 'naspolya',
      port: 445,
      username: undefined,
      path: undefined,
    })
    expect(parseServerAddress('smb://ada@naspolya/media')).toMatchObject({
      protocol: 'smb',
      host: 'naspolya',
      username: 'ada',
      path: '/media',
    })
    expect(parseServerAddress('cifs://naspolya')).toMatchObject({ protocol: 'smb', port: 445 })
  })

  /** A UNC path off Windows or a colleague's email is SMB spelled with backslashes (QA round 2). */
  it('reads a UNC path as SMB, share and all', () => {
    expect(parseServerAddress('\\\\nas\\share\\docs')).toMatchObject({
      kind: 'parsed',
      protocol: 'smb',
      host: 'nas',
      port: 445,
      path: '/share/docs',
    })
    expect(parseServerAddress('\\\\nas')).toMatchObject({ protocol: 'smb', host: 'nas' })
  })
})

describe('parseServerAddress: folding and refusing', () => {
  it('folds the host to lowercase and leaves the account alone', () => {
    // The same fold `cmdr_fs::volume::ids` performs: DNS is case-insensitive, a
    // POSIX account is not, and `Ada` and `ada` can be two people.
    expect(parseServerAddress('SFTP://Ada@NAS.Local:22')).toMatchObject({
      protocol: 'sftp',
      host: 'nas.local',
      username: 'Ada',
    })
  })

  it('ignores whitespace around what was pasted', () => {
    expect(parseServerAddress('  sftp://nas.local  ')).toMatchObject({ host: 'nas.local' })
  })

  /**
   * ❗ A pasted URL may carry `user:password@`. The password must never reach the
   * Username field, the volume id, or the saved store: all three are plain text
   * a person reads, and the id is what the Keychain scope is keyed on.
   */
  it('takes the account out of a URL that carries a password, and drops the password', () => {
    expect(parseServerAddress('https://ada:hunter2@nas.local/dav')).toMatchObject({
      protocol: 'webdav',
      host: 'nas.local',
      username: 'ada',
    })
  })

  it('refuses a URL whose userinfo is a password with no account', () => {
    expect(parseServerAddress('sftp://:hunter2@nas.local')).toEqual({ kind: 'unparsed' })
  })

  it.each([
    ['', 'nothing typed yet'],
    ['   ', 'whitespace only'],
    ['not a server!!', 'punctuation no host can carry'],
    ['ftp://nas.local', 'a protocol Cmdr does not speak'],
    ['sftp://', 'a scheme with no host'],
    ['ada@', 'an account with no host'],
    ['sftp://nas.local:99999', 'a port outside the range'],
    ['sftp://nas.local:0', 'port zero'],
    ['sftp://[2001:db8::1]:2222', 'an IPv6 literal, which no remote path can spell yet'],
    ['/srv/data', 'a bare server-absolute path, which names no server at all'],
  ])('refuses %j (%s)', (input) => {
    expect(parseServerAddress(input)).toEqual({ kind: 'unparsed' })
  })
})

/**
 * The warning under the address field: which protocol the address looks like,
 * when that isn't the one selected. It never moves the toggle; the person reads
 * it and decides.
 *
 * ❗ Only two things count as evidence. A scheme (or an `ssh` line) is the
 * address saying outright which protocol it is. A well-known port on an address
 * with no scheme is strong enough to mention, since `nas:22` with SMB selected
 * is almost certainly a mix-up. Nothing else is: an account, a path, or a bare
 * host fits all three protocols.
 */
describe('addressLooksLike', () => {
  it.each([
    ['sftp://ada@nas.local', 'smb', 'sftp', 'an sftp URL'],
    ['ssh://nas.local', 'webdav', 'sftp', 'an ssh URL'],
    ['ssh -p 2222 ada@nas.local', 'smb', 'sftp', 'a pasted ssh line'],
    ['smb://sven@192.168.0.153/sven', 'sftp', 'smb', 'an smb URL'],
    ['cifs://nas', 'webdav', 'smb', 'a cifs URL'],
    ['https://cloud.example.com/remote.php/dav', 'smb', 'webdav', 'a web address'],
    ['davs://nas/dav', 'sftp', 'webdav', 'a davs URL'],
    ['nas.local:22', 'smb', 'sftp', "SSH's port on an address with no scheme"],
    ['ada@nas.local:445', 'sftp', 'smb', "SMB's port"],
    ['nas.local:139', 'webdav', 'smb', "NetBIOS SMB's port"],
    ['nas.local:443', 'sftp', 'webdav', "HTTPS's port"],
    ['nas.local:80', 'smb', 'webdav', "HTTP's port"],
  ] as const)('with %j and %s selected, says it looks like %s (%s)', (input, selected, looksLike, _why) => {
    expect(addressLooksLike(input, selected)).toBe(looksLike)
  })

  it.each([
    ['sven@192.168.0.153', 'smb', 'the issue #8 shape: `user@host` is a fine SMB address'],
    ['sven@192.168.0.153', 'sftp', 'and a fine SFTP one'],
    ['sven@192.168.0.153', 'webdav', 'and a fine WebDAV one'],
    ['naspolya', 'sftp', 'a bare host fits every protocol'],
    ['ada@nas.local/srv/data', 'webdav', 'so does a path'],
    ['nas.local:2222', 'smb', 'a port nobody owns'],
    ['sftp://nas.local', 'sftp', 'a scheme that agrees'],
    ['https://nas:22/dav', 'webdav', 'a scheme that agrees wins over the port it names'],
    ['', 'smb', 'nothing typed'],
    ['ada@', 'sftp', 'a half-typed address, which is the normal state of a field being typed into'],
    ['ftp://nas.local', 'smb', 'a protocol Cmdr does not speak, which Connect refuses on its own'],
  ] as const)('says nothing about %j with %s selected (%s)', (input, selected, _why) => {
    expect(addressLooksLike(input, selected)).toBeNull()
  })
})
