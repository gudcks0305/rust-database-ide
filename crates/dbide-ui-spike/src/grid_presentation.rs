//! Bounded, single-line grid previews. The owning result remains unchanged for
//! inspection, copying and export; these strings must never replace raw values.

pub const DEFAULT_CELL_PREVIEW_BYTES: usize = 256;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CellPresentation {
    pub display: String,
    pub accessibility_value: String,
    pub clipped: bool,
    /// `None` denotes SQL NULL; an empty text value has `Some(0)`.
    pub original_bytes: Option<usize>,
}

pub fn present_cell(value: Option<&str>) -> CellPresentation {
    present_cell_with_limit(value, DEFAULT_CELL_PREVIEW_BYTES)
}

/// Limit the escaped text payload to `max_bytes`, plus a three-byte ellipsis
/// when clipped. SQL NULL uses a fixed label; accessibility adds fixed semantic
/// labels and the original byte length. Work and allocation are O(max_bytes),
/// independent of the raw value length. No grapheme segmentation is performed.
pub fn present_cell_with_limit(value: Option<&str>, max_bytes: usize) -> CellPresentation {
    let Some(value) = value else {
        return CellPresentation {
            display: "NULL".into(),
            accessibility_value: "NULL (database null)".into(),
            clipped: false,
            original_bytes: None,
        };
    };
    let (display, clipped) = bounded_preview(value, max_bytes);
    let accessibility_value = if value.is_empty() {
        "Empty text".into()
    } else if clipped {
        format!("Text: {display} (preview truncated; {} bytes)", value.len())
    } else {
        format!("Text: {display}")
    };
    CellPresentation {
        display,
        accessibility_value,
        clipped,
        original_bytes: Some(value.len()),
    }
}

/// Apply the same bounded single-line presentation to untrusted column labels.
pub fn bounded_label(value: &str, max_bytes: usize) -> String {
    bounded_preview(value, max_bytes).0
}

fn bounded_preview(value: &str, max_bytes: usize) -> (String, bool) {
    // `len` is O(1); reserve neither the full value nor an untrusted huge cap.
    let mut display = String::with_capacity(value.len().min(max_bytes).min(4096));
    let mut consumed = 0;
    for ch in value.chars() {
        let mut encoded = [0; 4];
        let mut escaped = [0; 10];
        let representation = match ch {
            '\\' => "\\\\",
            '\n' => "\\n",
            '\r' => "\\r",
            '\t' => "\\t",
            '\u{2028}' => "\\u{2028}",
            '\u{2029}' => "\\u{2029}",
            ch if ch.is_control() => {
                let mut length = 0;
                for escaped_ch in ch.escape_default() {
                    escaped[length] = escaped_ch as u8;
                    length += 1;
                }
                // escape_default produces ASCII for control characters.
                std::str::from_utf8(&escaped[..length]).expect("ASCII control escape")
            }
            ch => ch.encode_utf8(&mut encoded),
        };
        if representation.len() > max_bytes.saturating_sub(display.len()) {
            break;
        }
        display.push_str(representation);
        consumed += ch.len_utf8();
        // Do not decode another source character after exhausting the budget.
        if display.len() == max_bytes {
            break;
        }
    }
    let clipped = consumed < value.len();
    if clipped {
        display.push('…');
    }
    (display, clipped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_empty_and_literal_null_have_distinct_semantics() {
        let null = present_cell(None);
        let empty = present_cell(Some(""));
        let literal = present_cell(Some("NULL"));
        assert_eq!(null.display, "NULL");
        assert_eq!(empty.display, "");
        assert_eq!(literal.display, "NULL");
        assert_eq!(null.original_bytes, None);
        assert_eq!(empty.original_bytes, Some(0));
        assert_eq!(literal.original_bytes, Some(4));
        assert_eq!(null.accessibility_value, "NULL (database null)");
        assert_eq!(empty.accessibility_value, "Empty text");
        assert_eq!(literal.accessibility_value, "Text: NULL");
        assert!(!null.clipped && !empty.clipped && !literal.clipped);
    }

    #[test]
    fn utf8_boundaries_never_split_encoded_characters() {
        for (value, limit, expected) in [
            ("서울한글", 5, "서…"),
            ("서울", 6, "서울"),
            ("🙂🙂", 7, "🙂…"),
            ("🙂", 3, "…"),
            ("e\u{301}x", 3, "e\u{301}…"),
            ("e\u{301}", 1, "e…"),
        ] {
            let preview = present_cell_with_limit(Some(value), limit);
            assert_eq!(preview.display, expected);
            assert!(preview.display.len() <= limit + '…'.len_utf8());
        }
    }

    #[test]
    fn control_characters_are_single_line_and_escapes_are_unambiguous() {
        let raw = "a\n\r\t\\n\0\u{7f}\u{2028}\u{2029}";
        let preview = present_cell(Some(raw));
        assert_eq!(
            preview.display,
            "a\\n\\r\\t\\\\n\\u{0}\\u{7f}\\u{2028}\\u{2029}"
        );
        assert!(!preview.display.chars().any(char::is_control));
        assert!(!preview.clipped);
        assert_eq!(preview.original_bytes, Some(raw.len()));
        assert_eq!(bounded_label(raw, 3), "a\\n…");
    }

    #[test]
    fn cap_applies_after_escaping_without_partial_escape_sequences() {
        assert_eq!(bounded_label("\n", 1), "…");
        assert_eq!(bounded_label("\n", 2), "\\n");
        assert_eq!(bounded_label("\u{7f}", 5), "…");
        assert_eq!(bounded_label("\u{7f}", 6), "\\u{7f}");
        assert_eq!(bounded_label("\\hello", 2), "\\\\…");
    }

    #[test]
    fn huge_source_has_bounded_display_and_accessibility() {
        let raw = "서울🙂\n".repeat(100_000);
        let preview = present_cell(Some(&raw));
        assert!(preview.clipped);
        assert_eq!(preview.original_bytes, Some(raw.len()));
        assert!(preview.display.len() <= DEFAULT_CELL_PREVIEW_BYTES + 3);
        assert!(preview.accessibility_value.len() <= DEFAULT_CELL_PREVIEW_BYTES + 80);
        assert!(preview.accessibility_value.contains("preview truncated"));
        assert!(preview.accessibility_value.contains(&raw.len().to_string()));
    }

    #[test]
    fn zero_limit_and_exact_limit_are_well_defined() {
        assert_eq!(bounded_label("", 0), "");
        assert_eq!(bounded_label("a", 0), "…");
        assert_eq!(bounded_label("hello", 5), "hello");
        assert_eq!(bounded_label("hello!", 5), "hello…");
        assert!(!present_cell_with_limit(Some(""), 0).clipped);
        assert!(present_cell_with_limit(Some("a"), 0).clipped);
        assert_eq!(present_cell_with_limit(None, 0).display, "NULL");
    }
}
