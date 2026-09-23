# WebDAV follow-ups

The backend is `crates/cmdr-webdav` (canonical account, including what real servers answer and what it deliberately
doesn't support: `crates/cmdr-webdav/DETAILS.md`), the app-side stores and wiring are
`apps/desktop/src-tauri/src/network/DETAILS.md` § "The WebDAV twin", the frontend it shares with SFTP is
`apps/desktop/src/lib/servers/DETAILS.md`, and the Docker fixtures are `apps/desktop/test/webdav-servers/README.md`.
What's open is below; each item stands alone.

## 1. Trust-on-first-use for self-signed NAS certificates

- **Problem**: most home NAS boxes present a self-signed certificate, and that connect answers `certificate_untrusted`
  and stops. There's no way to say "trust this one", so the sign-in sheet has to word around a wall it can't offer a
  button for.
- **Impact**: high for the NAS audience: it's the day-one wall for anyone connecting to their own Synology or QNAP over
  HTTPS.
- **Solution**: a fingerprint prompt (SHA-256 of the leaf, shown the way the SFTP host-key prompt shows a key
  fingerprint), an app-side trusted-certificate store mirroring `apps/desktop/src-tauri/src/network/sftp_host_keys.rs`
  (keyed `(host, port)`, one entry per fingerprint, with `approve` / `forget` / `list` commands), and a `reqwest` client
  built with a custom root or verifier for that host. The tricky half is the verifier: `reqwest`'s
  `add_root_certificate` accepts a CA, not a leaf, so a self-signed leaf either goes in as its own root or the client
  uses a `rustls` verifier comparing the presented chain against the pinned fingerprint. Decide once and record it in
  `crates/cmdr-webdav/DETAILS.md`. Related: `CertificateUntrusted` today covers ANY TLS refusal (`tokio-rustls` surfaces
  them all as `InvalidData`), so a trust prompt also needs the `rustls::Error` downcast that tells "untrusted" apart
  from other handshake failures.
- **Size**: L, two to three days, most of it the verifier and its tests against a fixture serving a self-signed
  certificate (the Apache stack can grow a service for it).

## 2. Nextcloud chunked upload for large files

- **Problem**: RFC 4918 PUT is single-shot, and Nextcloud's reverse-proxy defaults cut a request at a few hundred MB.
- **Impact**: copying a large file (a video, a disk image) to a typical Nextcloud fails partway, after the user waited
  for most of it.
- **Solution**: detect Nextcloud once per connect (the `OC-` response headers or a `/status.php` probe) and route
  writes above a threshold through Nextcloud's chunking API (`remote.php/dav/uploads/<user>/<id>`: MKCOL a staging
  collection, PUT numbered chunks, MOVE the collection's `.file` to the destination) instead of the staged PUT+MOVE.
  The staged write already ends in a MOVE, so the assembly step is the same last line.
- **Size**: M, about two days. The container it needs exists: `webdav-fixture-nextcloud`, in its own stack mode
  outside the default lane (`apps/desktop/test/webdav-servers/README.md` § "The Nextcloud server").

## 3. A by-hand pass against a Synology and a Nextcloud behind nginx + php-fpm

- **Problem**: the real-server claims in `crates/cmdr-webdav/DETAILS.md` § "What a real server answers" are observed on
  one server only (the official Nextcloud image, Apache + `mod_php`). Two shapes are unwatched: a Synology, and a
  Nextcloud behind nginx + php-fpm, the deployment where "sabre/dav answers 411 to a chunked PUT" is plausible because
  PHP never sees a chunked body. The Synology is also where RFC 4331 quota hasn't been looked at.
- **Impact**: medium. A wrong belief about ranges, the PUT length, or quota surfaces for the first real user on that
  server, as a failed write or a missing free-space figure.
- **Solution**: point the suite at each server by hand with `CMDR_WEBDAV_TEST_URL`
  (`apps/desktop/test/webdav-servers/README.md` § "Against a server of your own"), check the quota numbers on the
  Synology, and record the answers with evidence anchors in `crates/cmdr-webdav/DETAILS.md` § "What a real server
  answers".
- **Size**: S, an afternoon. **Blocked on** access to a Synology and a php-fpm Nextcloud; can't be automated here.

## 4. The collection's self entry behind a proxy that rewrites hrefs

- **Problem**: `query.rs` leaves the collection's own row out of a PROPFIND listing by comparing its href with the
  base path. A reverse proxy that rewrites hrefs would make that comparison miss.
- **Impact**: low, but visible: a phantom child folder named after the directory itself, which opens into itself.
- **Solution**: test against a proxied Nextcloud (an nginx in front of `webdav-fixture-nextcloud` with a path prefix)
  and, if it shows, match the self entry on something the rewrite preserves.
- **Size**: S, an hour or two with the fixture.

## 5. A file where an ancestor folder should be reads as "folder exists"

- **Problem**: the shared `cmdr_fs::volume::mkdir_all::create_directory_all` (used by both `cmdr-webdav` and
  `cmdr-sftp`) treats `AlreadyExists` on an ancestor as "it's there, carry on". On WebDAV, MKCOL answers 405 for ANY
  occupied name, so a FILE sitting where an ancestor folder should be passes as a folder, and the failure surfaces
  later as the leaf's `NotFound`.
- **Impact**: low. The user gets a confusing "not found" for a folder they asked to create instead of a clear "a file
  is in the way". No data risk.
- **Solution**: on `AlreadyExists` for an ancestor (and for the leaf, which answers `AlreadyExisted` the same way), stat
  the name and refuse with a typed error when it isn't a directory. Check whether SFTP has the same blind spot while
  there.
- **Size**: S, an hour plus a cell per backend.

## 6. Digest authentication

- **Problem**: the crate speaks Basic only. A Digest-only server gets the typed `AuthMethodUnsupported` refusal (pinned
  by `webdav-fixture-digest` on port 13481), so the UI says what the server wants instead of "wrong password", but it
  can't connect.
- **Impact**: none known. Synology's default is Basic over HTTPS, Fastmail is Basic, and so are Nextcloud and ownCloud.
- **Solution**: implement RFC 7616 Digest in the client (a challenge round trip plus MD5 / SHA-256 hashing), with the
  fixture already there to test against.
- **Size**: M, about a day. **Blocked on a trigger**: a real user's server that needs it.
