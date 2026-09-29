//! Log free-form text from outside Cmdr (OS, server, or CLI output) in full, as one field.
//!
//! Print it as `detail={:?}` (or `stderr={:?}` / `stdout={:?}`) with a [`LogDetail`]. The local
//! log keeps the text, capped at [`LOCAL_DETAIL_MAX_BYTES`], because it's often the only window
//! into what a server or tool actually said. The error reporter knows those three keys: a
//! report redacts the value with that report's context and caps it much shorter. That
//! redaction lives in the app's `redact` module, so producers never redact at the log site.
//!
//! The Debug quoting is load-bearing: it keeps the value on one line and gives the report pass
//! an exact end quote, whatever the text contains.

use std::fmt;

/// Local-log cap for one field, in bytes of the raw text.
pub const LOCAL_DETAIL_MAX_BYTES: usize = 1024;

/// External text for one `detail=` / `stderr=` / `stdout=` log field. Its `Debug` prints the
/// text quoted and escaped, cut at [`LOCAL_DETAIL_MAX_BYTES`] on a char boundary with a
/// `…[N bytes]` marker naming the full length.
#[derive(Clone, Copy)]
pub struct LogDetail<'a>(pub &'a str);

impl fmt::Debug for LogDetail<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = self.0.trim();
        if text.len() <= LOCAL_DETAIL_MAX_BYTES {
            return fmt::Debug::fmt(text, f);
        }
        let mut end = LOCAL_DETAIL_MAX_BYTES;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        fmt::Debug::fmt(&format!("{}…[{} bytes]", &text[..end], text.len()), f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_prints_quoted_escaped_and_trimmed() {
        let text = "  tree connect failed: \"Private Share\"\nsecond line\n";
        assert_eq!(
            format!("detail={:?}", LogDetail(text)),
            r#"detail="tree connect failed: \"Private Share\"\nsecond line""#
        );
    }

    #[test]
    fn long_text_is_cut_on_a_char_boundary_with_its_full_length() {
        let text = format!("{}é{}", "a".repeat(LOCAL_DETAIL_MAX_BYTES - 1), "b".repeat(100));
        let printed = format!("{:?}", LogDetail(&text));
        assert!(printed.starts_with(&format!("\"{}", "a".repeat(LOCAL_DETAIL_MAX_BYTES - 1))));
        assert!(printed.ends_with(&format!("…[{} bytes]\"", text.len())), "{printed}");
        assert!(!printed.contains("bb"), "{printed}");
    }
}
