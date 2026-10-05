# Fuzz targets: details

## Why this crate looks the way it does

- **One shared crate, not a `fuzz/` per crate.** Eight targets across four crates plus three app decoders share one
  instrumented build (one `target/`, one lock, one CI cache). Per-crate dirs would build the common dependency graph
  (tokio, quick-xml, the codecs) once per crate.
- **Outside the workspace** (`[workspace]` in its own `Cargo.toml`): the sanitizer build needs nightly flags no other
  lane should see, and cargo-fuzz wants its own lock and target dir. The lane holds no lock on the shared `target/`.
- **`libfuzzer-sys` only, no `arbitrary`**: every target reads raw bytes, which is what the attacker sends. A target
  that needs structure takes it from leading bytes (`adb_sync`'s feature byte, `archive_index`'s format byte).
- **Seams behind a `fuzzing` feature** where the parser is `pub(crate)`. The seam adds no logic: it calls the parser the
  way the transport does (lossy UTF-8 for XML bodies, a scripted `AdbConnection` for the ADB wire).
- **Versions** (verified on crates.io and GitHub, 2026-10-05): `libfuzzer-sys` 0.4.13 (2026-06-04), cargo-fuzz 0.13.2
  (2026-06-09, pinned in `desktop-rust-fuzz.go`), the nightly shared with cargo-udeps.

## Targets

- `adb_sync`: `SyncSession` stat, listing, and pull against device-chosen bytes; first byte picks v1/v2 verbs.
- `adb_shell`: shell v2 frames, bounded and unbounded, then `parse_df_k` on stdout.
- `webdav_propfind`: `parse_multistatus` (namespaces, entities, hrefs, RFC 3339 and HTTP dates).
- `s3_xml`: every S3 response parser over one body (`xml::parse_tree` plus the per-document readers).
- `archive_index`: `ArchiveIndex::parse` over in-memory zip, tar (plain, gzip, bzip2, xz, zstd), and 7z; walks the tree
  asserting every listed path is safe and consistent, then reads up to 256 KiB from each of the first three files.
- `archive_entry_name`: `sanitize_entry_name` never accepts a name that escapes its root, and accepting is idempotent.
- `pdf`: the app's `pdf-extract` calls (load, page tree, encryption, Info strings, the first three pages) inside
  `catch_unwind`, behind the app's `parent_chain_ends` guard.
- `image_headers`: the `image` dimensions read and EXIF shaping, which the app does NOT contain, so a panic counts.

## Running and reproducing

- Everything: `pnpm check fuzz` (`CMDR_FUZZ_SECONDS=600` for a real hunt). Per target, from `fuzz/`:
  `cargo +<nightly> fuzz run -O -a <target> corpus/<target> seeds/<target> -- -max_total_time=120 -timeout=20`.
  `./scripts/check/check --print-nightly` prints the nightly.
- A finding lands in `artifacts/<target>/` (CI uploads it as `fuzz-artifacts`). Reproduce with
  `cargo +<nightly> fuzz run -O -a <target> artifacts/<target>/<file>`; minimize with `fuzz tmin`.
- Then: a regression test in the owning crate, red first, then the fix. Copy the input into `seeds/<target>/` once it
  passes, so every run replays it.
- New target: a `fuzzing` seam if the parser is private, a file in `fuzz_targets/`, a `[[bin]]` in `Cargo.toml`, and a
  seed dir. The lane discovers targets through `cargo fuzz list`.

## Findings

- **ADB, ours, fixed**: a sync payload or shell frame length straight off the wire sized a `Vec` (4 GiB per packet).
  Capped before allocating; `crates/cmdr-adb/DETAILS.md` § The wire contract.
- **PDF, `pdf-extract` 0.12.0, guarded in our wrapper**: a page that is its own `Parent` with no `Resources` recursed
  forever in `get_inherited` (stack overflow, uncontainable). `parent_chain_ends` in `inspect/pdf.rs` skips such pages.
  Worth reporting upstream.
- **xz, `lzma-rust2` 0.18.1, open**: a 25-byte `.tar.xz` whose block header asks for a ~3 GiB LZMA2 dictionary makes
  `XzReader` allocate it up front. `XzReader` has no memory limit to set. Input (base64, with the target's format byte):
  `BP03elhaAAAE5ta0RgCezuxft9v//+Al`. Until it's bounded, `archive_index` stops on it within seconds.

## Deliberately not fuzzed here

- **SMB**: the protocol layer is the `smb2` crate, which has its own 14 targets and a weekly fuzz workflow.
- **MTP**: `mtp-rs` parses PTP; it has proptest but no fuzz targets yet. Belongs in that repo.
- **SFTP**: `russh` / `russh-sftp` parse the wire; Cmdr's SFTP code maps typed results. `known_hosts.rs` reads the
  user's own file, not an attacker's.
- **Git**: `gix` parses repos and fuzzes upstream.
- **The ADB host messages** (`host:devices-l`, features): split-and-trim over a 16-bit-length message; nothing to find.
- **Thumbnails**: the native ImageIO / Vision backend in `cmdr-index`, not Rust code.
