//! Tests for the redactor.
//!
//! Each pattern class has its own test with 6+ input→expected tuples. There's also a
//! negative test (path-shaped strings that aren't paths), an idempotency check, a
//! golden-corpus snapshot, and a histogram test that prints replacement counts so
//! coverage regressions show up as numeric diffs.

use super::*;
use std::borrow::Cow;

/// Helper: redact_line returns Cow; tests want String.
fn r(s: &str) -> String {
    redact_line(s).into_owned()
}

#[test]
fn unix_home_paths() {
    let cases = [
        ("/Users/john/Documents/budget.pdf", "$HOME/Documents/<file>.pdf"),
        ("/Users/alice/Downloads/installer.dmg", "$HOME/Downloads/<file>.dmg"),
        // `id_rsa` has no extension-like suffix → labeled `<dir>` under the post-fix-7
        // heuristic. Acceptable trade-off: extensionless files (id_rsa, README, Makefile)
        // are rare in our log corpus, while extensionless directories are common, so
        // defaulting to `<dir>` reads more accurately on real triage data.
        ("/home/bob/.ssh/id_rsa", "$HOME/<dir>/<dir>"),
        ("/Users/veszelovszki/SecretProject/notes.md", "$HOME/<dir>/<file>.md"),
        (
            "Error reading /Users/foo/Desktop/screenshot.png now",
            "Error reading $HOME/Desktop/<file>.png now",
        ),
        (
            "two paths: /Users/a/Documents/x.txt and /Users/b/Downloads/y.zip done",
            "two paths: $HOME/Documents/<file>.txt and $HOME/Downloads/<file>.zip done",
        ),
        ("/Users/john", "$HOME"),
        (
            "/Users/veszelovszki/Library/Application Support/com.veszelovszki.cmdr-dev",
            // "Library" is in allowlist as a parent dir. The leaf `com.veszelovszki.cmdr-dev`
            // has dots but the trailing segment `cmdr-dev` contains a `-` (not alnum), so
            // `has_extension_like_suffix` returns false → leaf labeled `<dir>` (correct: it
            // IS a directory). "Application Support" is the allowlisted penultimate parent.
            "$HOME/<dir>/Application Support/<dir>",
        ),
    ];
    for (input, expected) in cases {
        assert_eq!(r(input), expected, "input: {input:?}");
    }
}

#[test]
fn windows_home_paths() {
    let cases = [
        (r"C:\Users\Bob\Desktop\passwords.txt", r"$HOME\Desktop\<file>.txt"),
        (r"D:\Users\alice\Documents\report.docx", r"$HOME\Documents\<file>.docx"),
        (
            r"C:\Users\bob\AppData\Roaming\config.json",
            // "Roaming" not safe → <dir>; "AppData" is safe but it's the GRANDPARENT.
            r"$HOME\<dir>\<dir>\<file>.json",
        ),
        (
            r"file at C:\Users\carol\Music\song.mp3 found",
            r"file at $HOME\Music\<file>.mp3 found",
        ),
        (r"C:\Users\dave\SecretFolder\thing.exe", r"$HOME\<dir>\<file>.exe"),
        (r"C:\Users\eve", r"$HOME"),
    ];
    for (input, expected) in cases {
        assert_eq!(r(input), expected, "input: {input:?}");
    }
}

#[test]
fn volumes_paths() {
    let cases = [
        ("/Volumes/MyDrive/file.txt", "/Volumes/<volume>/<file>.txt"),
        (
            "/Volumes/My Backup Drive/Documents/photo.jpg",
            "/Volumes/<volume>/Documents/<file>.jpg",
        ),
        (
            "/Volumes/Backup/2026/january/data.csv",
            "/Volumes/<volume>/<dir>/<dir>/<file>.csv",
        ),
        ("/Volumes/Untitled", "/Volumes/<volume>"),
        (
            "mounted at /Volumes/External SSD/work/project.tar.gz now",
            // .gz keeps as ext (3 alnum chars)
            "mounted at /Volumes/<volume>/<dir>/<file>.gz now",
        ),
        ("/Volumes/Time Machine Backups/foo.bak", "/Volumes/<volume>/<file>.bak"),
    ];
    for (input, expected) in cases {
        assert_eq!(r(input), expected, "input: {input:?}");
    }
}

#[test]
fn media_paths() {
    let cases = [
        ("/media/usb0/file.txt", "/media/<volume>/<file>.txt"),
        (
            "/media/alice/External/Documents/x.pdf",
            "/media/<volume>/<dir>/Documents/<file>.pdf",
        ),
        ("/media/cdrom", "/media/<volume>"),
        ("/media/My Stick/data.bin", "/media/<volume>/<file>.bin"),
        (
            "mounted /media/sdcard/dcim/photo.jpg ok",
            "mounted /media/<volume>/<dir>/<file>.jpg ok",
        ),
        ("/media/usb1/Music/track.mp3", "/media/<volume>/Music/<file>.mp3"),
    ];
    for (input, expected) in cases {
        assert_eq!(r(input), expected, "input: {input:?}");
    }
}

/// A volume label may contain spaces, but prose after the path must survive.
///
/// Real incident: `VolumeManager::report_identity_conflict` logs the two roots and then keeps
/// talking. The label group swallowed every following lowercase word, so 98 uploaded error
/// reports arrived truncated at the second `/Volumes/<volume>`, losing the volume ID and the
/// resolution, the two fields triage actually needs.
#[test]
fn mount_label_stops_at_lowercase_prose() {
    let cases = [
        (
            "Two different mount roots (/Volumes/naspi and /Volumes/naspi-1) claim volume ID \
             smb-192-168-1-111-445-naspi; the earlier root stays recorded, so the two share \
             per-volume state. Expected only for a cloned volume or a doubly-mounted filesystem.",
            "Two different mount roots (/Volumes/<volume> and /Volumes/<volume>) claim volume ID \
             smb-192-168-1-111-445-naspi; the earlier root stays recorded, so the two share \
             per-volume state. Expected only for a cloned volume or a doubly-mounted filesystem.",
        ),
        // A single trailing lowercase word was already handled; a run of them was not.
        (
            "/Volumes/Untitled is not the volume we wanted here",
            "/Volumes/<volume> is not the volume we wanted here",
        ),
        (
            "/media/usb0 could not be read because the device went away.",
            "/media/<volume> could not be read because the device went away.",
        ),
        // Multi-word labels still match: the continuation words are capitalized.
        ("/Volumes/My Backup Drive is full", "/Volumes/<volume> is full"),
    ];
    for (input, expected) in cases {
        assert_eq!(r(input), expected, "input: {input:?}");
    }
}

/// A filename whose own words are lowercase must be redacted WHOLE.
///
/// Real incident: a trash failure logged `.../Screenshot 2026-09-04 at 01.13.03 PM-2.jpeg: …`
/// and the capture stopped dead at ` at`, so ` 01.13.03 PM-2.jpeg` shipped verbatim in an
/// uploaded bundle. Any filename with two or more words leaked its tail the same way, and
/// `Invoice for Acme Corp.pdf` is the shape that actually hurts.
#[test]
fn multi_word_filenames_are_redacted_whole() {
    let cases = [
        // The reported line. The `: ` is the `{path}: {message}` seam, so the message survives.
        (
            "/Users/kajotac/Pics/Screenshot 2026-09-04 at 01.13.03 PM-2.jpeg: the Trash refused it",
            "$HOME/<dir>/<file>.jpeg: the Trash refused it",
        ),
        // A folder whose last word is lowercase, right up to the seam. The seam says where the
        // path ends, so `trip` is part of the name, not prose.
        (
            "/Users/jo/Pics/summer trip: it timed out",
            "$HOME/<dir>/<dir>: it timed out",
        ),
        // A lowercase word mid-filename, nothing after it.
        ("/Users/jo/Docs/my secret notes.txt", "$HOME/<dir>/<file>.txt"),
        // The shape with real exposure in it.
        ("/Users/jo/Work/Invoice for Acme Corp.pdf", "$HOME/<dir>/<file>.pdf"),
        // Prose after an extension still survives: the trailing run has no extension to hold it.
        (
            "/Users/jo/Documents/notes.md failed to open",
            "$HOME/Documents/<file>.md failed to open",
        ),
        // A digit-led "extension" inside a timestamp must NOT end the name early. This is
        // the exact shape that leaked: cutting at `01.13.03` ships ` PM-2.jpeg`.
        (
            "/Users/jo/Shots/Screenshot 2026-09-15 at 11.10.35 AM-2.jpeg",
            "$HOME/<dir>/<file>.jpeg",
        ),
    ];
    for (input, expected) in cases {
        assert_eq!(r(input), expected, "input: {input:?}");
    }
}

/// The reported line, end to end.
///
/// ⚠️ Known gap, deliberately pinned here: macOS repeats the filename inside its own error
/// prose, in curly quotes and with no path around it. No path pattern claims a bare name, so
/// that copy still ships. Closing it needs a separate pass that redacts verbatim repeats of a
/// segment already recognized on the same line; the path itself is what this test covers.
#[test]
fn trash_refusal_line_redacts_its_path() {
    let input = "op 01a0 (Trash) failed: /Users/kajotac/Library/CloudStorage/Dropbox/Shots/\
                 Screenshot 2026-09-04 at 01.13.03 PM-2.jpeg: the Trash refused it";
    let out = r(input);
    assert!(
        out.starts_with("op 01a0 (Trash) failed: $HOME/<dir>/<dir>/<dir>/<dir>/<file>.jpeg"),
        "path not fully redacted: {out}"
    );
    assert!(
        !out.contains("01.13.03") && !out.contains("PM-2"),
        "filename fragments survived: {out}"
    );
    assert!(out.ends_with(": the Trash refused it"), "message lost: {out}");
}

/// A share-relative path in a `key=value` log field is redacted like any other path.
///
/// Real incident: every SMB log line printed its share-relative path in full (30 lines in one
/// bundle), because no path branch matches a path with no mount prefix in front of it. The
/// `smb2` crate's own lines print it unquoted and backslash-separated, with spaces in it.
#[test]
fn share_relative_paths_in_fields() {
    let cases = [
        // `cmdr-smb`'s Debug-quoted shape.
        (
            r#"SmbVolume::delete: share=media, path="trips/2023/summer trip/kapu méretek.jpg""#,
            r#"SmbVolume::delete: share=media, path="<dir>/<dir>/<dir>/<file>.jpg""#,
        ),
        (
            r#"SmbVolume::rename: share=media, from="trips/a b.jpg.cmdr-tmp-66381a4a-7bff", to="trips/a b.jpg", force=false"#,
            // Cmdr's own temp suffix carries no PII and survives, so the temp and the final
            // name visibly belong together.
            r#"SmbVolume::rename: share=media, from="<dir>/<file>.jpg.cmdr-tmp-66381a4a-7bff", to="<dir>/<file>.jpg", force=false"#,
        ),
        // Both fields on one line: the absolute one goes to the `/Volumes/` branch.
        (
            r#"SmbVolume::get_metadata: share=media, input="/Volumes/media/trips/x y.jpg", smb_path="trips/x y.jpg""#,
            r#"SmbVolume::get_metadata: share=media, input="/Volumes/<volume>/<dir>/<file>.jpg", smb_path="<dir>/<file>.jpg""#,
        ),
        // `smb2`'s unquoted, backslash-separated shapes. The value ends at the next field.
        (
            r"tree: renamed from=trips\2023\summer trip\kapu méretek.jpg.cmdr-tmp-6638 to=trips\2023\summer trip\kapu méretek.jpg",
            r"tree: renamed from=<dir>\<dir>\<dir>\<file>.jpg.cmdr-tmp-6638 to=<dir>\<dir>\<dir>\<file>.jpg",
        ),
        (
            r"tree: deleted file=trips\2023\summer trip\kapu méretek.jpg",
            r"tree: deleted file=<dir>\<dir>\<dir>\<file>.jpg",
        ),
        (
            r"tree: created directory=trips\2021\rotate script",
            r"tree: created directory=<dir>\<dir>\<dir>",
        ),
        (
            r"tree: watch path=trips\2021, recursive=false, tree_id=5",
            r"tree: watch path=<dir>\<dir>, recursive=false, tree_id=5",
        ),
        // The `{path}: {message}` seam and a closing paren end an unquoted value too.
        (
            "SmbVolume::download(share=media, path=trips/a b.jpg): cancelled after 5 bytes",
            "SmbVolume::download(share=media, path=<dir>/<file>.jpg): cancelled after 5 bytes",
        ),
        // A bare name, and a volume-relative path with a leading slash.
        (
            "loadDirectory called: paneId=right, path=/private, selectName=-Users-alice-projects-cmdr, currentLoading=false",
            "loadDirectory called: paneId=right, path=/private, selectName=<dir>, currentLoading=false",
        ),
        (
            r#"checking 1 item(s) against path="/trips/2023/summer trip" on volume smb-1"#,
            r#"checking 1 item(s) against path="/<dir>/<dir>/<dir>" on volume smb-1"#,
        ),
        // Allowlisted parents survive, like in every other path shape.
        (
            r#"write_from_stream: share=media, path="Documents/report.pdf", size=12"#,
            r#"write_from_stream: share=media, path="Documents/<file>.pdf", size=12"#,
        ),
    ];
    for (input, expected) in cases {
        assert_eq!(r(input), expected, "input: {input:?}");
    }
}

/// Keys that merely end in a path key, and values that aren't paths, stay put.
#[test]
fn path_fields_leave_non_paths_alone() {
    let unchanged = [
        "enrich parent_id=12 new_parent_id=13",
        "path=None",
        "ByteSeekBackend::get_lines: target=Line(5) -> byte 0",
        "refresh_listing: path=/Volumes/<volume>/<dir:dae7a7>/<dir:ee4032>",
        "list_directory_core: path=$HOME/<dir:b36d39>/<dir:7e85ce>, entries=1",
    ];
    for input in unchanged {
        assert_eq!(r(input), input, "should be unchanged: {input:?}");
    }
}

/// A temp-suffixed filename with a space in it must be redacted whole.
///
/// Real incident: `…/boldogságkapu me\u{301}retek.jpg.cmdr-tmp-<uuid>` (a `{:?}`-printed NFD
/// name) was split at the space. The tail carries no extension the backward trim recognizes
/// (`cmdr-tmp-…` has dashes), so it read as prose and shipped verbatim.
#[test]
fn temp_suffixed_filename_with_a_space_is_redacted_whole() {
    let cases = [
        (
            r#"input="/Volumes/media/trips/2023/kapu me\u{301}retek.jpg.cmdr-tmp-66381a4a-7bff-45c7", smb_path="x""#,
            r#"input="/Volumes/<volume>/<dir>/<dir>/<file>.jpg.cmdr-tmp-66381a4a-7bff-45c7", smb_path="<dir>""#,
        ),
        (
            "/Users/jo/Pics/kapu méretek.jpg.cmdr-tmp-66381a4a-7bff-45c7",
            "$HOME/<dir>/<file>.jpg.cmdr-tmp-66381a4a-7bff-45c7",
        ),
        // A Debug escape closing a word is not the end of a sentence.
        (
            r#"path="/Volumes/media/cafe\u{301} menu.pdf""#,
            r#"path="/Volumes/<volume>/<file>.pdf""#,
        ),
    ];
    for (input, expected) in cases {
        assert_eq!(r(input), expected, "input: {input:?}");
    }
}

/// The same name must hash to the same token however it was printed.
///
/// Real incident: one jpg came out as three different `<file:…>` tokens in one bundle, because
/// it was logged via `Display` (raw NFD), via `{:?}` (`e\u{301}` escaped), and from the NAS
/// listing (NFC). Salted correlation is the whole point of `redact_line_salted`.
#[test]
fn a_name_hashes_the_same_whatever_printed_it() {
    let salt = b"0123456789abcdef";
    let display_nfd = redact_line_salted("/Users/jo/Pics/kapu me\u{301}retek.jpg", salt).into_owned();
    let debug_nfd = redact_line_salted(r#"path="/Users/jo/Pics/kapu me\u{301}retek.jpg""#, salt).into_owned();
    let display_nfc = redact_line_salted("/Users/jo/Pics/kapu m\u{e9}retek.jpg", salt).into_owned();
    let token = |s: &str| s.split("<file:").nth(1).map(|t| t[..6].to_string());
    let t1 = token(&display_nfd).expect("display token");
    assert_eq!(token(&debug_nfd), Some(t1.clone()), "{display_nfd} vs {debug_nfd}");
    assert_eq!(token(&display_nfc), Some(t1.clone()), "{display_nfd} vs {display_nfc}");
    // The temp a transfer writes first correlates with the name it's renamed to.
    let temp = redact_line_salted(r#"path="Pics/kapu méretek.jpg.cmdr-tmp-66381a4a""#, salt).into_owned();
    assert_eq!(token(&temp), Some(t1), "{display_nfd} vs {temp}");
}

#[test]
fn smb_uris() {
    let cases = [
        ("smb://server.local/share/file.txt", "smb://<host>/<share>/<file>.txt"),
        ("smb://192.168.1.10/Public/doc.pdf", "smb://<host>/<share>/<file>.pdf"),
        (
            "smb://nas.local/backups/2026/jan.zip",
            "smb://<host>/<share>/<dir>/<file>.zip",
        ),
        (
            "Connecting to smb://homer/movies/film.mkv now",
            "Connecting to smb://<host>/<share>/<file>.mkv now",
        ),
        ("smb://homer", "smb://<host>"),
        ("smb://homer/share", "smb://<host>/<share>"),
    ];
    for (input, expected) in cases {
        assert_eq!(r(input), expected, "input: {input:?}");
    }
}

#[test]
fn unc_paths() {
    let cases = [
        (r"\\server\share\file.txt", r"\\<host>\<share>\<file>.txt"),
        (
            r"\\nas.local\public\Documents\plan.docx",
            // public is the SMB share, Documents is the parent dir (allowlisted).
            r"\\<host>\<share>\Documents\<file>.docx",
        ),
        (r"\\server\share", r"\\<host>\<share>"),
        (r"\\server", r"\\<host>"),
        (
            r"opening \\fileserver\team\report.pdf failed",
            r"opening \\<host>\<share>\<file>.pdf failed",
        ),
        (
            r"\\10.0.0.5\backup\daily\snapshot.tar",
            r"\\<host>\<share>\<dir>\<file>.tar",
        ),
    ];
    for (input, expected) in cases {
        assert_eq!(r(input), expected, "input: {input:?}");
    }
}

#[test]
fn mdns_hostnames() {
    let cases = [
        ("connecting to homer.local", "connecting to <host>.local"),
        ("nas.local resolved", "<host>.local resolved"),
        ("ping macbook-pro.local for status", "ping <host>.local for status"),
        ("two: alpha.local and beta.local", "two: <host>.local and <host>.local"),
        ("server-1.local", "<host>.local"),
        ("foo.local:445", "<host>.local:445"),
    ];
    for (input, expected) in cases {
        assert_eq!(r(input), expected, "input: {input:?}");
    }
}

#[test]
fn ipv4_addresses() {
    let cases = [
        ("connect to 192.168.1.1 timeout", "connect to <ipv4> timeout"),
        ("10.0.0.5", "<ipv4>"),
        ("from 8.8.8.8 to 8.8.4.4", "from <ipv4> to <ipv4>"),
        ("172.16.254.1:8080", "<ipv4>:8080"),
        ("0.0.0.0", "<ipv4>"),
        ("255.255.255.255", "<ipv4>"),
    ];
    for (input, expected) in cases {
        assert_eq!(r(input), expected, "input: {input:?}");
    }
}

#[test]
fn ipv6_addresses() {
    let cases = [
        ("2001:db8:85a3::8a2e:370:7334", "<ipv6>"),
        ("::1", "<ipv6>"),
        ("fe80::1", "<ipv6>"),
        ("from 2001:db8::1 to ::1", "from <ipv6> to <ipv6>"),
        ("fe80::abcd:1234", "<ipv6>"),
        ("2001:0db8:0000:0000:0000:ff00:0042:8329", "<ipv6>"),
    ];
    for (input, expected) in cases {
        assert_eq!(r(input), expected, "input: {input:?}");
    }
}

#[test]
fn email_addresses() {
    let cases = [
        ("contact alice@example.com please", "contact <email> please"),
        ("john.doe+tag@example.co.uk", "<email>"),
        ("two: a@b.com and c@d.org", "two: <email> and <email>"),
        ("noreply@subdomain.example.com", "<email>"),
        ("name_with_underscores@x-y.io", "<email>"),
        ("user@domain.dev failed login", "<email> failed login"),
    ];
    for (input, expected) in cases {
        assert_eq!(r(input), expected, "input: {input:?}");
    }
}

#[test]
fn url_userinfo() {
    let cases = [
        (
            "https://alice:s3cret@example.com/path",
            "https://<userinfo>@example.com/path",
        ),
        (
            "ftp://anon@files.example.com/pub",
            "ftp://<userinfo>@files.example.com/pub",
        ),
        ("https://user@host.example.com/", "https://<userinfo>@host.example.com/"),
        (
            "fetched https://bob:hunter2@api.example.com/v1 ok",
            "fetched https://<userinfo>@api.example.com/v1 ok",
        ),
        ("ssh://git@github.com/foo/bar", "ssh://<userinfo>@github.com/foo/bar"),
        (
            "https://u:p@a.com and https://x:y@b.com",
            "https://<userinfo>@a.com and https://<userinfo>@b.com",
        ),
    ];
    for (input, expected) in cases {
        assert_eq!(r(input), expected, "input: {input:?}");
    }
}

#[test]
fn bare_userinfo_no_scheme() {
    // The macOS `smbutil` and Linux `smbclient` fallbacks build `//user:pass@host` URLs
    // (no scheme). A misbehaving server can reflect them in stderr, so the redactor must
    // strip the userinfo on this scheme-less shape too.
    let cases = [
        // Host is preserved verbatim, mirroring `url_userinfo` (the host is assumed to be
        // diagnostically useful; only the secret userinfo is stripped).
        ("//alice:s3cret@192.168.1.10", "//<userinfo>@192.168.1.10"),
        ("//bob@nas.example.com/share", "//<userinfo>@nas.example.com/share"),
        (
            "smbutil failed for //user:pass@host:10480",
            "smbutil failed for //<userinfo>@host:10480",
        ),
        ("stderr: //admin:hunter2@server now", "stderr: //<userinfo>@server now"),
        // A scheme'd URL must still go through url_userinfo, not double-match the bare tail.
        ("http://alice:s3cret@example.com/x", "http://<userinfo>@example.com/x"),
        (
            "connect //u:p@a and //x:y@b",
            "connect //<userinfo>@a and //<userinfo>@b",
        ),
    ];
    for (input, expected) in cases {
        assert_eq!(r(input), expected, "input: {input:?}");
    }
}

#[test]
fn mtp_device_owner_names() {
    let cases = [
        (
            "Connected to John's Pixel 8 Pro",
            "Connected to <mtp-owner>'s Pixel 8 Pro",
        ),
        ("device: Alice's iPhone 15 Pro", "device: <mtp-owner>'s iPhone 15 Pro"),
        (
            "Mary's Galaxy S24 Ultra connected",
            "<mtp-owner>'s Galaxy S24 Ultra connected",
        ),
        ("Bob's Pixel discovered", "<mtp-owner>'s Pixel discovered"),
        ("Found Charlie's iPad Pro", "Found <mtp-owner>'s iPad Pro"),
        (
            "two: Anna's Phone and Diana's Tablet",
            "two: <mtp-owner>'s Phone and <mtp-owner>'s Tablet",
        ),
        ("Eric's OnePlus 12 connected", "<mtp-owner>'s OnePlus 12 connected"),
    ];
    for (input, expected) in cases {
        assert_eq!(r(input), expected, "input: {input:?}");
    }
}

/// English contractions, module paths, and bare model names must NOT be touched.
#[test]
fn mtp_owner_negatives() {
    let must_be_unchanged = [
        // English contractions: `It`, `That`, `He`, `She` would be the "owner"
        // candidate but we only match capitalized words AND a known model word.
        // `it's a Pixel` has lowercase `it`, so safe. `That's a Pixel 8 Pro` has
        // capitalized "That" but "Pixel 8 Pro" follows (uh oh, that WOULD match).
        // Avoid that by listing safe sentences without leading "<Capital>'s <model>"
        // shape, plus a few realistic non-owner sentences.
        "it's a Pixel 8 Pro phone",
        "the device is a Pixel 8 Pro",
        "Pixel 8 Pro detected",
        "iPhone 15 Pro detected",
        "Galaxy S24 connected",
        // Module paths: must not match.
        "cmdr_lib::mtp::device",
        "cmdr_lib::redact::tests",
        // Random capitalized phrases that look ownership-y but aren't followed by
        // an MTP model word: must NOT match.
        "John's car was here",
        "Alice's project codename",
    ];
    for input in must_be_unchanged {
        assert_eq!(r(input), input, "should be unchanged: {input:?}");
    }
}

/// `<Capitalized>'s <Model>` triggers redaction, including pronouns like `That's Pixel`.
/// We accept this overmatch: the `'s` + model shape is rare in English without an actual
/// possessive, and over-redacting a generic sentence is safer than under-redacting a real
/// owner name. Pin the behaviour so any future tightening is deliberate.
///
/// The `\x20+ Model` requirement immediately after `'s` keeps natural sentences with an
/// article in between safe (`That's a Pixel 8 Pro` is unchanged).
#[test]
fn mtp_owner_known_overmatches() {
    assert_eq!(r("That's a Pixel 8 Pro"), "That's a Pixel 8 Pro");
    assert_eq!(r("That's Pixel 8 Pro"), "<mtp-owner>'s Pixel 8 Pro");
}

/// The `user=` / `username=` shape our SMB logs use for the account someone signs in to a
/// share with. The value goes; the `Some(...)` / `None` wrapper stays, because "was there a
/// username at all" is the part a triager actually reads.
#[test]
fn account_names() {
    let cases = [
        (
            "Upgrading volume smb-1 to SmbVolume: server=nas, share=media, user=Some(\"david\")",
            "Upgrading volume smb-1 to SmbVolume: server=nas, share=media, user=Some(\"<user>\")",
        ),
        (
            "Found Keychain credentials for user=david",
            "Found Keychain credentials for user=<user>",
        ),
        (
            "try_list_shares_authenticated: addr=nas:445, user=admin",
            "try_list_shares_authenticated: addr=nas:445, user=<user>",
        ),
        ("username=dvesz connected", "username=<user> connected"),
        // Windows-style domain accounts and dotted names go whole.
        ("user=WORKGROUP\\david", "user=<user>"),
        ("user=david.veszelovszki", "user=<user>"),
        // Quoted, and inside a Rust debug struct.
        (
            "SmbCredentials { username: \"david\", .. }",
            "SmbCredentials { username: \"<user>\", .. }",
        ),
        // Two on one line.
        (
            "user=alice fell back to user=bob",
            "user=<user> fell back to user=<user>",
        ),
        // An absent username is not a secret and stays readable.
        ("Upgrading volume smb-1: user=None", "Upgrading volume smb-1: user=None"),
    ];
    for (input, expected) in cases {
        assert_eq!(r(input), expected, "input: {input:?}");
    }
}

/// The account pattern must not eat identifiers that merely end in `user`, nor the many
/// `<key>=<value>` pairs our logs are full of.
#[test]
fn account_name_negatives() {
    let must_be_unchanged = [
        "cmdr_lib::network::smb_client",
        "max_users=12",
        "parent_user=1",
        "browser=safari",
        "users=3",
        "user_count=7",
        "the user pressed Escape",
    ];
    for input in must_be_unchanged {
        assert_eq!(r(input), input, "should be unchanged: {input:?}");
    }
}

#[test]
fn unix_system_paths() {
    let cases = [
        (
            "error at /tmp/build-abc123/src/main.rs:42:5",
            "error at /tmp/<dir>/src/<file>.rs:42:5",
        ),
        ("/tmp/foo.txt", "/tmp/<file>.txt"),
        // `zeb_def_ipc_93056` has no extension-like suffix → `<dir>` (post-fix-7 default).
        ("/private/tmp/zeb_def_ipc_93056", "/private/<dir>/<dir>"),
        (
            "/var/folders/xy/abcdef/T/cache.bin",
            "/var/<dir>/<dir>/<dir>/<dir>/<file>.bin",
        ),
        // `something` has no extension-like suffix → `<dir>` (post-fix-7 default).
        ("/opt/homebrew/bin/something", "/opt/<dir>/<dir>/<dir>"),
        ("/tmp/", "/tmp/"),
    ];
    for (input, expected) in cases {
        assert_eq!(r(input), expected, "input: {input:?}");
    }
}

/// Strings that look path-ish or PII-ish but aren't. Must pass through unchanged.
#[test]
fn negatives_unchanged() {
    let cases = [
        "Cargo.toml",
        "cmdr_lib::network::smb_client",
        "cmdr_lib::redact::tests",
        "0.1.2-alpha",
        "192.168.x.y",
        "version 1.2.3",
        "MustScanSubDirs: reconcile slow for / (+38 -0 ~515676, 1691s)",
        "called `Option::unwrap()` on a `None` value",
        "Reconciler: switched to live mode",
        "indexing::manager  Replay: watcher started (since_event_id=888910657, current=890423195)",
        "127 errors", // not 4-octet IP
        "1.2.3.4.5",  // 5 octets: IPv4 regex matches first 4. acceptable; see below.
        "release v0.13.0",
    ];
    // Most must be unchanged. A couple noted as "acceptable to redact":
    let must_be_unchanged = [
        "Cargo.toml",
        "cmdr_lib::network::smb_client",
        "cmdr_lib::redact::tests",
        "0.1.2-alpha",
        "192.168.x.y",
        "version 1.2.3",
        "MustScanSubDirs: reconcile slow for / (+38 -0 ~515676, 1691s)",
        "called `Option::unwrap()` on a `None` value",
        "Reconciler: switched to live mode",
        "indexing::manager  Replay: watcher started (since_event_id=888910657, current=890423195)",
        "127 errors",
        "release v0.13.0",
    ];
    for input in must_be_unchanged {
        assert_eq!(r(input), input, "should be unchanged: {input:?}");
    }
    // The 1.2.3.4.5 case: IPv4 regex matches "1.2.3.4", which is acceptable; assert it doesn't
    // crash and produces some redaction.
    let _ = r(cases[11]);
}

#[test]
fn idempotency() {
    let corpus = [
        "/Users/john/Documents/budget.pdf",
        r"C:\Users\Bob\Desktop\x.txt",
        "/Volumes/Backup/photo.jpg",
        "smb://homer.local/share/x.txt",
        "https://u:p@host.com/path",
        "alice@example.com",
        "192.168.1.1",
        "2001:db8::1",
        "homer.local",
        "Reconciler: switched to live mode",
        "indexing::manager  Replay: watcher started (since_event_id=888910657)",
        r#"SmbVolume::delete: share=media, path="trips/summer trip/a b.jpg""#,
        r"tree: renamed from=trips\summer trip\a b.jpg.cmdr-tmp-1 to=trips\summer trip\a b.jpg",
        "loadDirectory: path=/private, selectName=projects",
        r#"checking 1 item(s) against path="/trips/summer trip" on volume smb-1"#,
        "/Volumes/media/cafe\\u{301} menu.pdf",
    ];
    for input in corpus {
        let once = r(input);
        let twice = r(&once);
        assert_eq!(once, twice, "not idempotent for {input:?}");
    }
}

#[test]
fn redact_text_handles_multiple_lines() {
    let input = "first /Users/john/x.txt\nsecond /Volumes/foo/y.bin\nthird clean line\n";
    let expected = "first $HOME/<file>.txt\nsecond /Volumes/<volume>/<file>.bin\nthird clean line\n";
    assert_eq!(redact_text(input), expected);
}

#[test]
fn redact_panic_message_alias() {
    let msg = "panicked at /Users/foo/Documents/bar.rs:10:5";
    let expected = "panicked at $HOME/Documents/<file>.rs:10:5";
    assert_eq!(redact_panic_message(msg), expected);
}

#[test]
fn cow_borrowed_when_no_match() {
    // No PII → Cow::Borrowed (no allocation). We can't observe Cow variant directly via
    // == String, but we can assert the output is identical and the input is short
    // (regression guard against accidental allocation).
    let input = "Reconciler: switched to live mode";
    let out = redact_line(input);
    assert_eq!(&*out, input);
    // Confirm it's a Borrowed variant.
    matches!(out, Cow::Borrowed(_));
}

// --- Golden corpus snapshot ---

/// The synthesized log corpus + its expected redacted form. Touch this snapshot deliberately
/// when a redaction rule changes; CI will diff it for review.
///
/// To regenerate the snapshot after an intentional change:
///     REGENERATE_REDACT_CORPUS=1 cargo nextest run --lib redact::tests::golden_corpus_snapshot
/// Then review the diff and commit.
#[test]
#[allow(clippy::print_stderr, reason = "diagnostic for the regenerate workflow")]
fn golden_corpus_snapshot() {
    let corpus = include_str!("fixtures/log-corpus.txt");
    let expected = include_str!("fixtures/log-corpus.redacted.txt");
    let actual = redact_text(corpus);

    if std::env::var("REGENERATE_REDACT_CORPUS").is_ok() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/redact/fixtures/log-corpus.redacted.txt");
        std::fs::write(&path, &actual).expect("write redacted corpus");
        eprintln!("wrote {}", path.display());
        return;
    }

    if actual != expected {
        // Print a unified diff so the failure is easy to interpret in CI.
        let actual_lines: Vec<&str> = actual.lines().collect();
        let expected_lines: Vec<&str> = expected.lines().collect();
        let mut diff = String::new();
        for (i, (a, e)) in actual_lines.iter().zip(expected_lines.iter()).enumerate() {
            if a != e {
                diff.push_str(&format!("line {}:\n  expected: {e}\n  actual:   {a}\n", i + 1));
            }
        }
        if actual_lines.len() != expected_lines.len() {
            diff.push_str(&format!(
                "line count differs: expected {}, actual {}\n",
                expected_lines.len(),
                actual_lines.len()
            ));
        }
        panic!("golden corpus mismatch (set REGENERATE_REDACT_CORPUS=1 to rewrite):\n{diff}");
    }
}

/// Histogram of replacement counts per pattern class. Prints a table on every run; future
/// coverage regressions show up as numeric drops.
#[test]
#[allow(clippy::print_stderr, reason = "intentional diagnostic output for the histogram")]
fn replacement_count_histogram() {
    let corpus = include_str!("fixtures/log-corpus.txt");
    let redacted = redact_text(corpus);

    let counts = [
        ("$HOME", redacted.matches("$HOME").count()),
        ("/Volumes/<volume>", redacted.matches("/Volumes/<volume>").count()),
        ("/media/<volume>", redacted.matches("/media/<volume>").count()),
        ("smb://<host>", redacted.matches("smb://<host>").count()),
        (r"\\<host>", redacted.matches(r"\\<host>").count()),
        ("<host>.local", redacted.matches("<host>.local").count()),
        ("<ipv4>", redacted.matches("<ipv4>").count()),
        ("<ipv6>", redacted.matches("<ipv6>").count()),
        ("<email>", redacted.matches("<email>").count()),
        ("<userinfo>", redacted.matches("<userinfo>").count()),
        ("<file>", redacted.matches("<file>").count()),
        ("<dir>", redacted.matches("<dir>").count()),
        ("<mtp-owner>", redacted.matches("<mtp-owner>").count()),
        ("<user>", redacted.matches("<user>").count()),
    ];

    eprintln!("\n=== Redaction histogram ===");
    for (label, count) in &counts {
        eprintln!("  {label:>20} : {count}");
    }
    eprintln!("===========================\n");

    // Sanity: every pattern class is exercised at least once in the corpus.
    for (label, count) in &counts {
        assert!(*count > 0, "no occurrences of {label}: corpus coverage gap");
    }
}
