# cmdr-http details

## Why a crate

Every HTTP client in Cmdr (the app's api-server calls, the updater, AI, model downloads, and the WebDAV and S3 backends)
needs the same proxy behavior, and the backend crates can't depend on the app. One crate both sides depend on keeps the
routing in one place, and `clippy.toml`'s `disallowed-methods` makes it the only way in. Evidence that motivated it:
`docs/notes/proxy-and-tls-inspection-2026-10.md`.

## The decision, per destination

reqwest calls the `Proxy::custom` closure with `scheme://host[:port]` once per new connection and, for a plain-HTTP
request, once per request (to decide on proxy auth). `route::decide` answers:

1. **No host** → direct.
2. **Local host** → direct. `localhost` and `*.localhost`, `127.0.0.0/8`, `::1`, `169.254.0.0/16`, `fe80::/10`,
   `0.0.0.0` / `::`, and the IPv4-mapped forms of those. Not `*.local` and not private ranges: those are LAN hosts a
   corporate proxy may legitimately be asked to skip through the bypass list, which macOS lists by default (`*.local`,
   `169.254/16`).
3. **`NO_PROXY` matches** → direct, even when the system would proxy. Same as reqwest (hyper-util's matcher applies `no`
   to every proxy). Entries: `*`, domains (`corp.example`, `.corp.example`, `*.corp.example` all cover the domain and
   its subdomains), IPs, CIDR blocks; a `:port` suffix is ignored.
4. **`HTTPS_PROXY` / `HTTP_PROXY`** for the URL's scheme, else `ALL_PROXY` → that proxy. Uppercase wins over lowercase.
   A value without a scheme gets `http://`. Credentials in the URL are kept; reqwest's matcher turns them into a
   `Proxy-Authorization` header.
5. **macOS** (`system::mac::route`) → CFNetwork's verdict. Elsewhere there's no system layer and this step is direct.

The environment is snapshotted on the first `client_builder()` call (`OnceLock`): a running process's environment
doesn't change.

## The macOS answer

`CFNetworkCopySystemProxySettings()` returns the live settings (cheap: CFNetwork caches them, 76 ns a call) and
`CFNetworkCopyProxiesForURL(url, settings)` returns an ordered list of dictionaries, each with a `kCFProxyTypeKey`.
`first_route` walks it:

- `kCFProxyTypeNone` → direct.
- `kCFProxyTypeHTTP` / `kCFProxyTypeHTTPS` → `http://host:port` (the HTTPS type is an HTTP proxy that tunnels with
  `CONNECT`, so the proxy URL's scheme stays `http`). Username and password are embedded when CFNetwork supplies both.
- SOCKS, FTP, or an entry missing its host → skipped, logged at debug; the next entry decides.
- An exhausted list → direct.

When a PAC file is configured, the list is `[AutoConfigurationURL, None]`: CFNetwork appends its own DIRECT fallback
(verified on macOS 27.0, `system_test.rs::a_pac_setting_surfaces_as_its_url`, 2026-10-06).

Cost: about 15 µs per lookup in a release build, almost all of it `CFNetworkCopyProxiesForURL` (macOS 27.0, M3 MacBook
Pro, 1,000 lookups of `https://api.getcmdr.com`, 2026-10-06). That's noise next to a connection, so there's no cache.

## Testing

- `route_test.rs` / `env_test.rs`: the pure decision, with the environment injected (`EnvProxies::from_vars`) and a fake
  system layer. ❌ Never `std::env::set_var` in a test: the runner is parallel.
- `system_test.rs`: `first_route` everywhere, plus the real CFNetwork call on macOS against a settings dictionary the
  test builds (`mac::settings`), so it never reads or changes this Mac's own settings.
- `lib_test.rs`: the built client end to end, with a fake proxy and origin on loopback.
