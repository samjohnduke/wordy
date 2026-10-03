//! IME composition: marked text and the UTF-16 offsets platforms speak.
//!
//! The editor's `EntityInputHandler` methods are thin wrappers over these
//! functions, which know nothing about gpui and can be exercised with a bare
//! `LoroText`. Offsets inside the editor are Unicode code points; the platform
//! talks in UTF-16 code units, so every range crosses here on the way in or out.

use std::ops::Range;

use wordy_doc::loro::LoroText;

use crate::Selection;

/// Composition state the editor keeps between IME calls, in code points.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Composition {
    /// Text the IME is still composing (underlined in the editor).
    pub marked: Option<Range<usize>>,
    pub sel: Selection,
}

pub fn cp_to_utf16(plain: &str, cp: usize) -> usize {
    plain.chars().take(cp).map(|c| c.len_utf16()).sum()
}

/// A UTF-16 offset inside a surrogate pair rounds up to the next code point.
pub fn utf16_to_cp(plain: &str, u: usize) -> usize {
    let mut acc = 0;
    for (i, c) in plain.chars().enumerate() {
        if acc >= u {
            return i;
        }
        acc += c.len_utf16();
    }
    plain.chars().count()
}

pub fn range_cp_to_utf16(plain: &str, r: &Range<usize>) -> Range<usize> {
    cp_to_utf16(plain, r.start)..cp_to_utf16(plain, r.end)
}

pub fn range_utf16_to_cp(plain: &str, r: &Range<usize>) -> Range<usize> {
    utf16_to_cp(plain, r.start)..utf16_to_cp(plain, r.end)
}

/// The range an IME call applies to: the explicit range if the platform gave
/// one, else the text being composed, else the selection. Clamped to `max`
/// (the last editable position, before the body's terminating newline).
pub fn target_range(plain: &str, st: &Composition, range_utf16: Option<Range<usize>>, max: usize) -> Range<usize> {
    let r = range_utf16
        .map(|r| range_utf16_to_cp(plain, &r))
        .or_else(|| st.marked.clone())
        .unwrap_or_else(|| st.sel.range());
    r.start.min(max)..r.end.min(max)
}

/// Commit: the text at `target` becomes `new_text`, composition ends, and
/// the caret sits after it. Backs `replace_text_in_range`.
pub fn replace(text: &LoroText, st: &mut Composition, target: Range<usize>, new_text: &str) {
    splice(text, target.clone(), new_text);
    st.sel = Selection::caret(target.start + new_text.chars().count());
    st.marked = None;
}

/// Compose: swap `new_text` in at `target`, mark it, and select inside it
/// where the platform asks (offsets relative to `new_text`, in UTF-16).
/// Empty `new_text` cancels the composition. Backs `replace_and_mark_text_in_range`.
pub fn replace_and_mark(
    text: &LoroText,
    st: &mut Composition,
    target: Range<usize>,
    new_text: &str,
    new_selected_range_utf16: Option<Range<usize>>,
) {
    splice(text, target.clone(), new_text);
    let n = new_text.chars().count();
    st.marked = (n > 0).then(|| target.start..target.start + n);
    st.sel = match new_selected_range_utf16 {
        Some(r) => Selection {
            anchor: target.start + utf16_to_cp(new_text, r.start).min(n),
            head: target.start + utf16_to_cp(new_text, r.end).min(n),
        },
        None => Selection::caret(target.start + n),
    };
}

/// The IME gave up on the composition; the text stays as typed.
pub fn unmark(st: &mut Composition) {
    st.marked = None;
}

fn splice(text: &LoroText, target: Range<usize>, new_text: &str) {
    if !target.is_empty() {
        if let Err(e) = text.delete(target.start, target.len()) {
            tracing::warn!("ime delete failed: {e}");
        }
    }
    if !new_text.is_empty() {
        if let Err(e) = text.insert(target.start, new_text) {
            tracing::warn!("ime insert failed: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wordy_doc::loro::LoroDoc;

    fn body(s: &str) -> (LoroDoc, LoroText) {
        let doc = LoroDoc::new();
        let text = doc.get_text("body");
        text.insert(0, s).unwrap();
        (doc, text)
    }

    fn max(text: &LoroText) -> usize {
        text.len_unicode() - 1
    }

    #[test]
    fn composition_round_trip_kana() {
        // Typing "k" then "a" in a Japanese IME: "k" is marked, becomes "か",
        // then is committed. The text before and after the caret stays put.
        let (_doc, text) = body("ab\n");
        let mut st = Composition {
            marked: None,
            sel: Selection::caret(1),
        };

        let t = target_range(&text.to_string(), &st, None, max(&text));
        assert_eq!(t, 1..1);
        replace_and_mark(&text, &mut st, t, "k", Some(0..1));
        assert_eq!(text.to_string(), "akb\n");
        assert_eq!(st.marked, Some(1..2));
        assert_eq!(st.sel, Selection { anchor: 1, head: 2 });

        // No explicit range: the marked text is replaced, not the selection.
        let t = target_range(&text.to_string(), &st, None, max(&text));
        assert_eq!(t, 1..2);
        replace_and_mark(&text, &mut st, t, "か", Some(1..1));
        assert_eq!(text.to_string(), "aかb\n");
        assert_eq!(st.marked, Some(1..2));
        assert_eq!(st.sel, Selection::caret(2));

        let t = target_range(&text.to_string(), &st, None, max(&text));
        replace(&text, &mut st, t, "か");
        assert_eq!(text.to_string(), "aかb\n");
        assert_eq!(st.marked, None);
        assert_eq!(st.sel, Selection::caret(2));
    }

    #[test]
    fn dead_key_then_letter() {
        // A dead acute is shown marked, then replaced by the composed letter.
        let (_doc, text) = body("e\n");
        let mut st = Composition::default();
        let t = target_range(&text.to_string(), &st, None, max(&text));
        replace_and_mark(&text, &mut st, t, "´", None);
        assert_eq!(text.to_string(), "´e\n");
        assert_eq!(st.marked, Some(0..1));
        assert_eq!(st.sel, Selection::caret(1));

        let t = target_range(&text.to_string(), &st, None, max(&text));
        replace(&text, &mut st, t, "é");
        assert_eq!(text.to_string(), "ée\n");
        assert_eq!((st.marked, st.sel), (None, Selection::caret(1)));
    }

    #[test]
    fn dead_key_cancelled_leaves_text_as_typed() {
        let (_doc, text) = body("x\n");
        let mut st = Composition::default();
        let t = target_range(&text.to_string(), &st, None, max(&text));
        replace_and_mark(&text, &mut st, t, "`", None);
        unmark(&mut st);
        assert_eq!(text.to_string(), "`x\n");
        assert_eq!(st.marked, None);
        // The next call has no mark to replace, so it targets the selection.
        let t = target_range(&text.to_string(), &st, None, max(&text));
        assert_eq!(t, 1..1);
    }

    #[test]
    fn composing_over_a_selection_then_replacing_the_marked_range() {
        let (_doc, text) = body("hello world\n");
        let mut st = Composition {
            marked: None,
            sel: Selection { anchor: 0, head: 5 },
        };
        // First composition call replaces the selected word.
        let t = target_range(&text.to_string(), &st, None, max(&text));
        assert_eq!(t, 0..5);
        replace_and_mark(&text, &mut st, t, "x", Some(0..1));
        assert_eq!(text.to_string(), "x world\n");
        assert_eq!(st.marked, Some(0..1));

        // The selection moves while composing; the marked range still wins.
        st.sel = Selection { anchor: 3, head: 5 };
        let t = target_range(&text.to_string(), &st, None, max(&text));
        assert_eq!(t, 0..1);
        replace(&text, &mut st, t, "yz");
        assert_eq!(text.to_string(), "yz world\n");
        assert_eq!((st.marked, st.sel), (None, Selection::caret(2)));
    }

    #[test]
    fn utf16_offsets_cross_surrogate_pairs() {
        // "𝒜" is one code point but two UTF-16 units.
        let (_doc, text) = body("𝒜b\n");
        let plain = text.to_string();
        assert_eq!(range_utf16_to_cp(&plain, &(2..3)), 1..2);
        assert_eq!(range_cp_to_utf16(&plain, &(1..2)), 2..3);
        assert_eq!(utf16_to_cp(&plain, 1), 1, "inside a pair rounds up");
        assert_eq!(utf16_to_cp(&plain, 99), 3, "past the end clamps");

        let mut st = Composition::default();
        let t = target_range(&plain, &st, Some(2..3), max(&text));
        replace_and_mark(&text, &mut st, t, "ß", None);
        assert_eq!(text.to_string(), "𝒜ß\n");
        assert_eq!(st.marked, Some(1..2));
        assert_eq!(range_cp_to_utf16(&text.to_string(), &(1..2)), 2..3);

        // A selection the platform gives inside astral marked text.
        let t = target_range(&text.to_string(), &st, None, max(&text));
        replace_and_mark(&text, &mut st, t, "𝒜𝒜", Some(2..4));
        assert_eq!(text.to_string(), "𝒜𝒜𝒜\n");
        assert_eq!(st.marked, Some(1..3));
        assert_eq!(st.sel, Selection { anchor: 2, head: 3 });
    }

    #[test]
    fn empty_composition_cancels_and_targets_clamp() {
        let (_doc, text) = body("ab\n");
        let mut st = Composition {
            marked: Some(0..2),
            sel: Selection::caret(2),
        };
        let t = target_range(&text.to_string(), &st, None, max(&text));
        replace_and_mark(&text, &mut st, t, "", None);
        assert_eq!(text.to_string(), "\n");
        assert_eq!((st.marked, st.sel), (None, Selection::caret(0)));

        // A range past the editable end is clamped before the newline.
        let (_doc, text) = body("ab\n");
        let st = Composition::default();
        assert_eq!(target_range(&text.to_string(), &st, Some(1..9), max(&text)), 1..2);
    }
}
