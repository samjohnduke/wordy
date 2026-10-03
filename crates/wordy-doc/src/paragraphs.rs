//! A derived, paragraph-oriented view over a rich-text body.
//!
//! The Loro text follows the Quill convention: every paragraph is terminated
//! by a `\n`, and the `block` mark on that newline carries the paragraph's
//! block type (`p`, `h1`, `h2`, `h3`, `quote`, `break`). This module turns the
//! flat delta into a list of [`Paragraph`]s with their inline [`Run`]s, and
//! provides the Unicode-code-point <-> (paragraph, byte) mapping the editor
//! needs for layout and cursor movement.
//!
//! Everything here is derived from the document and is never written back.

use loro::{LoroText, LoroValue, TextDelta};
use std::collections::HashMap;
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

use crate::schema::block as block_keys;

/// Paragraph-level block type.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Block {
    #[default]
    Paragraph,
    H1,
    H2,
    H3,
    Quote,
    /// A scene break (rendered as a centered ornament); has no text.
    Break,
}

impl Block {
    pub fn as_str(self) -> &'static str {
        match self {
            Block::Paragraph => block_keys::PARAGRAPH,
            Block::H1 => block_keys::H1,
            Block::H2 => block_keys::H2,
            Block::H3 => block_keys::H3,
            Block::Quote => block_keys::QUOTE,
            Block::Break => block_keys::BREAK,
        }
    }

    pub fn parse(s: &str) -> Option<Block> {
        Some(match s {
            block_keys::PARAGRAPH => Block::Paragraph,
            block_keys::H1 => Block::H1,
            block_keys::H2 => Block::H2,
            block_keys::H3 => Block::H3,
            block_keys::QUOTE => Block::Quote,
            block_keys::BREAK => Block::Break,
            _ => return None,
        })
    }

    pub fn is_heading(self) -> bool {
        matches!(self, Block::H1 | Block::H2 | Block::H3)
    }

    /// Menu label.
    pub fn label(self) -> &'static str {
        match self {
            Block::Paragraph => "Paragraph",
            Block::H1 => "Heading 1",
            Block::H2 => "Heading 2",
            Block::H3 => "Heading 3",
            Block::Quote => "Quote",
            Block::Break => "Scene break",
        }
    }
}

/// Highlighter colour stored as the `highlight` mark's value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Highlight {
    Yellow,
    Green,
    Blue,
    Pink,
    Grey,
}

impl Highlight {
    pub const ALL: [Highlight; 5] = [
        Highlight::Yellow,
        Highlight::Green,
        Highlight::Blue,
        Highlight::Pink,
        Highlight::Grey,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Highlight::Yellow => "yellow",
            Highlight::Green => "green",
            Highlight::Blue => "blue",
            Highlight::Pink => "pink",
            Highlight::Grey => "grey",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Highlight::Yellow => "Yellow",
            Highlight::Green => "Green",
            Highlight::Blue => "Blue",
            Highlight::Pink => "Pink",
            Highlight::Grey => "Grey",
        }
    }

    pub fn parse(s: &str) -> Option<Highlight> {
        Highlight::ALL.into_iter().find(|h| h.as_str() == s)
    }

    /// The colour after this one when cycling; `None` after the last.
    pub fn next(self) -> Option<Highlight> {
        let i = Highlight::ALL.iter().position(|h| *h == self)?;
        Highlight::ALL.get(i + 1).copied()
    }

    /// Hex colour (no `#`) used by the exporters.
    pub fn hex(self) -> &'static str {
        match self {
            Highlight::Yellow => "fff2a8",
            Highlight::Green => "c8f0c0",
            Highlight::Blue => "c6e2ff",
            Highlight::Pink => "ffcfe0",
            Highlight::Grey => "dedede",
        }
    }
}

/// Inline formatting active on a run of text.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Marks {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub smallcaps: bool,
    pub highlight: Option<Highlight>,
    /// Link target: a node id string or a title alias.
    pub link: Option<String>,
    /// Comment id (key into the project's `comments` map).
    pub comment: Option<String>,
    /// Version-compare marker; only ever set on diff documents.
    pub diff: Option<crate::diff::Diff>,
}

impl Marks {
    pub fn from_attributes<S: std::hash::BuildHasher>(attrs: Option<&HashMap<String, LoroValue, S>>) -> Marks {
        let mut m = Marks::default();
        let Some(attrs) = attrs else { return m };
        for (k, v) in attrs {
            let truthy = match v {
                LoroValue::Bool(b) => *b,
                LoroValue::Null => false,
                _ => true,
            };
            match k.as_str() {
                "bold" => m.bold = truthy,
                "italic" => m.italic = truthy,
                "underline" => m.underline = truthy,
                "strike" => m.strike = truthy,
                "smallcaps" => m.smallcaps = truthy,
                "highlight" => {
                    m.highlight = match v {
                        // Pre-colour documents stored a bare `true`.
                        LoroValue::Bool(true) => Some(Highlight::Yellow),
                        LoroValue::String(s) => Highlight::parse(s),
                        _ => None,
                    }
                }
                "link" => m.link = v.as_string().map(|s| s.to_string()),
                "comment" => m.comment = v.as_string().map(|s| s.to_string()),
                "diff" => m.diff = v.as_string().and_then(|s| crate::diff::Diff::parse(s)),
                _ => {}
            }
        }
        m
    }

    pub fn is_plain(&self) -> bool {
        *self == Marks::default()
    }
}

/// A maximal run of text sharing one set of marks. Never contains `\n`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub text: String,
    pub marks: Marks,
}

/// One paragraph: its runs, block type, and position in the whole text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paragraph {
    pub block: Block,
    pub runs: Vec<Run>,
    /// Concatenation of all run texts (no trailing newline).
    pub text: String,
    /// Code-point offset of the first character in the whole body.
    pub start_cp: usize,
    /// Length in code points, excluding the terminating newline.
    pub len_cp: usize,
    /// False only for a trailing fragment without a terminating newline.
    pub terminated: bool,
}

impl Paragraph {
    /// Code-point index of this paragraph's terminating newline (or end).
    pub fn newline_cp(&self) -> usize {
        self.start_cp + self.len_cp
    }

    /// Code-point index just past the terminating newline.
    pub fn end_cp(&self) -> usize {
        self.newline_cp() + usize::from(self.terminated)
    }

    /// Convert a byte offset within `text` to a code-point offset within the paragraph.
    pub fn byte_to_cp(&self, byte: usize) -> usize {
        self.text[..byte.min(self.text.len())].chars().count()
    }

    /// Convert a code-point offset within the paragraph to a byte offset in `text`.
    pub fn cp_to_byte(&self, cp: usize) -> usize {
        self.text
            .char_indices()
            .nth(cp)
            .map(|(b, _)| b)
            .unwrap_or(self.text.len())
    }

    /// Marks in effect at a code-point offset, looking at the character before it
    /// (how a caret inherits formatting when typing).
    pub fn marks_before(&self, cp: usize) -> Marks {
        if cp == 0 {
            return self.runs.first().map(|r| r.marks.clone()).unwrap_or_default();
        }
        let mut acc = 0;
        for run in &self.runs {
            let n = run.text.chars().count();
            if cp <= acc + n {
                return run.marks.clone();
            }
            acc += n;
        }
        self.runs.last().map(|r| r.marks.clone()).unwrap_or_default()
    }

    pub fn word_count(&self) -> usize {
        count_words(&self.text)
    }
}

/// A position inside a [`Paragraphs`] view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Position {
    pub para: usize,
    /// Byte offset within the paragraph text.
    pub byte: usize,
    /// Code-point offset within the paragraph text.
    pub cp: usize,
}

/// The whole body split into paragraphs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Paragraphs {
    paras: Vec<Paragraph>,
    total_cp: usize,
}

impl Paragraphs {
    pub fn from_text(text: &LoroText) -> Paragraphs {
        Paragraphs::from_delta(&text.to_delta())
    }

    pub fn from_delta(delta: &[TextDelta]) -> Paragraphs {
        let mut paras = Vec::new();
        let mut runs: Vec<Run> = Vec::new();
        let mut text = String::new();
        let mut start_cp = 0usize;
        let mut cp = 0usize;

        let push_run = |runs: &mut Vec<Run>, s: &str, marks: &Marks| {
            if s.is_empty() {
                return;
            }
            if let Some(last) = runs.last_mut() {
                if &last.marks == marks {
                    last.text.push_str(s);
                    return;
                }
            }
            runs.push(Run {
                text: s.to_string(),
                marks: marks.clone(),
            });
        };

        for item in delta {
            let TextDelta::Insert { insert, attributes } = item else {
                continue;
            };
            let marks = Marks::from_attributes(attributes.as_ref());
            let mut rest = insert.as_str();
            while let Some(nl) = rest.find('\n') {
                let piece = &rest[..nl];
                push_run(&mut runs, piece, &marks);
                text.push_str(piece);
                cp += piece.chars().count();
                let block = attributes
                    .as_ref()
                    .and_then(|a| a.get("block"))
                    .and_then(|v| v.as_string())
                    .and_then(|s| Block::parse(s))
                    .unwrap_or_default();
                paras.push(Paragraph {
                    block,
                    runs: std::mem::take(&mut runs),
                    text: std::mem::take(&mut text),
                    start_cp,
                    len_cp: cp - start_cp,
                    terminated: true,
                });
                cp += 1; // the newline itself
                start_cp = cp;
                rest = &rest[nl + 1..];
            }
            push_run(&mut runs, rest, &marks);
            text.push_str(rest);
            cp += rest.chars().count();
        }

        if !runs.is_empty() || paras.is_empty() {
            paras.push(Paragraph {
                block: Block::Paragraph,
                runs,
                text,
                start_cp,
                len_cp: cp - start_cp,
                terminated: false,
            });
        }

        Paragraphs { paras, total_cp: cp }
    }

    pub fn iter(&self) -> impl Iterator<Item = &Paragraph> {
        self.paras.iter()
    }

    pub fn len(&self) -> usize {
        self.paras.len()
    }

    pub fn is_empty(&self) -> bool {
        self.paras.is_empty()
    }

    pub fn get(&self, ix: usize) -> Option<&Paragraph> {
        self.paras.get(ix)
    }

    /// Every explicit link as a code-point range plus its target id string.
    /// Adjacent runs linking to the same target (bold inside a link, say)
    /// are merged into one span.
    pub fn link_spans(&self) -> Vec<(Range<usize>, String)> {
        let mut out: Vec<(Range<usize>, String)> = Vec::new();
        for p in &self.paras {
            let mut cp = p.start_cp;
            for r in &p.runs {
                let n = r.text.chars().count();
                if let Some(l) = &r.marks.link {
                    if let Some((last, id)) = out.last_mut() {
                        if *id == *l && last.end == cp {
                            last.end = cp + n;
                            cp += n;
                            continue;
                        }
                    }
                    out.push((cp..cp + n, l.clone()));
                }
                cp += n;
            }
        }
        out
    }

    /// The text within a code-point range (never crosses a newline).
    pub fn slice_cp(&self, r: Range<usize>) -> String {
        let pos = self.locate(r.start);
        let Some(p) = self.paras.get(pos.para) else {
            return String::new();
        };
        let start = r.start.saturating_sub(p.start_cp);
        let end = r.end.saturating_sub(p.start_cp).min(p.len_cp);
        p.text.chars().skip(start).take(end.saturating_sub(start)).collect()
    }

    pub fn last(&self) -> Option<&Paragraph> {
        self.paras.last()
    }

    /// Total length in code points, including newlines.
    pub fn total_cp(&self) -> usize {
        self.total_cp
    }

    /// Whether the body ends with a newline (the editor invariant).
    pub fn is_terminated(&self) -> bool {
        self.paras.last().map(|p| p.terminated).unwrap_or(false)
    }

    /// The largest valid caret position: just before the final newline.
    pub fn max_cursor(&self) -> usize {
        match self.paras.last() {
            Some(p) if p.terminated => p.newline_cp(),
            Some(p) => p.end_cp(),
            None => 0,
        }
    }

    /// Locate a code-point offset. Offsets on a newline map to the end of
    /// that paragraph; offsets past the end clamp to the last paragraph.
    pub fn locate(&self, cp: usize) -> Position {
        if self.paras.is_empty() {
            return Position {
                para: 0,
                byte: 0,
                cp: 0,
            };
        }
        let ix = match self.paras.binary_search_by(|p| {
            if cp < p.start_cp {
                std::cmp::Ordering::Greater
            } else if cp > p.newline_cp() {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        }) {
            Ok(ix) => ix,
            Err(ix) => ix.min(self.paras.len() - 1),
        };
        let p = &self.paras[ix];
        let cp_in = cp.saturating_sub(p.start_cp).min(p.len_cp);
        Position {
            para: ix,
            byte: p.cp_to_byte(cp_in),
            cp: cp_in,
        }
    }

    /// Code-point offset of a (paragraph, byte offset) pair.
    pub fn cp_at(&self, para: usize, byte: usize) -> usize {
        let p = &self.paras[para];
        p.start_cp + p.byte_to_cp(byte)
    }

    pub fn word_count(&self) -> usize {
        self.paras.iter().map(|p| p.word_count()).sum()
    }

    /// Plain text with paragraphs joined by newlines (no trailing newline).
    pub fn plain_text(&self) -> String {
        let mut s = String::new();
        for (i, p) in self.paras.iter().enumerate() {
            if i > 0 {
                s.push('\n');
            }
            s.push_str(&p.text);
        }
        s
    }
}

/// Count words the way a novelist expects: Unicode word boundaries, ignoring punctuation.
pub fn count_words(s: &str) -> usize {
    s.unicode_words().count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::configure_text_styles;
    use loro::LoroDoc;

    fn doc_with(f: impl FnOnce(&LoroText)) -> LoroText {
        let doc = LoroDoc::new();
        configure_text_styles(&doc);
        let t = doc.get_text("body");
        f(&t);
        doc.commit();
        t
    }

    #[test]
    fn splits_paragraphs_and_blocks() {
        let t = doc_with(|t| {
            t.insert(0, "Title\nFirst para.\nSecond para.\n").unwrap();
            t.mark(5..6, "block", "h1").unwrap();
            t.mark(0..5, "bold", true).unwrap();
        });
        let p = Paragraphs::from_text(&t);
        assert_eq!(p.len(), 3);
        assert!(p.is_terminated());
        assert_eq!(p.get(0).unwrap().block, Block::H1);
        assert_eq!(p.get(0).unwrap().runs.len(), 1);
        assert!(p.get(0).unwrap().runs[0].marks.bold);
        assert_eq!(p.get(1).unwrap().block, Block::Paragraph);
        assert_eq!(p.get(1).unwrap().text, "First para.");
        assert_eq!(p.get(1).unwrap().start_cp, 6);
        assert_eq!(p.get(2).unwrap().start_cp, 6 + 12);
        assert_eq!(p.total_cp(), t.len_unicode());
        assert_eq!(p.max_cursor(), t.len_unicode() - 1);
        assert_eq!(p.word_count(), 5);
    }

    #[test]
    fn runs_merge_and_split_on_marks() {
        let t = doc_with(|t| {
            t.insert(0, "plain bold italic\n").unwrap();
            t.mark(6..10, "bold", true).unwrap();
            t.mark(11..17, "italic", true).unwrap();
        });
        let p = Paragraphs::from_text(&t);
        let runs = &p.get(0).unwrap().runs;
        let texts: Vec<&str> = runs.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(texts, vec!["plain ", "bold", " ", "italic"]);
        assert!(runs[1].marks.bold && !runs[1].marks.italic);
        assert!(runs[3].marks.italic);
    }

    #[test]
    fn locate_handles_multibyte_and_newlines() {
        let t = doc_with(|t| {
            t.insert(0, "héllo\nwörld\n").unwrap();
        });
        let p = Paragraphs::from_text(&t);
        // 'é' is 2 bytes, 1 cp.
        let pos = p.locate(3);
        assert_eq!(
            pos,
            Position {
                para: 0,
                byte: 4,
                cp: 3
            }
        );
        // The newline position maps to end of paragraph 0.
        let pos = p.locate(5);
        assert_eq!(pos.para, 0);
        assert_eq!(pos.cp, 5);
        // Start of paragraph 1.
        let pos = p.locate(6);
        assert_eq!(
            pos,
            Position {
                para: 1,
                byte: 0,
                cp: 0
            }
        );
        assert_eq!(p.cp_at(1, 3), 6 + 2); // "wö" is 3 bytes, 2 cps
                                          // Past the end clamps.
        let pos = p.locate(999);
        assert_eq!(pos.para, 1);
        assert_eq!(pos.cp, 5);
    }

    #[test]
    fn unterminated_tail_is_reported() {
        let t = doc_with(|t| t.insert(0, "a\nb").unwrap());
        let p = Paragraphs::from_text(&t);
        assert_eq!(p.len(), 2);
        assert!(!p.is_terminated());
        assert_eq!(p.max_cursor(), 3);
    }

    #[test]
    fn empty_text_has_one_empty_paragraph() {
        let t = doc_with(|_| {});
        let p = Paragraphs::from_text(&t);
        assert_eq!(p.len(), 1);
        assert_eq!(p.get(0).unwrap().text, "");
        assert_eq!(
            p.locate(0),
            Position {
                para: 0,
                byte: 0,
                cp: 0
            }
        );
    }

    #[test]
    fn marks_before_inherits_from_previous_char() {
        let t = doc_with(|t| {
            t.insert(0, "ab cd\n").unwrap();
            t.mark(0..2, "bold", true).unwrap();
        });
        let p = Paragraphs::from_text(&t);
        let para = p.get(0).unwrap();
        assert!(para.marks_before(2).bold);
        assert!(!para.marks_before(3).bold);
        assert!(para.marks_before(0).bold);
    }
}
