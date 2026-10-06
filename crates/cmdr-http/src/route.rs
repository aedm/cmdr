//! Where one request goes: straight to its host, or through a proxy. The layers, in order:
//! this Mac's own addresses, then the environment, then the system settings.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use reqwest::Url;
use url::Host;

use crate::env::EnvProxies;

/// The routing verdict for one destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Route {
    Direct,
    /// A proxy URL reqwest can tunnel or forward through: `http://host:port`, with
    /// `user:password@` when the source carried credentials.
    Proxy(String),
}

/// What the operating system says about a URL, once the earlier layers have had no say.
pub(crate) trait SystemProxies: Send + Sync {
    fn route(&self, url: &Url) -> Route;
}

/// The routing decision for `url`.
pub(crate) fn decide(url: &Url, env: &EnvProxies, system: &dyn SystemProxies) -> Route {
    let Some(host) = url.host() else {
        return Route::Direct;
    };
    // ❗ Before the environment and the system: a proxy is another machine, so it can't reach
    // this Mac's loopback or its link-local neighbors. Cmdr's own llama-server lives on
    // `127.0.0.1`, and macOS proxies loopback unless the bypass list says otherwise.
    if is_local(&host) {
        return Route::Direct;
    }
    if let Some(route) = env.route(url, &host) {
        return route;
    }
    system.route(url)
}

/// Loopback (`localhost`, `127.0.0.0/8`, `::1`), link-local (`169.254.0.0/16`, `fe80::/10`),
/// and the unspecified address, which connects to loopback.
pub(crate) fn is_local(host: &Host<&str>) -> bool {
    match host {
        Host::Domain(name) => {
            let name = name.trim_end_matches('.').to_ascii_lowercase();
            name == "localhost" || name.ends_with(".localhost")
        }
        Host::Ipv4(ip) => is_local_v4(*ip),
        Host::Ipv6(ip) => is_local_v6(*ip),
    }
}

fn is_local_v4(ip: Ipv4Addr) -> bool {
    ip.is_loopback() || ip.is_link_local() || ip.is_unspecified()
}

fn is_local_v6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_local_v4(v4);
    }
    ip.is_loopback() || ip.is_unicast_link_local() || ip.is_unspecified()
}

/// `host` as an IP address, when it is one.
pub(crate) fn host_ip(host: &Host<&str>) -> Option<IpAddr> {
    match host {
        Host::Domain(_) => None,
        Host::Ipv4(ip) => Some(IpAddr::V4(*ip)),
        Host::Ipv6(ip) => Some(IpAddr::V6(*ip)),
    }
}

#[cfg(test)]
#[path = "route_test.rs"]
mod tests;
