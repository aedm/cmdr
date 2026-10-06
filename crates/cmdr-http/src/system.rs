//! The operating system's proxy verdict for one URL.
//!
//! On macOS that's CFNetwork's own answer (`CFNetworkCopyProxiesForURL` over
//! `CFNetworkCopySystemProxySettings`), so the bypass list and "Exclude simple hostnames" match
//! exactly the way Safari and every other CFNetwork client match them. Elsewhere there's no system
//! layer: the environment variables are the whole configuration.

use reqwest::Url;

use crate::route::{Route, SystemProxies};

/// The system layer [`crate::client_builder`] uses.
pub(crate) struct MacSystem;

impl SystemProxies for MacSystem {
    fn route(&self, url: &Url) -> Route {
        #[cfg(target_os = "macos")]
        {
            mac::route(url)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = url;
            Route::Direct
        }
    }
}

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

/// The first entry Cmdr can act on, the way CFNetwork clients walk the list: an unusable entry
/// is skipped, and an empty or exhausted list means direct.
pub(crate) fn first_route(entries: &[Entry]) -> Route {
    for entry in entries {
        match entry {
            Entry::Direct => return Route::Direct,
            Entry::Http {
                host,
                port,
                credentials,
            } => return Route::Proxy(proxy_url(host, *port, credentials.as_ref())),
            Entry::AutoConfigUrl(_) | Entry::AutoConfigScript(_) => {}
            Entry::Unsupported(kind) => {
                log::debug!(target: "cmdr_http", "skipping a {kind} proxy: Cmdr speaks HTTP proxies only");
            }
        }
    }
    Route::Direct
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

#[cfg(target_os = "macos")]
pub(crate) mod mac {
    use objc2_cf_network::{
        CFNetworkCopyProxiesForURL, CFNetworkCopySystemProxySettings, kCFProxyAutoConfigurationJavaScriptKey,
        kCFProxyAutoConfigurationURLKey, kCFProxyHostNameKey, kCFProxyPasswordKey, kCFProxyPortNumberKey,
        kCFProxyTypeAutoConfigurationJavaScript, kCFProxyTypeAutoConfigurationURL, kCFProxyTypeHTTP, kCFProxyTypeHTTPS,
        kCFProxyTypeKey, kCFProxyTypeNone, kCFProxyUsernameKey,
    };
    use objc2_core_foundation::{CFArray, CFDictionary, CFNumber, CFString, CFType, CFURL};
    use reqwest::Url;

    use super::{Entry, first_route};
    use crate::route::Route;

    /// The current system settings' verdict for `url`.
    pub(crate) fn route(url: &Url) -> Route {
        // SAFETY: no arguments; the returned dictionary is owned (`Copy` rule) and wrapped in a
        // `CFRetained` by the binding.
        let Some(settings) = (unsafe { CFNetworkCopySystemProxySettings() }) else {
            return Route::Direct;
        };
        first_route(&entries_for(&settings, url))
    }

    /// CFNetwork's ordered answer for `url` under `settings`. Split out so a test can pass its own
    /// settings dictionary instead of the Mac's.
    pub(crate) fn entries_for(settings: &CFDictionary, url: &Url) -> Vec<Entry> {
        let Some(target) = CFURL::from_string(None, &CFString::from_str(url.as_str()), None) else {
            return Vec::new();
        };
        // SAFETY: `settings` is a proxy-settings dictionary (string keys, CF values), which is the
        // only "generics" requirement the binding names; `target` is a valid CFURL.
        let list = unsafe { CFNetworkCopyProxiesForURL(&target, settings) };
        // SAFETY: CFNetwork documents every element of this array as a CFDictionary keyed by the
        // `kCFProxy*Key` strings.
        let list: &CFArray<CFDictionary<CFString, CFType>> = unsafe { list.cast_unchecked() };
        list.iter().map(|entry| entry_from(&entry)).collect()
    }

    /// CFNetwork's key and type-name constants, read once per lookup.
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

    /// A proxy-settings dictionary for a test, in place of the Mac's own.
    #[cfg(test)]
    pub(crate) fn settings(pairs: &[(&CFString, &CFType)]) -> objc2_core_foundation::CFRetained<CFDictionary> {
        let keys: Vec<&CFString> = pairs.iter().map(|(key, _)| *key).collect();
        let values: Vec<&CFType> = pairs.iter().map(|(_, value)| *value).collect();
        let typed = CFDictionary::<CFString, CFType>::from_slices(&keys, &values);
        // SAFETY: a typed dictionary is the same CF object as its untyped view.
        unsafe { objc2_core_foundation::CFRetained::cast_unchecked(typed) }
    }
}

#[cfg(test)]
#[path = "system_test.rs"]
mod tests;
