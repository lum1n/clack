use unicode_segmentation::UnicodeSegmentation;

/// How to turn `previous` text already in a field into `next`.
///
/// Counts user-perceived characters (Unicode grapheme clusters), not bytes
/// and not scalar values. A family emoji or a combining accent is one cluster.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextRevision {
    pub backspace_graphemes: usize,
    pub suffix: String,
}

/// Common grapheme prefix of `previous` and `next`.
///
/// The caller backspaces `backspace_graphemes` clusters, then types `suffix`.
pub fn revise_partial(previous: &str, next: &str) -> TextRevision {
    let prev: Vec<&str> = previous.graphemes(true).collect();
    let nxt: Vec<&str> = next.graphemes(true).collect();
    let mut shared = 0;
    while shared < prev.len() && shared < nxt.len() && prev[shared] == nxt[shared] {
        shared += 1;
    }
    TextRevision {
        backspace_graphemes: prev.len() - shared,
        suffix: nxt[shared..].concat(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_text_is_a_noop() {
        let rev = revise_partial("hello", "hello");
        assert_eq!(rev.backspace_graphemes, 0);
        assert_eq!(rev.suffix, "");
    }

    #[test]
    fn append_only_types_the_new_suffix() {
        let rev = revise_partial("hello wor", "hello world");
        assert_eq!(rev.backspace_graphemes, 0);
        assert_eq!(rev.suffix, "ld");
    }

    #[test]
    fn unstable_suffix_is_backspaced_then_replaced() {
        let rev = revise_partial("hello world", "hello word");
        assert_eq!(rev.backspace_graphemes, 2);
        assert_eq!(rev.suffix, "d");
    }

    #[test]
    fn full_replace_clears_every_grapheme() {
        let rev = revise_partial("abc", "xyz");
        assert_eq!(rev.backspace_graphemes, 3);
        assert_eq!(rev.suffix, "xyz");
    }

    #[test]
    fn empty_previous_types_everything() {
        let rev = revise_partial("", "hi");
        assert_eq!(rev.backspace_graphemes, 0);
        assert_eq!(rev.suffix, "hi");
    }

    #[test]
    fn empty_next_backspaces_everything() {
        let rev = revise_partial("hi", "");
        assert_eq!(rev.backspace_graphemes, 2);
        assert_eq!(rev.suffix, "");
    }

    #[test]
    fn counts_graphemes_not_bytes() {
        // é is U+00E9: 2 bytes, 1 scalar, 1 grapheme.
        let rev = revise_partial("é", "éa");
        assert_eq!("é".len(), 2);
        assert_eq!(rev.backspace_graphemes, 0);
        assert_eq!(rev.suffix, "a");

        let cleared = revise_partial("éé", "");
        assert_eq!(cleared.backspace_graphemes, 2);
    }

    #[test]
    fn combining_mark_is_one_grapheme() {
        let composed = "é"; // U+00E9
        let combining = "e\u{0301}";
        assert_eq!(combining.chars().count(), 2);
        assert_eq!(combining.graphemes(true).count(), 1);
        // They are different clusters, so the whole cluster is replaced.
        let rev = revise_partial(combining, composed);
        assert_eq!(rev.backspace_graphemes, 1);
        assert_eq!(rev.suffix, composed);

        let prefix = revise_partial(&format!("caf{combining}"), "cafe");
        assert_eq!(prefix.backspace_graphemes, 1);
        assert_eq!(prefix.suffix, "e");
    }

    #[test]
    fn zwj_family_emoji_is_one_grapheme() {
        let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466}";
        assert!(family.chars().count() > 1);
        assert_eq!(family.graphemes(true).count(), 1);

        let previous = format!("a{family}b");
        let next = format!("a{family}c");
        let rev = revise_partial(&previous, &next);
        assert_eq!(rev.backspace_graphemes, 1);
        assert_eq!(rev.suffix, "c");

        let grown = revise_partial(family, &format!("{family}!"));
        assert_eq!(grown.backspace_graphemes, 0);
        assert_eq!(grown.suffix, "!");
    }
}
