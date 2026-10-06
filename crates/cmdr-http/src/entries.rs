//! CFNetwork's ordered proxy list, as Cmdr reads and walks it. The list comes from the system
//! settings (`system.rs`) or from a PAC file (`pac.rs`), in the same shape either way.
#![cfg_attr(
    not(target_os = "macos"),
    allow(
        dead_code,
        reason = "the list walk serves macOS's answer; elsewhere only its tests use it"
    )
)]

use reqwest::Url;

use crate::Route;

/// One entry of the ordered list CFNetwork answers with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Entry {
    Direct,
    /// An HTTP proxy; for an `https` URL, one that tunnels with `CONNECT`.
    Http {
        host: String,
        port: u16,
        credentials: Option<(String, String)>,
    },
    /// A PAC file to evaluate for this URL.
    AutoConfigUrl(String),
    /// A PAC script given inline (`ProxyAutoConfigJavaScript`).
    AutoConfigScript(String),
    /// A proxy type Cmdr can't speak (SOCKS, FTP), named for the log.
    Unsupported(String),
}

/// Where a PAC script comes from.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum PacSource {
    Url(String),
    Script(String),
}

/// Runs a PAC script for one URL. `None` when it couldn't (unreachable, broken, too slow): the
/// walk then moves on to the next entry, which is CFNetwork's own DIRECT fallback.
pub(crate) trait Pac {
    fn evaluate(&self, source: &PacSource, url: &Url) -> Option<Vec<Entry>>;
}

/// The first entry Cmdr can act on, the way CFNetwork clients walk the list: an unusable entry
/// is skipped, a PAC entry stands for the PAC's own answer, and an empty or exhausted list means
/// direct.
pub(crate) fn first_route(entries: &[Entry], url: &Url, pac: &dyn Pac) -> Route {
    for entry in entries {
        let source = match entry {
            Entry::AutoConfigUrl(pac_url) => PacSource::Url(pac_url.clone()),
            Entry::AutoConfigScript(script) => PacSource::Script(script.clone()),
            other => match usable(other) {
                Some(route) => return route,
                None => continue,
            },
        };
        // A PAC's answer never names another PAC, so its list is walked without one.
        if let Some(route) = pac
            .evaluate(&source, url)
            .and_then(|answer| answer.iter().find_map(usable))
        {
            return route;
        }
    }
    Route::Direct
}

/// The route one entry gives on its own, or `None` when Cmdr can't use it.
fn usable(entry: &Entry) -> Option<Route> {
    match entry {
        Entry::Direct => Some(Route::Direct),
        Entry::Http {
            host,
            port,
            credentials,
        } => Some(Route::Proxy(proxy_url(host, *port, credentials.as_ref()))),
        Entry::AutoConfigUrl(_) | Entry::AutoConfigScript(_) => None,
        Entry::Unsupported(kind) => {
            log::debug!(target: "cmdr_http", "skipping a {kind} proxy: Cmdr speaks HTTP proxies only");
            None
        }
    }
}

fn proxy_url(host: &str, port: u16, credentials: Option<&(String, String)>) -> String {
    let host = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    match credentials {
        Some((user, password)) => {
            let encode = |part: &str| url::form_urlencoded::byte_serialize(part.as_bytes()).collect::<String>();
            format!("http://{}:{}@{host}:{port}", encode(user), encode(password))
        }
        None => format!("http://{host}:{port}"),
    }
}

/// Reading CFNetwork's CF objects into [`Entry`]s.
#[cfg(target_os = "macos")]
pub(crate) mod cf {
    use objc2_cf_network::{
        kCFProxyAutoConfigurationJavaScriptKey, kCFProxyAutoConfigurationURLKey, kCFProxyHostNameKey,
        kCFProxyPasswordKey, kCFProxyPortNumberKey, kCFProxyTypeAutoConfigurationJavaScript,
        kCFProxyTypeAutoConfigurationURL, kCFProxyTypeHTTP, kCFProxyTypeHTTPS, kCFProxyTypeKey, kCFProxyTypeNone,
        kCFProxyUsernameKey,
    };
    use objc2_core_foundation::{CFArray, CFDictionary, CFNumber, CFRetained, CFString, CFType, CFURL};

    use super::Entry;

    pub(crate) fn cf_url(url: &str) -> Option<CFRetained<CFURL>> {
        CFURL::from_string(None, &CFString::from_str(url), None)
    }

    /// Reads a CFNetwork proxy list: the answer to `CFNetworkCopyProxiesForURL`, or a PAC's.
    pub(crate) fn entries_from(list: &CFArray) -> Vec<Entry> {
        // SAFETY: CFNetwork documents every element of a proxy list as a CFDictionary keyed by the
        // `kCFProxy*Key` strings.
        let list: &CFArray<CFDictionary<CFString, CFType>> = unsafe { list.cast_unchecked() };
        list.iter().map(|entry| entry_from(&entry)).collect()
    }

    /// CFNetwork's key and type-name constants, read once per entry.
    struct Names {
        type_key: &'static CFString,
        host: &'static CFString,
        port: &'static CFString,
        user: &'static CFString,
        password: &'static CFString,
        pac_url: &'static CFString,
        pac_script: &'static CFString,
        none: &'static CFString,
        http: &'static CFString,
        https: &'static CFString,
        auto_config_url: &'static CFString,
        auto_config_script: &'static CFString,
    }

    fn names() -> Names {
        // SAFETY: CFNetwork exports these as immutable CFString constants that live for the whole
        // process; reading one is a plain load of a pointer nothing ever writes.
        unsafe {
            Names {
                type_key: kCFProxyTypeKey,
                host: kCFProxyHostNameKey,
                port: kCFProxyPortNumberKey,
                user: kCFProxyUsernameKey,
                password: kCFProxyPasswordKey,
                pac_url: kCFProxyAutoConfigurationURLKey,
                pac_script: kCFProxyAutoConfigurationJavaScriptKey,
                none: kCFProxyTypeNone,
                http: kCFProxyTypeHTTP,
                https: kCFProxyTypeHTTPS,
                auto_config_url: kCFProxyTypeAutoConfigurationURL,
                auto_config_script: kCFProxyTypeAutoConfigurationJavaScript,
            }
        }
    }

    fn entry_from(entry: &CFDictionary<CFString, CFType>) -> Entry {
        let names = names();
        let string = |key: &CFString| {
            entry
                .get(key)
                .and_then(|value| value.downcast::<CFString>().ok())
                .map(|value| value.to_string())
        };
        let kind = string(names.type_key).unwrap_or_default();
        let is = |constant: &CFString| kind == constant.to_string();
        if is(names.none) {
            Entry::Direct
        } else if is(names.http) || is(names.https) {
            let port = entry
                .get(names.port)
                .and_then(|value| value.downcast::<CFNumber>().ok())
                .and_then(|value| value.as_i64())
                .and_then(|value| u16::try_from(value).ok())
                .unwrap_or(80);
            let credentials = string(names.user).zip(string(names.password));
            match string(names.host) {
                Some(host) => Entry::Http {
                    host,
                    port,
                    credentials,
                },
                None => Entry::Unsupported(String::from("host-less HTTP")),
            }
        } else if is(names.auto_config_url) {
            let pac = entry
                .get(names.pac_url)
                .and_then(|value| value.downcast::<CFURL>().ok())
                .map(|value| value.string().to_string());
            pac.map_or_else(
                || Entry::Unsupported(String::from("URL-less PAC")),
                Entry::AutoConfigUrl,
            )
        } else if is(names.auto_config_script) {
            string(names.pac_script).map_or_else(
                || Entry::Unsupported(String::from("script-less PAC")),
                Entry::AutoConfigScript,
            )
        } else {
            Entry::Unsupported(kind)
        }
    }
}

#[cfg(test)]
#[path = "entries_test.rs"]
mod tests;
