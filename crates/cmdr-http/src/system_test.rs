use super::*;

fn http(host: &str, port: u16) -> Entry {
    Entry::Http {
        host: host.to_string(),
        port,
        credentials: None,
    }
}

#[test]
fn the_first_usable_entry_wins() {
    assert_eq!(
        first_route(&[http("proxy.test", 3128), Entry::Direct]),
        Route::Proxy("http://proxy.test:3128".into())
    );
    assert_eq!(first_route(&[Entry::Direct, http("proxy.test", 3128)]), Route::Direct);
}

#[test]
fn an_entry_cmdr_cant_speak_is_skipped() {
    let entries = [Entry::Unsupported("kCFProxyTypeSOCKS".into()), http("proxy.test", 8080)];
    assert_eq!(first_route(&entries), Route::Proxy("http://proxy.test:8080".into()));
}

#[test]
fn an_empty_or_exhausted_list_means_direct() {
    assert_eq!(first_route(&[]), Route::Direct);
    assert_eq!(
        first_route(&[Entry::Unsupported("kCFProxyTypeSOCKS".into())]),
        Route::Direct
    );
}

#[test]
fn credentials_and_ipv6_hosts_make_a_valid_proxy_url() {
    let entry = Entry::Http {
        host: "fd00::1".into(),
        port: 3128,
        credentials: Some(("ada".into(), "p@ss word".into())),
    };
    assert_eq!(
        first_route(&[entry]),
        Route::Proxy("http://ada:p%40ss+word@[fd00::1]:3128".into())
    );
}

#[cfg(target_os = "macos")]
mod mac_settings {
    use objc2_cf_network::{
        kCFNetworkProxiesExceptionsList, kCFNetworkProxiesExcludeSimpleHostnames, kCFNetworkProxiesHTTPEnable,
        kCFNetworkProxiesHTTPPort, kCFNetworkProxiesHTTPProxy, kCFNetworkProxiesHTTPSEnable,
        kCFNetworkProxiesHTTPSPort, kCFNetworkProxiesHTTPSProxy, kCFNetworkProxiesProxyAutoConfigEnable,
        kCFNetworkProxiesProxyAutoConfigURLString,
    };
    use objc2_core_foundation::{CFArray, CFDictionary, CFNumber, CFRetained, CFString};
    use reqwest::Url;

    use super::super::mac::{entries_for, settings};
    use super::super::{Entry, first_route};
    use crate::route::Route;

    /// A Mac with a manual HTTP and HTTPS proxy, a bypass list, and "Exclude simple hostnames" on.
    fn static_proxy_settings() -> CFRetained<CFDictionary> {
        let on = CFNumber::new_i32(1);
        let port = CFNumber::new_i32(3128);
        let host = CFString::from_str("proxy.test");
        let exceptions = CFArray::<CFString>::from_retained_objects(&[
            CFString::from_str("*.corp.test"),
            CFString::from_str("getcmdr.com"),
        ]);
        // SAFETY: CFNetwork's key constants are immutable, process-lifetime CFStrings.
        let keys = unsafe {
            [
                kCFNetworkProxiesHTTPEnable,
                kCFNetworkProxiesHTTPProxy,
                kCFNetworkProxiesHTTPPort,
                kCFNetworkProxiesHTTPSEnable,
                kCFNetworkProxiesHTTPSProxy,
                kCFNetworkProxiesHTTPSPort,
                kCFNetworkProxiesExceptionsList,
                kCFNetworkProxiesExcludeSimpleHostnames,
            ]
        };
        settings(&[
            (keys[0], &on),
            (keys[1], &host),
            (keys[2], &port),
            (keys[3], &on),
            (keys[4], &host),
            (keys[5], &port),
            (keys[6], &exceptions),
            (keys[7], &on),
        ])
    }

    fn route(settings: &CFDictionary, url: &str) -> Route {
        first_route(&entries_for(settings, &Url::parse(url).expect("a valid test URL")))
    }

    #[test]
    fn a_manual_proxy_carries_ordinary_hosts() {
        let settings = static_proxy_settings();
        assert_eq!(
            route(&settings, "https://example.com"),
            Route::Proxy("http://proxy.test:3128".into())
        );
        assert_eq!(
            route(&settings, "http://example.com"),
            Route::Proxy("http://proxy.test:3128".into())
        );
    }

    #[test]
    fn the_bypass_list_sends_its_hosts_direct() {
        let settings = static_proxy_settings();
        assert_eq!(route(&settings, "https://getcmdr.com"), Route::Direct);
        assert_eq!(route(&settings, "https://files.corp.test"), Route::Direct);
    }

    #[test]
    fn exclude_simple_hostnames_sends_dotless_hosts_direct() {
        assert_eq!(route(&static_proxy_settings(), "http://intranet"), Route::Direct);
    }

    #[test]
    fn no_proxy_settings_mean_direct() {
        assert_eq!(route(&settings(&[]), "https://example.com"), Route::Direct);
    }

    #[test]
    fn a_pac_setting_surfaces_as_its_url() {
        let on = CFNumber::new_i32(1);
        let pac = CFString::from_str("http://127.0.0.1:1/proxy.pac");
        // SAFETY: CFNetwork's key constants are immutable, process-lifetime CFStrings.
        let (enable, url) = unsafe {
            (
                kCFNetworkProxiesProxyAutoConfigEnable,
                kCFNetworkProxiesProxyAutoConfigURLString,
            )
        };
        let settings = settings(&[(enable, &on), (url, &pac)]);
        let entries = entries_for(&settings, &Url::parse("https://example.com").expect("a valid test URL"));
        // CFNetwork appends its own DIRECT fallback after the PAC entry (verified on macOS 27.0).
        assert_eq!(
            entries,
            vec![
                Entry::AutoConfigUrl("http://127.0.0.1:1/proxy.pac".into()),
                Entry::Direct
            ]
        );
    }
}
