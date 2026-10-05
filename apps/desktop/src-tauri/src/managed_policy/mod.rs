//! Managed preferences: what an organization's MDM profile restricts, read from the forced layer
//! of the `com.veszelovszki.cmdr` preferences domain. Named after the UI phrase "Managed by your
//! organization".
//!
//! Everything downstream asks [`current`] (cached) or [`for_egress`] (fresh, at the point bytes
//! would leave the Mac) and gets typed answers. Nothing outside this module touches CFPreferences
//! or spells a key name. The key catalog, parse rules, and refresh triggers: `DETAILS.md`.

// The policy core lands before its callers: the telemetry, update, and AI gates arrive in M2–M4 of
// `docs/specs/mdm-managed-preferences-plan.md`. ❗ Remove this allow with the last of them, so
// anything still unused then gets flagged.
#![allow(
    dead_code,
    unused_imports,
    reason = "gates that call this module land in later MDM milestones"
)]

mod cache;
mod ceiling;
mod egress;
mod hosts;
pub(crate) mod keys;
mod locked;
mod refusal;
mod source;
pub(crate) mod view;
#[cfg(target_os = "macos")]
mod watch;

pub use cache::{current, for_egress, init, refresh};
pub use ceiling::UpdateCeiling;
pub use egress::Egress;
pub use hosts::HostPattern;
pub use locked::{LockedSetting, LockedValue, SettingLock, locked_settings, overlay};
pub use refusal::{AiDestination, ManagedAiRefusal};
pub use view::{ManagedPolicyChanged, ManagedPolicyView};

/// For tests outside this module: build a policy from forced keys and put it in force on the
/// test's thread.
#[cfg(test)]
pub mod testing {
    pub use super::cache::{PolicyOverride, override_for_test};
    pub use super::keys::*;
    use super::{ManagedPolicy, source::FakeSource};

    /// The policy a profile forcing each key in `forced` to `true` produces. A non-bool key (like
    /// `MaxUpdateVersion`) reads a `true` as unparseable, which is its most restrictive reading.
    pub fn forcing(forced: &[&str]) -> ManagedPolicy {
        let entries: Vec<_> = forced.iter().map(|key| (*key, plist::Value::Boolean(true))).collect();
        parse(&FakeSource::with(&entries)).policy
    }

    /// The policy a profile forcing exactly `entries` produces, for keys that take a value.
    pub fn from_values(entries: &[(&str, plist::Value)]) -> ManagedPolicy {
        parse(&FakeSource::with(entries)).policy
    }
}

use serde::{Deserialize, Serialize};

/// What the organization restricts. `Default` is no restriction. Fields hold what each key said
/// after parsing; the methods below apply precedence, so callers never combine keys themselves.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ManagedPolicy {
    usage_stats_disabled: bool,
    reports_disabled: bool,
    automatic_update_checks_disabled: bool,
    /// Also set by an unreadable `MaxUpdateVersion` (rule 4).
    updates_disabled: bool,
    update_ceiling: Option<UpdateCeiling>,
    ai_disabled: bool,
    cloud_ai_disabled: bool,
    /// `None`: the key is absent. `Some(empty)`: nothing allowed, the same as `cloud_ai_disabled`.
    allowed_cloud_ai_hosts: Option<Vec<HostPattern>>,
}

/// How updates may run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdatePolicy {
    /// No check of any kind, no download, no install.
    Disabled,
    /// Updates run, possibly without background checks and up to a ceiling. The two combine: a
    /// ceiling with manual checks only is the most common IT setup.
    Enabled {
        automatic_checks: bool,
        ceiling: Option<UpdateCeiling>,
    },
}

/// Why the policy refuses an update to a given version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateRefusal {
    /// `DisableUpdates`: no version at all.
    Disabled,
    /// The version is past `MaxUpdateVersion`.
    AboveCeiling(UpdateCeiling),
}

/// How much AI the organization allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum AiPolicy {
    /// Any provider, cloud hosts subject to `AllowedCloudAIHosts`.
    Allowed,
    /// Cmdr's own local model only.
    LocalOnly,
    /// No AI at all.
    Off,
}

impl ManagedPolicy {
    /// Whether any key restricts anything. A key forced to its permissive value doesn't count.
    pub fn is_managed(&self) -> bool {
        *self != Self::default()
    }

    pub fn usage_stats_disabled(&self) -> bool {
        self.usage_stats_disabled
    }

    pub fn reports_disabled(&self) -> bool {
        self.reports_disabled
    }

    /// `DisableUpdates` wins over the other two update keys, which combine with each other.
    pub fn updates(&self) -> UpdatePolicy {
        if self.updates_disabled {
            return UpdatePolicy::Disabled;
        }
        UpdatePolicy::Enabled {
            automatic_checks: !self.automatic_update_checks_disabled,
            ceiling: self.update_ceiling,
        }
    }

    /// The one version decision: may this Mac download or install `version`? `DisableUpdates`
    /// refuses every version; a ceiling refuses one past it, compared on the release core. The
    /// updater asks it when a check finds a release, and again before the download and the install.
    pub fn update_to(&self, version: &semver::Version) -> Result<(), UpdateRefusal> {
        match self.updates() {
            UpdatePolicy::Disabled => Err(UpdateRefusal::Disabled),
            UpdatePolicy::Enabled {
                ceiling: Some(ceiling), ..
            } if !ceiling.allows(version) => Err(UpdateRefusal::AboveCeiling(ceiling)),
            UpdatePolicy::Enabled { .. } => Ok(()),
        }
    }

    /// `DisableAI` > `DisableCloudAI` > `AllowedCloudAIHosts` (an empty list allows no host).
    pub fn ai(&self) -> AiPolicy {
        if self.ai_disabled {
            AiPolicy::Off
        } else if self.cloud_ai_disabled || self.allowed_cloud_ai_hosts.as_ref().is_some_and(Vec::is_empty) {
            AiPolicy::LocalOnly
        } else {
            AiPolicy::Allowed
        }
    }

    /// The cloud hosts AI may reach, when the organization narrowed them. `None` while cloud AI is
    /// off altogether (see [`Self::ai`]) or unrestricted.
    pub fn allowed_cloud_hosts(&self) -> Option<&[HostPattern]> {
        match self.ai() {
            AiPolicy::Allowed => self.allowed_cloud_ai_hosts.as_deref(),
            AiPolicy::LocalOnly | AiPolicy::Off => None,
        }
    }

    /// Whether cloud AI may send to `url`.
    pub fn cloud_host_allowed(&self, url: &url::Url) -> bool {
        self.ai_destination(&AiDestination::Remote(url.clone())).is_ok()
    }

    /// The one AI decision: may a request go to `destination`? `resolve_backend` asks it for the
    /// user-facing reason and the LLM client asks it again per request as the backstop.
    pub fn ai_destination(&self, destination: &AiDestination) -> Result<(), ManagedAiRefusal> {
        match (self.ai(), destination) {
            (AiPolicy::Off, _) => Err(ManagedAiRefusal::AiOff),
            (_, AiDestination::LocalServer) => Ok(()),
            (AiPolicy::LocalOnly, AiDestination::Remote(_)) => Err(ManagedAiRefusal::CloudAiOff),
            (AiPolicy::Allowed, AiDestination::Remote(url)) => match &self.allowed_cloud_ai_hosts {
                Some(hosts) if !hosts.iter().any(|host| host.allows(url)) => Err(ManagedAiRefusal::HostNotAllowed),
                _ => Ok(()),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote(url: &str) -> AiDestination {
        AiDestination::Remote(url::Url::parse(url).expect("a valid test URL"))
    }

    fn hosts(entries: &[&str]) -> Option<Vec<HostPattern>> {
        Some(entries.iter().filter_map(|e| HostPattern::parse(e)).collect())
    }

    #[test]
    fn no_policy_allows_every_destination() {
        let policy = ManagedPolicy::default();
        assert_eq!(policy.ai_destination(&AiDestination::LocalServer), Ok(()));
        assert_eq!(policy.ai_destination(&remote("https://api.openai.com/v1")), Ok(()));
    }

    #[test]
    fn ai_off_refuses_local_and_remote() {
        let policy = ManagedPolicy {
            ai_disabled: true,
            ..Default::default()
        };
        assert_eq!(
            policy.ai_destination(&AiDestination::LocalServer),
            Err(ManagedAiRefusal::AiOff)
        );
        assert_eq!(
            policy.ai_destination(&remote("http://localhost:11434/v1")),
            Err(ManagedAiRefusal::AiOff)
        );
    }

    #[test]
    fn cloud_off_keeps_the_local_server_and_blocks_loopback_endpoints() {
        let policy = ManagedPolicy {
            cloud_ai_disabled: true,
            ..Default::default()
        };
        assert_eq!(policy.ai_destination(&AiDestination::LocalServer), Ok(()));
        assert_eq!(
            policy.ai_destination(&remote("http://localhost:11434/v1")),
            Err(ManagedAiRefusal::CloudAiOff)
        );
    }

    #[test]
    fn a_host_list_allows_only_its_hosts() {
        let policy = ManagedPolicy {
            allowed_cloud_ai_hosts: hosts(&["localhost", "127.0.0.1"]),
            ..Default::default()
        };
        assert_eq!(policy.ai_destination(&remote("http://localhost:11434/v1")), Ok(()));
        assert_eq!(
            policy.ai_destination(&remote("https://api.openai.com/v1")),
            Err(ManagedAiRefusal::HostNotAllowed)
        );
        assert_eq!(policy.ai_destination(&AiDestination::LocalServer), Ok(()));
        assert!(!policy.cloud_host_allowed(&url::Url::parse("https://api.openai.com/").expect("url")));
    }

    fn version(text: &str) -> semver::Version {
        semver::Version::parse(text).expect("a valid test version")
    }

    #[test]
    fn no_policy_allows_an_update_to_any_version() {
        assert_eq!(ManagedPolicy::default().update_to(&version("99.0.0")), Ok(()));
    }

    #[test]
    fn updates_off_allows_no_version() {
        let policy = ManagedPolicy {
            updates_disabled: true,
            ..Default::default()
        };
        assert_eq!(policy.update_to(&version("0.0.1")), Err(UpdateRefusal::Disabled));
    }

    #[test]
    fn a_ceiling_allows_up_to_itself_and_no_further() {
        let policy = ManagedPolicy {
            update_ceiling: Some(UpdateCeiling::Minor(0, 52)),
            automatic_update_checks_disabled: true,
            ..Default::default()
        };
        let held = Err(UpdateRefusal::AboveCeiling(UpdateCeiling::Minor(0, 52)));
        assert_eq!(policy.update_to(&version("0.52.9")), Ok(()));
        assert_eq!(policy.update_to(&version("0.53.0")), held);
        assert_eq!(policy.update_to(&version("0.53.0-rc.1")), held);
    }

    #[test]
    fn an_empty_host_list_is_cloud_off() {
        let policy = ManagedPolicy {
            allowed_cloud_ai_hosts: Some(vec![]),
            ..Default::default()
        };
        assert_eq!(policy.ai(), AiPolicy::LocalOnly);
        assert_eq!(
            policy.ai_destination(&remote("https://api.openai.com/v1")),
            Err(ManagedAiRefusal::CloudAiOff)
        );
        assert_eq!(policy.allowed_cloud_hosts(), None);
    }
}
