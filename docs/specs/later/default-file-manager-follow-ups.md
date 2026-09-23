# Default file manager: what's still open

Reveal-in-Cmdr shipped: another app's "Show in Finder" lands in a Cmdr pane once the person switches it on, through the
undocumented `NSFileViewer` global default. The mechanism, the cold-launch buffering, the uninstall guardrails, and every
decision live in `apps/desktop/src-tauri/src/reveal/DETAILS.md`; the Settings row, the once-ever offer, and the
first-landing notice live in `apps/desktop/src/lib/reveal/DETAILS.md`. Three items are open.

## 1. Folder opens land in Cmdr (the `public.folder` handler)

- **Problem**: folder opens dispatched through LaunchServices (`open .` in a terminal, a Spotlight folder hit, another
  app opening a folder URL) still go to Finder. Reveal-in-Cmdr only covers "Show in Finder".
- **Impact**: medium, for people who want Cmdr as their everyday file manager. This is the bolder half of "default file
  manager"; reveal is the gentle one.
- **Solution**:
  - Declare Cmdr a folder viewer in `apps/desktop/src-tauri/Info.plist` (which Tauri merges): `CFBundleDocumentTypes`
    with `LSItemContentTypes = [public.folder]`, `CFBundleTypeRole = Viewer`, `LSHandlerRank = Alternate` (so Cmdr never
    becomes the default by itself). Tauri's `bundle.fileAssociations` is extension-based and can't express a UTI-only
    type (tauri#13159). Side effect: Cmdr shows in Finder's "Open With" menu for folders even with the toggle off, which
    is arguably a free feature.
  - Register through the sanctioned API: `NSWorkspace.setDefaultApplication(at:toOpenContentType:completionHandler:)`
    (macOS 12+, so it needs a `macos_at_least(12, 0)` gate plus the `allowed-newer-selector` marker, and the toggle
    hides below macOS 12), `urlForApplication(toOpen:)` to read the current holder, and Finder's app URL to revert.
    ❌ Never poke `com.apple.launchservices.secure` with `defaults write` (the recipe most blogs show): it's fragile and
    an LS database rebuild can drop it.
  - Mirror the reveal row's rules: off by default, read through to the OS on every open (no stored setting), name a
    third-party incumbent rather than silently replacing it, and render the state the OS was left in.
  - Delivery can reuse `reveal/delivery.rs`: a folder URL already navigates into the folder. Open question: when Cmdr is
    already running, navigate the focused pane or open a new tab? Recommendation: a new tab, so the person's current
    locations survive.
  - Spike before building, all unverified: which surfaces actually route through LaunchServices (double-clicking a folder
    inside a Finder window is expected to stay in Finder, so the copy must not promise "replaces Finder"); what happens
    when Cmdr.app is deleted while registered (compare the dangling-`NSFileViewer` problem in
    `reveal/DETAILS.md` § "The uninstall problem"); and any side effect of the `Info.plist` declaration itself.
- **Size**: M (about a day for the mechanism, plus the Settings row, copy, and i18n).
- **Blocked on**: a David decision on whether he wants it, and on the tab-vs-pane question.

## 2. Offer reveal-in-Cmdr during onboarding

- **Problem**: the reveal feature is offered in Settings and by a once-ever offer toast a couple of launch days in
  (`apps/desktop/src/lib/reveal/`), but not in onboarding. The original plan put an opt-in on onboarding's optional step
  (`apps/desktop/src/lib/onboarding/StepOptional.svelte`).
- **Impact**: low. The launch-day offer already reaches everyone who keeps using Cmdr, so an onboarding row mostly moves
  the moment earlier.
- **Solution**: add a toggle to the optional step that calls the same `setRevealHandlerEnabled` command and renders the
  returned state, with the same production-build and Applications-folder gates the Settings row honors. If it lands,
  the launch-day offer should stay silent for anyone who already answered it there. Pitch it honestly: "reveals land in
  Cmdr", never "Cmdr becomes your default file manager".
- **Size**: S.
- **Blocked on**: a David decision: is the launch-day offer enough?

## 3. Check which apps' "Show in Finder" actually redirects

- **Problem**: reveal-in-Cmdr is verified for `open -R` and the two `NSWorkspace` reveal calls that Chromium and Electron
  use (macOS 26.6, 2026-09-09). AppleScript's `tell application "Finder" to reveal` does not redirect, by design. Nobody
  has checked the apps people actually use.
- **Impact**: low. The Settings and offer copy can only be as honest as what we know; an app that bypasses the key would
  read as a Cmdr bug.
- **Solution**: run a per-app matrix (Chrome, Safari, VS Code, Slack, Mail, Xcode, the Terminal's `open -R`) with the
  feature on, record the results in `apps/desktop/src-tauri/src/reveal/DETAILS.md` § "The mechanism", and adjust copy if
  a common app ignores the key.
- **Size**: S (an hour with a production build).
- **Blocked on**: nothing, but it needs a production build installed in Applications, so it's a David chore.
