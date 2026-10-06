# Proxies, PAC files, and TLS inspection (2026-10)

How Cmdr's outbound connections behave on a corporate network: behind an HTTP(S) proxy, with a PAC file, and behind a
TLS-inspecting proxy whose root CA is trusted on the Mac. Verified on Cmdr 0.50.0 (`/Applications/Cmdr.app`) and the
worktree at `3095412ec`, macOS 27.0 (Darwin 27.0.0), 2026-10-06. This answers the `/trust` gap "Not tested behind a
TLS-inspecting proxy" (vdavid/cmdr#118).

## Summary

- **Every HTTP request Cmdr makes goes through one reqwest configuration**, so they all behave the same: reqwest 0.13.4,
  rustls with `rustls-platform-verifier` 0.7.0, and hyper-util 0.1.20's system-proxy matcher. No call site sets
  `.proxy()`, `.no_proxy()`, a custom root store, or `danger_accept_invalid_certs` (verified by grep over
  `apps/desktop/src-tauri/src` and `crates/`).
- **Honored**: the static HTTP and HTTPS proxy from System Settings (read live from `SCDynamicStore` each time a client
  is built), and the `HTTPS_PROXY` / `HTTP_PROXY` / `ALL_PROXY` / `NO_PROXY` env vars.
- **Not honored**: PAC files (automatic proxy configuration), WPAD (auto proxy discovery), and the system proxy's bypass
  list ("Bypass proxy settings for these hosts"). With a PAC file, Cmdr connects directly.
- **TLS trust comes from macOS**: certificates are checked by Security.framework (`SecTrust`), so a root CA trusted in
  the keychain (as MDM deploys a company's inspection CA) is trusted by Cmdr too. An untrusted inspection CA fails
  cleanly with `errSecNotTrusted` (-67843).
- **Loopback and LAN traffic gets proxied** when a system proxy is on, because the bypass list is ignored and nothing
  exempts `127.0.0.1`. That breaks Cmdr's own local AI server and LAN WebDAV/S3 on a proxied network.

## Outbound connections (static inventory)

All use `reqwest::Client::builder()` (or `Client::new()`) with timeouts and, for AI, a redirect policy; nothing else.

- **Update check and download** (`updater/mod.rs`): `api.getcmdr.com/update-check/…` → `getcmdr.com/latest.json`, then
  the tarball from GitHub Releases (`github.com` → `release-assets.githubusercontent.com`). Custom updater, not
  `tauri-plugin-updater`'s client (the plugin's crate is in the graph but its download path isn't used on macOS).
- **License validate / activate** (`licensing/validation_client.rs`): `api.getcmdr.com`.
- **Usage stats** (`analytics/heartbeat.rs`): `api.getcmdr.com/heartbeat`.
- **Crash reports** (`crash_reporter/pending_delivery.rs`), **error reports and amend** (`error_reporter/`):
  `api.getcmdr.com`.
- **Feedback, beta signup** (`feedback.rs`, `commands/beta_signup.rs`): `api.getcmdr.com`.
- **S3 price table** (`s3_costs/price_source.rs`): `api.getcmdr.com/s3-prices/v1`.
- **Cloud AI** (`ai/client.rs` via `genai`, `ai/connection_check.rs`): the provider the user configures.
- **Local AI health check** (`ai/client.rs` `health_check`): `http://127.0.0.1:<port>/health`, plain HTTP. The chat
  calls to the local llama-server go to the same loopback address through `genai`.
- **Model downloads** (`ai/download.rs`, image-search model via `cmdr-index` `clip/install.rs`): `huggingface.co`, which
  302-redirects to a CDN host under `hf.co` (seen: `us.aws.cdn.hf.co`, `curl -sI`, 2026-10-06).
- **Remote file backends**: WebDAV and S3 use the same reqwest (`crates/cmdr-webdav`, `crates/cmdr-s3`), so they follow
  the same proxy rules. SMB, SFTP, MTP, and ADB open their own sockets and never use an HTTP proxy (no SOCKS either).
- **Webview**: no frontend `fetch` to an external host (grep, plus the CSP's `connect-src`). Links open in the browser.

## Why it behaves this way (source evidence)

- **Proxy**: reqwest's builder defaults to `Matcher::system()`, which calls hyper-util's `Builder::from_system()`: env
  vars first, then on macOS `mac::with_system` reads only `HTTPEnable/HTTPProxy/HTTPPort` and the `HTTPS` trio from
  `SCDynamicStore::get_proxies()`. It never reads `ProxyAutoConfigEnable`, `ProxyAutoDiscoveryEnable`, or
  `ExceptionsList`, and it has no implicit loopback exemption (`hyper-util-0.1.20/src/client/proxy/matcher.rs`).
- **Fragile**: the app's own `reqwest` line (`apps/desktop/src-tauri/Cargo.toml`) has `default-features = false` without
  `system-proxy`. The feature reaches the build only through `genai` 0.6.5's dependency, by Cargo feature unification
  (`cargo tree -e features -i reqwest`). If `genai` dropped it, the system proxy would silently stop working and only
  env vars would remain.
- **TLS**: reqwest's `rustls` feature pulls in `rustls-platform-verifier`, whose Apple backend (`verification/apple.rs`,
  present in the release binary per `strings`) evaluates the chain with `SecTrust`. The release binary links
  `Security.framework` and `SystemConfiguration.framework` (`otool -L`).
- **Proxy auth**: only credentials embedded in a proxy URL (`http://user:pass@host:port`) work. reqwest doesn't read the
  system proxy's stored credentials and can't do NTLM or Kerberos (`407` stays a failure). Static finding, not tested.

## Empirical tests

Setup: mitmproxy isn't installed, so a ~150-line Go proxy stood in for it (scratchpad only, bound to `127.0.0.1`): a
passthrough mode that logs each `CONNECT` host, and an inspecting mode that terminates TLS with a leaf cert minted by
its own throwaway CA, the way a corporate TLS-inspecting proxy does. It also served the PAC file. A probe binary pinned
to Cmdr's exact `Cargo.lock` and reqwest feature set
(`json, rustls, stream, multipart, system-proxy, gzip, charset, http2`) and built like Cmdr's clients made the requests.
System proxy changes went on the Wi-Fi service with `networksetup`, restored by a shell `trap`.

- **Env var, inspecting proxy, CA not trusted**: `HTTPS_PROXY=http://127.0.0.1:18443`, GET
  `https://getcmdr.com/latest.json` → went through the proxy, then failed:
  `invalid peer certificate: … certificate is not trusted: -67843`. The proxy saw the client abort the handshake
  (`unknown certificate`). Fails closed, as it should.
- **Env var, passthrough proxy**: → 200 through the proxy.
- **Loopback over plain HTTP with `HTTP_PROXY` set**: `http://127.0.0.1:18444/…` went to the proxy. With
  `NO_PROXY=127.0.0.1,localhost` it went direct.
- **Static system proxy**: → the probe's HTTPS request and its `http://127.0.0.1` request both went through the proxy.
- **Static system proxy plus bypass list** containing `getcmdr.com` and `127.0.0.1`: both still went through the proxy.
  The bypass list is ignored.
- **PAC file** (`ProxyAutoConfigURLString` set, PAC returning `PROXY 127.0.0.1:18444` for every non-local host): the
  probe connected directly; the proxy saw no `CONNECT` from it and the probe never fetched the PAC file. Chrome,
  Dropbox, Obsidian, and `CFNetworkAgent` fetched the PAC within a second. On a network where only the proxy can reach
  the internet, Cmdr would fail every request.
- **The real release app, static system proxy**: held a passthrough system proxy across Cmdr 0.50.0's scheduled update
  check. The proxy logged `CONNECT api.getcmdr.com:443` at 10:40:07.38 and `CONNECT getcmdr.com:443` at 10:40:07.95, and
  Cmdr's own log (`~/Library/Logs/com.veszelovszki.cmdr/cmdr.log`) shows
  `reqwest::connect proxy(http://127.0.0.1:18444/) intercepts 'Some("api.getcmdr.com")'`. That DEBUG line is the
  quickest way to see whether a user's Cmdr is using their proxy.

Not run against the live app: license validate, crash or error report upload, and model download. They share the client
and configuration above, and sending test traffic to prod endpoints wasn't worth it.

Not yet run: the positive case, with the test CA trusted in the keychain. Trusting a root needs David's password at a
GUI prompt. The claim that a trusted company CA works rests on the source (`SecTrust` evaluation) plus the failing case
above, whose `-67843` is macOS's own trust verdict, not a bundled root list.

Side note: this network intermittently gave `No route to host (os error 65)` for direct requests while `curl` worked
(the host resolves to IPv6 first). It's unrelated to proxies, but it can make a PAC test look like a proxy failure.
Rerun before drawing conclusions.

## Proposed fixes (not implemented)

1. **Declare `system-proxy` on the app's own reqwest dependency.** Removes the dependency on `genai` unifying it in. One
   line in `apps/desktop/src-tauri/Cargo.toml` (and the WebDAV/S3 crates if they're meant to work standalone). Size: XS.
   Clear win.
2. **Never proxy loopback, and honor the system bypass list.** Build every client through one shared helper that reads
   `ExceptionsList` and `ExcludeSimpleHostnames` from `SCDynamicStore`, adds `localhost`, `127.0.0.1`, and `::1`, and
   passes the result as `reqwest::NoProxy` (`Proxy::no_proxy`). Fixes local AI and LAN WebDAV/S3 on proxied networks.
   The helper also gives one place to add fix 3. Size: S (about a day with tests; 15 builder call sites in the app, plus
   the WebDAV and S3 crates to route through it). The loopback-only part alone (`.no_proxy()` on the local AI clients)
   is XS. Clear win.
3. **PAC and WPAD support.** Use a `reqwest::Proxy::custom` closure that asks CFNetwork per URL:
   `CFNetworkCopyProxiesForURL` over `CFNetworkCopySystemProxySettings`, and, when the answer is an auto-config URL,
   `CFNetworkExecuteProxyAutoConfigurationURL` on a dedicated run-loop thread, with a short per-host cache (the closure
   is sync and runs per request). Size: M (two to three days with a test harness that serves a PAC file). Tradeoff: a
   new piece of FFI to own, for a setup some corporate networks require. An alternative is documenting `HTTPS_PROXY` set
   via a launch agent, which works today but is clumsy for IT.
4. **Proxy authentication** (Basic from the keychain, NTLM/Kerberos): out of scope until someone asks; mention it on
   `/trust` if a reviewer does.
