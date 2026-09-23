//! The one key two names share when they differ only in Unicode form or case.
//!
//! NFC, then lowercase. macOS holds `é` composed and decomposed side by side,
//! the kernel's SMB mount decomposes every name it lists, and an SMB server
//! matches a name's exact bytes, so "is this the same name?" needs one answer
//! everywhere Cmdr asks it: a share's identity (`volume::ids::smb_volume_id`), a
//! transfer's destination buckets, the SMB spelling resolve, and a pane landing
//! its cursor on a name spelled another way.
//!
//! ❌ A comparison key only, never sent to a volume: the server stores the bytes
//! it was given, and one directory can hold both forms.

use std::borrow::Cow;

use unicode_normalization::UnicodeNormalization;

/// `name` folded for comparison. Borrows when the name is already its own key
/// (lowercase ASCII, the overwhelmingly common case), so a hot loop over a big
/// listing allocates nothing for it.
pub fn fold_name(name: &str) -> Cow<'_, str> {
    if name.is_ascii() {
        if name.bytes().any(|b| b.is_ascii_uppercase()) {
            return Cow::Owned(name.to_ascii_lowercase());
        }
        return Cow::Borrowed(name);
    }
    Cow::Owned(name.nfc().flat_map(char::to_lowercase).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowercase_ascii_is_its_own_key() {
        assert!(matches!(fold_name("readme.txt"), Cow::Borrowed("readme.txt")));
        assert_eq!(fold_name("README.txt"), "readme.txt");
    }

    #[test]
    fn both_unicode_forms_and_both_cases_share_one_key() {
        let composed = "R\u{e9}sz.jpg";
        let decomposed = "re\u{301}sz.jpg";
        assert_eq!(fold_name(composed), fold_name(decomposed));
        assert_eq!(fold_name(decomposed), "r\u{e9}sz.jpg");
    }

    #[test]
    fn folding_ascii_and_folding_through_the_normalizer_agree() {
        // The ASCII fast path is an optimization, not a second rule.
        for name in ["Notes.TXT", "a-b_c 1.txt", "UPPER", ""] {
            assert_eq!(
                fold_name(name),
                name.nfc().flat_map(char::to_lowercase).collect::<String>(),
                "the ASCII shortcut must answer what the general path answers"
            );
        }
    }
}
