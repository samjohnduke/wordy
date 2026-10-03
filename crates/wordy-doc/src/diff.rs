//! Word-level diff between two bodies, for comparing a saved version with
//! the live text. Tokens are words, whitespace, and punctuation (Unicode word
//! boundaries), so a changed word shows as one deletion and one insertion
//! rather than a sea of character noise.

use loro::{LoroDoc, LoroText};
use unicode_segmentation::UnicodeSegmentation;

use crate::schema;

/// Which side of the comparison a span belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Diff {
    /// Present in the live body but not in the version.
    Ins,
    /// Present in the version but not in the live body.
    Del,
}

impl Diff {
    pub fn as_str(self) -> &'static str {
        match self {
            Diff::Ins => "ins",
            Diff::Del => "del",
        }
    }

    pub fn parse(s: &str) -> Option<Diff> {
        match s {
            "ins" => Some(Diff::Ins),
            "del" => Some(Diff::Del),
            _ => None,
        }
    }
}

/// One stretch of the merged text: unchanged, inserted, or deleted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffSpan {
    pub text: String,
    pub kind: Option<Diff>,
}

/// Counts of changed words on each side.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DiffStats {
    pub inserted: usize,
    pub deleted: usize,
}

/// A word with the punctuation and spacing that follows it. Two tokens are
/// equal when their words match, so a reflowed paragraph diffs as unchanged.
#[derive(Clone, Copy, Debug)]
struct Tok<'a> {
    raw: &'a str,
    key: &'a str,
}

impl PartialEq for Tok<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

fn tokens(s: &str) -> Vec<Tok<'_>> {
    let mut out: Vec<Tok> = Vec::new();
    let mut cur: Option<(usize, usize)> = None; // byte range of the token being built
    for (i, w) in s.split_word_bound_indices() {
        if w.contains('\n') {
            // Newlines are their own tokens so paragraph structure survives.
            flush_tok(s, &mut out, &mut cur);
            let mut off = i;
            for part in w.split_inclusive('\n') {
                let (text, nl) = match part.strip_suffix('\n') {
                    Some(t) => (t, true),
                    None => (part, false),
                };
                if !text.is_empty() {
                    out.push(Tok {
                        raw: &s[off..off + text.len()],
                        key: text.trim_end(),
                    });
                }
                if nl {
                    out.push(Tok { raw: "\n", key: "\n" });
                }
                off += part.len();
            }
            continue;
        }
        let word = w.chars().any(|c| c.is_alphanumeric());
        match (&mut cur, word) {
            (Some((_, b)), false) => *b = i + w.len(),
            (Some(_), true) => {
                flush_tok(s, &mut out, &mut cur);
                cur = Some((i, i + w.len()));
            }
            (None, _) => cur = Some((i, i + w.len())),
        }
    }
    flush_tok(s, &mut out, &mut cur);
    out
}

fn flush_tok<'a>(s: &'a str, out: &mut Vec<Tok<'a>>, cur: &mut Option<(usize, usize)>) {
    if let Some((a, b)) = cur.take() {
        let raw = &s[a..b];
        out.push(Tok {
            raw,
            key: raw.trim_end(),
        });
    }
}

/// Merge `old` (the version) and `new` (the live body) into one sequence of
/// spans. Unchanged text appears once (with the live spacing); deletions come
/// before the insertions that replaced them.
pub fn word_diff(old: &str, new: &str) -> Vec<DiffSpan> {
    let a = tokens(old);
    let b = tokens(new);
    let mut spans: Vec<DiffSpan> = Vec::new();
    let mut del = String::new();
    let mut ins = String::new();
    for r in diff::slice(&a, &b) {
        match r {
            diff::Result::Both(_, t) => {
                flush_hunk(&mut spans, &mut del, &mut ins);
                push_span(&mut spans, t.raw, None);
            }
            diff::Result::Left(t) => del.push_str(t.raw),
            diff::Result::Right(t) => ins.push_str(t.raw),
        }
    }
    flush_hunk(&mut spans, &mut del, &mut ins);
    spans
}

fn push_span(spans: &mut Vec<DiffSpan>, text: &str, kind: Option<Diff>) {
    if text.is_empty() {
        return;
    }
    if let Some(last) = spans.last_mut() {
        if last.kind == kind {
            last.text.push_str(text);
            return;
        }
    }
    spans.push(DiffSpan {
        text: text.to_string(),
        kind,
    });
}

/// Emit one hunk: old words, new words, then the spacing that follows them.
fn flush_hunk(spans: &mut Vec<DiffSpan>, del: &mut String, ins: &mut String) {
    if del.is_empty() && ins.is_empty() {
        return;
    }
    let tail_src = if ins.is_empty() { del.as_str() } else { ins.as_str() };
    // Trailing spaces are spacing; a newline is a real paragraph change.
    let kept = tail_src.trim_end_matches(|c: char| c.is_whitespace() && c != '\n');
    let tail = tail_src[kept.len()..].to_string();
    let trim = |s: &str| s.trim_end_matches(|c: char| c.is_whitespace() && c != '\n').to_string();
    push_span(spans, &trim(del), Some(Diff::Del));
    push_span(spans, &trim(ins), Some(Diff::Ins));
    push_span(spans, &tail, None);
    del.clear();
    ins.clear();
}

pub fn stats(spans: &[DiffSpan]) -> DiffStats {
    let mut s = DiffStats::default();
    for span in spans {
        let words = crate::paragraphs::count_words(&span.text);
        match span.kind {
            Some(Diff::Ins) => s.inserted += words,
            Some(Diff::Del) => s.deleted += words,
            None => {}
        }
    }
    s
}

/// A standalone doc whose body is the merged text with `diff` marks, ready
/// to show in a read-only editor.
pub fn diff_doc(spans: &[DiffSpan]) -> (LoroDoc, LoroText) {
    let doc = LoroDoc::new();
    schema::configure_text_styles(&doc);
    let text = doc.get_text("diff");
    let mut cp = 0usize;
    for span in spans {
        let n = span.text.chars().count();
        let _ = text.insert(cp, &span.text);
        if let Some(kind) = span.kind {
            let _ = text.mark(cp..cp + n, "diff", kind.as_str());
        }
        cp += n;
    }
    if !text.to_string().ends_with('\n') {
        let _ = text.insert(cp, "\n");
    }
    doc.commit();
    (doc, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(spans: &[DiffSpan]) -> Vec<(Option<Diff>, &str)> {
        spans.iter().map(|s| (s.kind, s.text.as_str())).collect()
    }

    #[test]
    fn unchanged_text_is_one_span() {
        let d = word_diff("the cat sat\n", "the cat sat\n");
        assert_eq!(kinds(&d), vec![(None, "the cat sat\n")]);
    }

    #[test]
    fn replaced_word_is_del_then_ins() {
        let d = word_diff("the cat sat", "the dog sat");
        assert_eq!(
            kinds(&d),
            vec![
                (None, "the "),
                (Some(Diff::Del), "cat"),
                (Some(Diff::Ins), "dog"),
                (None, " sat")
            ]
        );
        let s = stats(&d);
        assert_eq!((s.inserted, s.deleted), (1, 1));
    }

    #[test]
    fn insertion_and_deletion_at_edges() {
        let d = word_diff("b c", "a b");
        assert_eq!(
            kinds(&d),
            vec![(Some(Diff::Ins), "a"), (None, " b"), (Some(Diff::Del), "c")]
        );
    }

    #[test]
    fn whitespace_only_changes_are_quiet() {
        let d = word_diff("one  two", "one two");
        assert_eq!(kinds(&d), vec![(None, "one two")]);
    }

    #[test]
    fn paragraph_breaks_count() {
        let d = word_diff("a b\n", "a\nb\n");
        assert_eq!(kinds(&d), vec![(None, "a"), (Some(Diff::Ins), "\n"), (None, "b\n")]);
    }

    #[test]
    fn diff_doc_carries_marks() {
        let spans = word_diff("the cat sat", "the dog sat");
        let (_doc, text) = diff_doc(&spans);
        let paras = crate::Paragraphs::from_text(&text);
        let runs = &paras.get(0).unwrap().runs;
        assert_eq!(runs.len(), 4, "{runs:?}");
        assert_eq!(runs[1].marks.diff, Some(Diff::Del));
        assert_eq!(runs[2].marks.diff, Some(Diff::Ins));
        assert_eq!(text.to_string(), "the catdog sat\n");
    }
}
