/**
 * The E2E teardown unmounts only the fixture's own SMB mounts. ❗ `/Volumes/public`
 * alone says nothing about whose share sits there: a person's NAS share, or a dev
 * session's mount of another server, lands at the same path, and a blind `umount`
 * took it away.
 */

import { describe, it, expect } from 'vitest'
import { fixtureMountPoints } from './smb-fixtures.js'

const mountOutput = [
  '/dev/disk3s1s1 on / (apfs, sealed, local, read-only, journaled)',
  '//david@192.168.1.111/naspi on /Volumes/naspi (smbfs, nodev, nosuid, mounted by veszelovszki)',
  '//GUEST:@localhost:11480/public on /Volumes/public (smbfs, nodev, nosuid, noowners, mounted by veszelovszki)',
  '//guest@localhost:10480/public on /Volumes/public-1 (smbfs, nodev, nosuid, mounted by veszelovszki)',
  '//testuser@localhost:10481/private on /Volumes/my private (smbfs, nodev, nosuid, mounted by veszelovszki)',
  '//testuser@localhost:11481/private on /Volumes/private (smbfs, nodev, nosuid, mounted by veszelovszki)',
].join('\n')

const fixture = [
  { host: 'localhost', port: 10480, share: 'public' },
  { host: 'localhost', port: 10481, share: 'private' },
]

describe('fixtureMountPoints', () => {
  it('finds the fixture server’s mounts wherever they landed, and nothing else', () => {
    expect(fixtureMountPoints(mountOutput, fixture)).toEqual(['/Volumes/public-1', '/Volumes/my private'])
  })

  it('leaves a share of the same name on another port or machine alone', () => {
    expect(fixtureMountPoints(mountOutput, [{ host: 'localhost', port: 445, share: 'public' }])).toEqual([])
    expect(fixtureMountPoints(mountOutput, [{ host: '192.168.1.111', port: 445, share: 'naspi' }])).toEqual([
      '/Volumes/naspi',
    ])
  })

  it('matches the host and share in any case', () => {
    expect(fixtureMountPoints(mountOutput, [{ host: 'LOCALHOST', port: 10480, share: 'Public' }])).toEqual([
      '/Volumes/public-1',
    ])
  })
})
