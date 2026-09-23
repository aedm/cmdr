# Image index follow-ups

The image index is shipped and in users' hands: OCR and tag search, image similarity, CLIP natural-language search,
opt-in SMB enrichment, and photo search as an Ask Cmdr and MCP tool. It lives in `crates/cmdr-index/src/media_index/`;
its founding decision log (cited from code as `media_index Decision N`) is `media_index/DETAILS.md` § "Key decisions".
What's below was parked on purpose: faces because David wants to be closer in the loop for them, and captions because
they're optional.

## 1. Faces pipeline: detect, embed, and cluster photos by person (no names yet)

- **Problem**: The image index can't find photos of a person. Apple's Vision framework detects faces but deliberately
  offers no face identity, so the identity half has to come from our own model.
- **Impact**: "Every photo of Dóri" is one of the most natural photo searches, and it's the one the "AI-native file
  manager" promise most obviously lacks. This item alone gives search by cluster ("this face"), and it's the base the
  naming work (item 2) builds on.
- **Solution**:
  - Detect with Vision (`VNDetectFaceRectanglesRequest`, plus `VNDetectFaceCaptureQualityRequest` to pick the best
    crop). Embed with an ArcFace-family Core ML model downloaded through the existing CLIP install path
    (`clip/install.rs`: SHA-256 verified before unpack). Verify the license before picking one: AuraFace is
    commercial-friendly, several ArcFace weights aren't. Cluster by cosine (agglomerative or HDBSCAN) into a
    `face_cluster` table in `media.db`.
  - A SEPARATE faces opt-in with its own privacy copy (`media_index Decision 6`), gated on Apple Silicon.
  - Face-crop avatars are BLOBs in the disposable `media.db`, GC'd with their rows, excluded from crash and error report
    bundles, and covered by `docs/security.md`'s redaction and backup posture. That's curated output, so it doesn't
    break `media_index Decision 5` (no thumbnail files as enrichment input).
  - Stamp every stored face embedding with the enrichment-provenance key
    `{model id + version, Core ML / OS version, tag-taxonomy version}` from day one (`media_index Decision 4`), so item
    2's re-attach gate has something to check.
  - Measure clustering thresholds on real libraries and record them in `docs/notes/`; never hardcode them blind.
  - Tests: TDD red→green on cluster merge/split correctness and on clustering honoring must-link / cannot-link
    constraints (stub the store item 2 adds); a fake-backend pipeline test; a macOS-gated detect+embed test on a fixture
    with known faces; an E2E where a cluster-id search returns the right photos.
- **Size**: L. Blocked on a David decision: he wants to be closer in the loop for faces, so start only when he says go.

## 2. Face naming, a durable identity store, and conservative re-attach (the People UI)

- **Problem**: Once faces cluster (item 1), the user will name people, merge and split clusters, and say "not this
  person". `media.db` is a disposable cache that a schema bump or corruption deletes, so that human work needs a store
  that survives it, and after a model or OS change it must never silently attach a name to the wrong face.
- **Impact**: A silent mislabel is worse than a miss: it's a privacy and trust failure in the most personal part of
  someone's photos. Losing hours of naming to a cache wipe is a data-loss bug by Cmdr's own principles.
- **Solution** (the data-safety design, `media_index Decision 4`):
  - A separate durable store holds, per named identity: the name, every correction, and one or more embedding centroids,
    each tagged with the provenance stamp.
  - **Every correction and negative carries a space-independent anchor**: `(path, IoU-tolerant face bounding box)`, in
    addition to any embedding. A "not this person" veto stored only as an old-space embedding can't be checked after a
    model change, so the user could silently re-approve the exact face they rejected. IoU-tolerant because re-detection
    shifts crops and face counts across OS versions.
  - **Re-attach is conservative.** If `media.db` survived (the common case), identity links survive with it, keyed by
    face id, never by path (a photo can hold several faces). After a true re-embed, re-attach by centroid cosine ONLY
    when the provenance stamp matches AND a self-check passes (re-embed one known-good stored face and require its
    cosine to its centroid to stay above a sanity floor, which catches drift the stamp missed). Any mismatch marks the
    identity "needs re-confirm" in the People UI, never a cross-space cosine match. Even on a clean match, use a high
    threshold plus a "Still <name>?" confirmation for low-confidence re-attaches.
  - **Negatives are hard vetoes on every re-attach AND re-cluster.** A face removed from "Dóri" will again be
    cosine-nearest to Dóri after a regenerate; a positive-only matcher would re-introduce the exact mislabel the user
    fixed. Cannot-link beats must-link: when a transitive must-link (a–b, b–c) would violate a cannot-link (a–c), drop
    and flag the must-link.
  - **Storage substrate is a decision to make here.** Lean: a migrating SQLite ladder like `operation-log.db` and
    `agent/main.db` (append-only forward ladder, refuse a downgrade, delete-and-recreate only on a typed
    unparseable-file code), because names, corrections, negatives, anchors, and centroids are relational and will
    evolve. The alternative is atomic JSON like `favorites/`: simpler and inspectable, no schema. The data-safety rules
    above hold either way.
  - People UI: largest unnamed clusters first, best-quality crop as avatar, name / merge / split / "not this person",
    and search by name. Person names join the photo-search agent tool's text-only DTO and its egress gate
    (`media_index Decision 10`).
  - Before choosing the self-check floor, confirm empirically that an OS upgrade can drift embeddings or the Vision tag
    taxonomy while the model id stays the same, and record the before/after data in `docs/notes/`.
  - Disabling image indexing must say what happens to this store (kept with a clear notice, or exportable), never
    silently wipe or silently keep it.
  - Tests (data-safety critical; the lead re-runs them rather than trusting delegation): names and corrections re-attach
    after a simulated `media.db` wipe; refuse a cross-space match on a stamp mismatch; same stamp but drifted embeddings
    fail the self-check into "needs re-confirm"; a "not this person: X" veto never re-attaches to X after a regenerate
    even when X's centroid is nearest; that veto resurfaces by its anchor after a model change; the transitive must-link
    conflict. E2E: name, search, merge, remove-then-regenerate without snap-back, and a stamp bump that asks to
    re-confirm.
- **Size**: XL. Blocked on item 1, and on a David decision for the storage substrate.

## 3. LLM captions for photos (on-device first, cloud optional)

- **Problem**: OCR, tags, and CLIP cover text in images, labels, and visual similarity to a phrase, but not a
  describable scene ("kids building a sandcastle at dusk") as searchable text.
- **Impact**: A nice-to-have on top of CLIP, which already answers most scene queries. Genuinely optional.
- **Solution**:
  - On-device captions through Apple's Foundation Models. It's Swift-only, so this needs a Swift bridge (a Swift static
    library or sidecar built as its own subproject, called over FFI). Spike the bridge first, and verify that Foundation
    Models accepts image input on the current macOS; that claim is still unverified.
  - Captions feed the existing FTS5 index in `media.db`, beside OCR and tag text.
  - An optional cloud route through a frontier vision model reuses the `agent/` stack (`agent/CLAUDE.md`): the
    backend-enforced consent gate (`agent/consent.rs`) with a separate egress consent copy version, the cost meter
    (`agent/pricing.rs`), and the request/response log (`ai/llm_log/`). Expect a thin image-in, text-out sibling to
    `AgentLlm`, because `AgentPart` has no image part. Never the default.
  - Egress is sensitive derived content: a caption of an ID scan leaks the ID (`docs/security.md`).
  - Tests: TDD red→green on provider selection and the consent gate, including that the cloud path sends nothing when
    consent is off.
- **Size**: L (the Swift bridge is the unknown). Not blocked; low priority.

## 4. Enrich images on an MTP device when the user visits a folder

- **Problem**: MTP devices (phones, cameras) never get image enrichment. `media_index` deliberately skips them for
  background sweeps (`network/DETAILS.md` § "MTP stays on-demand, never background"), but the on-demand-per-visit
  trigger that was meant to replace the sweep isn't wired.
- **Impact**: Photos on a connected phone aren't searchable by content until copied to a local disk. Low impact while
  few users browse phones in Cmdr, and MTP reads are slow and transient, so it must stay conservative.
- **Solution**: When the user opens a folder on an MTP volume with image indexing on, enrich just that folder's images
  in the background, cancelable, bounded in concurrency, and paused on disconnect like the SMB path. Reuse the SMB
  byte-fetch and resumability rules in `network/`.
- **Size**: M. Needs a David call on whether it's worth doing at all.

## 5. Offer to delete the image index when turning image indexing off

- **Problem**: Turning off "Index image contents" stops work and keeps every row (`media_index/DETAILS.md` § "Disabling
  stops the running pass (not just future ones)"). Nothing in the UI offers to delete `media.db`, so a user who turns it
  off for privacy keeps an OCR'd copy of their images' text on disk without being told.
- **Impact**: Small, but it's a privacy expectation: someone disabling the feature may assume the derived data goes too.
- **Solution**: When the toggle goes off, show a short confirm that keeps the index by default and offers "Delete the
  image index too" (with its size), which removes each volume's `media-{volume_id}.db` and ANN files through the writer
  registry (`MediaWriter::purge_volume` exists but drops only status and OCR rows and has no production caller yet).
  Once faces exist, say explicitly that named people are kept (item 2).
- **Size**: S. Needs a David decision on the copy and whether the default keeps or deletes.
