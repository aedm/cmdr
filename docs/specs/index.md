# Specs index

Open work lives in [GitHub issues](https://github.com/vdavid/cmdr/issues) on the "Cmdr backlog" project, which is where
priorities get set. This folder holds design docs for big planned work, each linked from its issue. A spec goes away
when its work ships (`DETAILS.md` § "Wiping a shipped spec"); what's still open then becomes an issue.

## Planned work

- `elevated-file-operations.md`: **A user couldn't move root-owned files out of a folder their macOS user can't change,
  and had to finish with `sudo`.** An out-of-process native alert, a 24-hour Cmdr admin right, and a tiny on-demand root
  helper. Issues: [#107](https://github.com/vdavid/cmdr/issues/107), [#280](https://github.com/vdavid/cmdr/issues/280).
- `error-report-triage-plan.md`: **Auto-sent error reports arrive one by one, and nothing says whether one is fixed,
  known, or new.** Stable signatures, a D1 registry, fix trailers, and one daily digest. Issue:
  [#108](https://github.com/vdavid/cmdr/issues/108).
- `i18n-glossaries-as-data.md`: **The translator glossaries are prose, so nothing can check the facts in them.** Store
  one typed row per term per locale and generate the markdown. Issue: [#281](https://github.com/vdavid/cmdr/issues/281).
- `search-arena-snapshot.md`: **Opening search waits ~1 s on every reopen past the idle window, and seconds on a
  session's first open.** Map a journaled columnar arena in place. Issue:
  [#114](https://github.com/vdavid/cmdr/issues/114).
- `swap-scan-plan.md`: **A rescan of a completed local index takes ~15 minutes; a fresh parallel scan takes two.** Build
  a fresh index beside the live one and swap it in atomically. Issues:
  [#242](https://github.com/vdavid/cmdr/issues/242), [#243](https://github.com/vdavid/cmdr/issues/243).
- `db-first-listings-plan.md`: **Serve directory listings from the SQLite index instead of `readdir` + `stat`**, so
  first paint is a query. Blocked on a measurement first. Issues: [#244](https://github.com/vdavid/cmdr/issues/244),
  [#245](https://github.com/vdavid/cmdr/issues/245).
- `data-dir-rename-spec-draft.md`: **Plain data-directory names** (`cmdr/`, not `com.veszelovszki.cmdr/`). Cosmetic and
  low value; a timeboxed go/no-go comes first. Issues: [#282](https://github.com/vdavid/cmdr/issues/282),
  [#283](https://github.com/vdavid/cmdr/issues/283).
- `linux-builds-plan.md`: **A Linux release build (AppImage + .deb) and a website that offers it.** Three known Linux
  gaps gate the download button. Issues: [#151](https://github.com/vdavid/cmdr/issues/151),
  [#284](https://github.com/vdavid/cmdr/issues/284)–[#287](https://github.com/vdavid/cmdr/issues/287).
- `dropbox-sync-status-linux.md`: **Cloud badges on Linux, which today are simply absent.** Holds the Dropbox socket
  protocol research and what a Linux arm needs. Issue: [#288](https://github.com/vdavid/cmdr/issues/288).
