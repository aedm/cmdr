use super::names::{split_cmdr_suffix, unescape_debug};
use rand::RngExt;
use sha2::{Digest, Sha256};
use std::sync::LazyLock;
use unicode_normalization::UnicodeNormalization;

const CONTEXT_KEY_LABEL: &[u8] = b"cmdr-redaction-context-v1";
const TOKEN_LABEL: &[u8] = b"cmdr-redaction-token-v1";

static PROCESS_SECRET: LazyLock<[u8; 32]> = LazyLock::new(|| rand::rng().random());

/// Report-scoped identity for correlated redaction tokens.
///
/// A context derives its key from an ephemeral process secret and the report ID. Rebuilding
/// one report in the same process therefore reproduces its tokens, while another report or
/// process cannot correlate them.
pub struct RedactionContext {
    key: [u8; 32],
}

#[derive(Clone, Copy, Debug)]
pub(super) enum TokenDomain {
    Path,
    Host,
    Userinfo,
    Volume,
    Server,
    Device,
}

impl TokenDomain {
    fn tag(self) -> &'static [u8] {
        match self {
            Self::Path => b"path",
            Self::Host => b"host",
            Self::Userinfo => b"userinfo",
            Self::Volume => b"volume",
            Self::Server => b"server",
            Self::Device => b"device",
        }
    }
}

impl RedactionContext {
    /// Build the context for one report. The report ID is not secret; the process secret is
    /// what prevents dictionary attacks and correlation across app launches.
    pub fn for_report(report_id: &str) -> Self {
        Self::from_secret(*PROCESS_SECRET, report_id)
    }

    #[cfg(test)]
    pub(crate) fn for_test(process_secret: [u8; 32], report_id: &str) -> Self {
        Self::from_secret(process_secret, report_id)
    }

    fn from_secret(process_secret: [u8; 32], report_id: &str) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(CONTEXT_KEY_LABEL);
        hasher.update([0]);
        hasher.update(process_secret);
        hasher.update([0]);
        hasher.update(report_id.as_bytes());
        Self {
            key: hasher.finalize().into(),
        }
    }

    pub(super) fn token(&self, domain: TokenDomain, value: &str) -> String {
        let name = match domain {
            TokenDomain::Path => split_cmdr_suffix(value).0,
            _ => value,
        };
        let normalized: String = unescape_debug(name).nfc().collect();
        let mut hasher = Sha256::new();
        hasher.update(TOKEN_LABEL);
        hasher.update([0]);
        hasher.update(self.key);
        hasher.update([0]);
        hasher.update(domain.tag());
        hasher.update([0]);
        hasher.update(normalized.as_bytes());
        let bytes = hasher.finalize();
        format!(
            "{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5]
        )
    }
}
