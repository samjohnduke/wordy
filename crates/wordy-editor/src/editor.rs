//! `ProseEditor`: editing state and behavior for one rich-text body.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, button::{Button, ButtonVariants as _}, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use unicode_segmentation::UnicodeSegmentation;
use wordy_doc::loro::{CommitOptions, ContainerTrait as _, LoroDoc, LoroText, LoroValue, UndoItemMeta, UndoManager};
use wordy_doc::{Block, Comments, Marks, Paragraphs, Run, META_ORIGIN};

use crate::element::{FrameLayout, ProseElement};
use crate::style::EditorStyle;
use crate::typography::smart_replace;
use crate::*;

/// A selection in Unicode code points. `anchor == head` is a caret.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub anchor: usize,
    pub head: usize,
}

impl Selection {
    pub fn caret(cp: usize) -> Self {
        Self { anchor: cp, head: cp }
    }

    pub fn range(&self) -> Range<usize> {
        self.anchor.min(self.head)..self.anchor.max(self.head)
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditorEvent {
    /// The text changed (and was committed to the Loro doc).
    Edited,
    SelectionChanged,
}

type SelSnapshot = Arc<Mutex<(usize, usize)>>;
type SelRestore = Arc<Mutex<Option<(usize, usize)>>>;

/// One paragraph of a copied fragment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FragmentPara {
    pub block: Block,
    pub runs: Vec<Run>,
    /// Whether the paragraph's terminating newline was part of the copy.
    pub terminated: bool,
}

/// A formatted slice of a body, as placed on the clipboard by `copy`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RichFragment {
    pub paras: Vec<FragmentPara>,
}

impl RichFragment {
    pub fn plain_text(&self) -> String {
        let mut s = String::new();
        for p in &self.paras {
            for r in &p.runs {
                s.push_str(&r.text);
            }
            if p.terminated {
                s.push('\n');
            }
        }
        s
    }
}

/// App-wide memory of the last in-app copy, so a paste can restore formatting
/// when the system clipboard still holds the same plain text.
#[derive(Default)]
pub struct RichClipboard {
    pub text: String,
    pub fragment: RichFragment,
}

impl Global for RichClipboard {}

/// A comment anchored in the body, derived from the `comment` marks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommentAnchor {
    pub id: String,
    pub range: Range<usize>,
    pub text: String,
    pub resolved: bool,
}

struct CommentEdit {
    id: String,
    input: Entity<InputState>,
    _sub: Subscription,
}

pub struct ProseEditor {
    doc: LoroDoc,
    text: LoroText,
    undo: UndoManager,
    /// Selection as of the start of the current edit; captured into undo items.
    undo_sel: SelSnapshot,
    /// Selection handed back by the undo manager after undo/redo.
    undo_restore: SelRestore,
    paras: Paragraphs,
    /// The full plain text including newlines (for UTF-16 conversions).
    plain: String,
    sel: Selection,
    goal_x: Option<Pixels>,
    marked: Option<Range<usize>>,
    pub focus: FocusHandle,
    pub style: EditorStyle,
    pub(crate) scroll_y: Pixels,
    pub(crate) scroll_to_cursor: bool,
    pub(crate) frame: Rc<RefCell<Option<FrameLayout>>>,
    pub(crate) blink_on: bool,
    blink_epoch: usize,
    _blink: Task<()>,
    pub(crate) selecting: bool,
    read_only: bool,
    /// Commit origin for this editor's body edits (`body:<container id>`).
    origin: String,
    /// Marks to apply to the next typed text when the caret is collapsed.
    pending: Option<Marks>,
    pub smart_typography: bool,
    comments: Option<Comments>,
    /// Node id string recorded on new comments.
    node_label: Option<String>,
    comment_edit: Option<CommentEdit>,
    pub(crate) show_resolved: bool,
    search: Option<String>,
    pub(crate) matches: Vec<Range<usize>>,
    pub(crate) match_ix: Option<usize>,
}

impl EventEmitter<EditorEvent> for ProseEditor {}

impl Focusable for ProseEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl ProseEditor {
    pub fn new(doc: LoroDoc, text: LoroText, cx: &mut Context<Self>) -> Self {
        // Editor invariant: the body always ends with a newline.
        let paras = Paragraphs::from_text(&text);
        if !paras.is_terminated() {
            let _ = text.insert(text.len_unicode(), "\n");
            doc.commit();
        }
        let paras = Paragraphs::from_text(&text);
        let plain = text.to_string();

        let undo_sel: SelSnapshot = Arc::new(Mutex::new((0, 0)));
        let undo_restore: SelRestore = Arc::new(Mutex::new(None));
        let origin = format!("body:{}", text.id());
        let mut undo = UndoManager::new(&doc);
        undo.set_merge_interval(700);
        undo.add_exclude_origin_prefix(META_ORIGIN);
        {
            let snap = undo_sel.clone();
            undo.set_on_push(Some(Box::new(move |_kind, _span, _event| {
                let (a, h) = *snap.lock().unwrap();
                UndoItemMeta {
                    value: LoroValue::from(vec![
                        LoroValue::I64(a as i64),
                        LoroValue::I64(h as i64),
                    ]),
                    cursors: Vec::new(),
                }
            })));
            let restore = undo_restore.clone();
            undo.set_on_pop(Some(Box::new(move |_kind, _span, meta| {
                if let LoroValue::List(list) = &meta.value {
                    if let [LoroValue::I64(a), LoroValue::I64(h)] = list.as_slice() {
                        *restore.lock().unwrap() = Some((*a as usize, *h as usize));
                    }
                }
            })));
        }

        let mut this = Self {
            doc,
            text,
            undo,
            undo_sel,
            undo_restore,
            paras,
            plain,
            sel: Selection::default(),
            goal_x: None,
            marked: None,
            focus: cx.focus_handle(),
            style: EditorStyle::default(),
            scroll_y: px(0.),
            scroll_to_cursor: false,
            frame: Rc::new(RefCell::new(None)),
            blink_on: true,
            blink_epoch: 0,
            _blink: Task::ready(()),
            selecting: false,
            read_only: false,
            origin,
            pending: None,
            smart_typography: true,
            comments: None,
            node_label: None,
            comment_edit: None,
            show_resolved: false,
            search: None,
            matches: Vec::new(),
            match_ix: None,
        };
        this.restart_blink(cx);
        this
    }

    /// The commit origin of this editor's edits. Other editors on the same doc
    /// exclude it from their undo stacks (see [`Self::exclude_origin`]).
    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// Keep edits committed under `prefix` out of this editor's undo history.
    pub fn exclude_origin(&mut self, prefix: &str) {
        self.undo.add_exclude_origin_prefix(prefix);
    }

    /// Attach the project's comments map; `node_label` is recorded on new comments.
    pub fn set_comments(&mut self, comments: Comments, node_label: String) {
        self.comments = Some(comments);
        self.node_label = Some(node_label);
    }

    // ----- accessors -------------------------------------------------------

    pub fn paragraphs(&self) -> &Paragraphs {
        &self.paras
    }

    pub fn selection(&self) -> Selection {
        self.sel
    }

    pub fn word_count(&self) -> usize {
        self.paras.word_count()
    }

    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    pub fn set_read_only(&mut self, ro: bool) {
        self.read_only = ro;
    }

    pub fn marked_range(&self) -> Option<Range<usize>> {
        self.marked.clone()
    }

    /// Block type of the paragraph at the selection head.
    pub fn current_block(&self) -> Block {
        let pos = self.paras.locate(self.sel.head);
        self.paras.get(pos.para).map(|p| p.block).unwrap_or_default()
    }

    /// Marks that the next typed character will get: pending toggles if any,
    /// else the marks inherited from the preceding character.
    pub fn current_marks(&self) -> Marks {
        if let Some(p) = &self.pending {
            return p.clone();
        }
        self.inherited_marks()
    }

    fn inherited_marks(&self) -> Marks {
        let pos = self.paras.locate(self.sel.head);
        self.paras
            .get(pos.para)
            .map(|p| p.marks_before(pos.cp))
            .unwrap_or_default()
    }

    /// Comments anchored in this body, in document order. Resolved ones are
    /// included only when `show_resolved` is set.
    pub fn comment_anchors(&self) -> Vec<CommentAnchor> {
        self.comment_anchors_with(self.show_resolved)
    }

    fn comment_anchors_with(&self, include_resolved: bool) -> Vec<CommentAnchor> {
        let Some(comments) = &self.comments else { return Vec::new() };
        let mut out: Vec<CommentAnchor> = Vec::new();
        for p in self.paras.iter() {
            let mut cp = p.start_cp;
            for run in &p.runs {
                let n = run.text.chars().count();
                if let Some(id) = &run.marks.comment {
                    match out.last_mut() {
                        Some(last) if &last.id == id => last.range.end = cp + n,
                        _ => {
                            let c = comments.get(id);
                            out.push(CommentAnchor {
                                id: id.clone(),
                                range: cp..cp + n,
                                text: c.as_ref().map(|c| c.text.clone()).unwrap_or_default(),
                                resolved: c.as_ref().map(|c| c.resolved).unwrap_or(false),
                            });
                        }
                    }
                }
                cp += n;
            }
        }
        if !include_resolved {
            out.retain(|a| !a.resolved);
        }
        out
    }

    /// The comment being edited, or the one under the caret.
    pub fn active_comment(&self) -> Option<String> {
        if let Some(e) = &self.comment_edit {
            return Some(e.id.clone());
        }
        let r = self.sel.range();
        self.comment_anchors()
            .into_iter()
            .find(|a| a.range.start <= r.start && r.end <= a.range.end)
            .map(|a| a.id)
    }

    pub fn is_editing_comment(&self) -> bool {
        self.comment_edit.is_some()
    }

    pub fn search_query(&self) -> Option<&str> {
        self.search.as_deref()
    }

    /// (index of the current match, number of matches).
    pub fn search_status(&self) -> (Option<usize>, usize) {
        (self.match_ix, self.matches.len())
    }

    pub fn selected_text(&self) -> String {
        let r = self.sel.range();
        self.plain.chars().skip(r.start).take(r.len()).collect()
    }

    // ----- internal plumbing ----------------------------------------------

    fn begin_edit(&mut self) {
        *self.undo_sel.lock().unwrap() = (self.sel.anchor, self.sel.head);
    }

    fn commit(&mut self, cx: &mut Context<Self>) {
        self.doc.commit_with(CommitOptions::default().origin(&self.origin));
        self.refresh(cx);
        cx.emit(EditorEvent::Edited);
    }

    /// Commit a metadata-only change (comment text, resolved flag) outside undo.
    fn commit_meta(&mut self, cx: &mut Context<Self>) {
        self.doc.commit_with(CommitOptions::default().origin(META_ORIGIN));
        cx.emit(EditorEvent::Edited);
        cx.notify();
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.paras = Paragraphs::from_text(&self.text);
        self.plain = self.text.to_string();
        let max = self.paras.max_cursor();
        self.sel.anchor = self.sel.anchor.min(max);
        self.sel.head = self.sel.head.min(max);
        if let Some(m) = &self.marked {
            if m.end > max {
                self.marked = None;
            }
        }
        self.pending = None;
        self.recompute_matches();
        self.scroll_to_cursor = true;
        self.restart_blink(cx);
        cx.notify();
    }

    fn set_selection(&mut self, sel: Selection, cx: &mut Context<Self>) {
        let max = self.paras.max_cursor();
        let sel = Selection { anchor: sel.anchor.min(max), head: sel.head.min(max) };
        if sel != self.sel {
            self.sel = sel;
            self.pending = None;
            self.match_ix = self.matches.iter().position(|m| *m == sel.range());
            cx.emit(EditorEvent::SelectionChanged);
        }
        self.scroll_to_cursor = true;
        self.restart_blink(cx);
        cx.notify();
    }

    fn move_to(&mut self, cp: usize, extend: bool, cx: &mut Context<Self>) {
        let anchor = if extend { self.sel.anchor } else { cp };
        self.set_selection(Selection { anchor, head: cp }, cx);
    }

    fn restart_blink(&mut self, cx: &mut Context<Self>) {
        self.blink_epoch += 1;
        let epoch = self.blink_epoch;
        self.blink_on = true;
        self._blink = cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(530)).await;
            let keep_going = this.update(cx, |e, cx| {
                if e.blink_epoch != epoch {
                    return false;
                }
                e.blink_on = !e.blink_on;
                cx.notify();
                true
            });
            if !matches!(keep_going, Ok(true)) {
                break;
            }
        });
    }

    fn set_block_at(&self, newline_cp: usize, block: Block) {
        let r = newline_cp..newline_cp + 1;
        let res = if block == Block::Paragraph {
            self.text.unmark(r, "block")
        } else {
            self.text.mark(r, "block", block.as_str())
        };
        if let Err(e) = res {
            tracing::warn!("set block failed: {e}");
        }
    }

    fn delete_cp_range(&self, r: Range<usize>) {
        if !r.is_empty() {
            if let Err(e) = self.text.delete(r.start, r.len()) {
                tracing::warn!("delete failed: {e}");
            }
        }
    }

    fn insert_at(&self, cp: usize, s: &str) {
        if !s.is_empty() {
            if let Err(e) = self.text.insert(cp, s) {
                tracing::warn!("insert failed: {e}");
            }
        }
    }

    fn set_mark(&self, r: Range<usize>, key: &str, on: bool) {
        if r.is_empty() {
            return;
        }
        let res = if on { self.text.mark(r, key, true) } else { self.text.unmark(r, key) };
        if let Err(e) = res {
            tracing::warn!("mark {key} failed: {e}");
        }
    }

    /// Make the inline marks over `r` exactly `want` (links included, comments
    /// left alone), by diffing against what the text currently carries.
    fn apply_marks_in(&self, r: Range<usize>, want: &Marks) {
        let paras = Paragraphs::from_text(&self.text);
        let mut segments: Vec<(Range<usize>, Marks)> = Vec::new();
        for p in paras.iter() {
            if p.end_cp() <= r.start || p.start_cp >= r.end {
                continue;
            }
            let mut cp = p.start_cp;
            for run in &p.runs {
                let n = run.text.chars().count();
                let seg = cp.max(r.start)..(cp + n).min(r.end);
                cp += n;
                if !seg.is_empty() {
                    segments.push((seg, run.marks.clone()));
                }
            }
        }
        for (seg, have) in segments {
            let bools: [(&str, bool, bool); 6] = [
                ("bold", want.bold, have.bold),
                ("italic", want.italic, have.italic),
                ("underline", want.underline, have.underline),
                ("strike", want.strike, have.strike),
                ("smallcaps", want.smallcaps, have.smallcaps),
                ("highlight", want.highlight, have.highlight),
            ];
            for (key, w, h) in bools {
                if w != h {
                    self.set_mark(seg.clone(), key, w);
                }
            }
            if want.link != have.link {
                let res = match &want.link {
                    Some(target) => self.text.mark(seg.clone(), "link", target.as_str()),
                    None => self.text.unmark(seg.clone(), "link"),
                };
                if let Err(e) = res {
                    tracing::warn!("link mark failed: {e}");
                }
            }
        }
    }

    // ----- editing ---------------------------------------------------------

    /// Insert typed or pasted text at the selection, replacing it. A single
    /// typed character goes through smart typography and picks up pending marks.
    pub fn insert_text(&mut self, s: &str, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        let s = s.replace("\r\n", "\n").replace('\r', "\n");
        self.begin_edit();
        let r = self.sel.range();
        self.delete_cp_range(r.clone());
        let mut start = r.start;
        let mut insert = s.clone();
        let mut chars = s.chars();
        if let (Some(ch), None, true) = (chars.next(), chars.next(), self.smart_typography) {
            let pos = self.paras.locate(r.start);
            let before: String = self
                .paras
                .get(pos.para)
                .map(|p| p.text[..pos.byte].chars().rev().take(2).collect::<Vec<_>>())
                .unwrap_or_default()
                .into_iter()
                .rev()
                .collect();
            if let Some((back, repl)) = smart_replace(&before, ch) {
                let back = back.min(pos.cp);
                start = r.start - back;
                self.delete_cp_range(start..r.start);
                insert = repl.to_string();
            }
        }
        self.insert_at(start, &insert);
        let end = start + insert.chars().count();
        if let Some(want) = self.pending.take() {
            self.apply_marks_in(start..end, &want);
        }
        self.sel = Selection::caret(end);
        self.marked = None;
        self.goal_x = None;
        self.commit(cx);
    }

    /// Split the current paragraph at the caret, carrying the block type the way
    /// a writer expects: headings end, quotes continue, breaks move on.
    pub fn new_paragraph(&mut self, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        self.begin_edit();
        let r = self.sel.range();
        if !r.is_empty() {
            self.delete_cp_range(r.clone());
        }
        let paras = if r.is_empty() {
            std::borrow::Cow::Borrowed(&self.paras)
        } else {
            std::borrow::Cow::Owned(Paragraphs::from_text(&self.text))
        };
        let cp = r.start;
        let pos = paras.locate(cp);
        let (block, at_end, old_nl) = match paras.get(pos.para) {
            Some(p) => (p.block, pos.cp >= p.len_cp, p.newline_cp() + 1),
            None => (Block::Paragraph, true, cp + 1),
        };
        drop(paras);
        self.insert_at(cp, "\n");
        let new_nl = cp;
        match block {
            Block::Paragraph => {}
            Block::Quote => self.set_block_at(new_nl, Block::Quote),
            Block::Break => {
                self.set_block_at(new_nl, Block::Break);
                self.set_block_at(old_nl, Block::Paragraph);
            }
            Block::H1 | Block::H2 | Block::H3 => {
                self.set_block_at(new_nl, block);
                if at_end {
                    self.set_block_at(old_nl, Block::Paragraph);
                }
            }
        }
        self.sel = Selection::caret(cp + 1);
        self.marked = None;
        self.goal_x = None;
        self.commit(cx);
    }

    pub fn backspace(&mut self, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        self.begin_edit();
        let r = self.sel.range();
        if !r.is_empty() {
            self.delete_cp_range(r.clone());
            self.sel = Selection::caret(r.start);
            self.commit(cx);
            return;
        }
        let cp = self.sel.head;
        if cp == 0 {
            return;
        }
        let pos = self.paras.locate(cp);
        let cur = self.paras.get(pos.para).cloned();
        if pos.cp == 0 {
            let Some(cur) = cur else { return };
            // Backspace at the start of a styled paragraph first demotes it.
            if cur.block != Block::Paragraph && cur.len_cp > 0 {
                self.set_block_at(cur.newline_cp(), Block::Paragraph);
                self.commit(cx);
                return;
            }
            let prev = self.paras.get(pos.para - 1).cloned();
            let keep = match prev {
                Some(p) if p.block != Block::Break => p.block,
                _ => cur.block,
            };
            self.delete_cp_range(cp - 1..cp);
            self.set_block_at(cur.newline_cp() - 1, keep);
            self.sel = Selection::caret(cp - 1);
        } else {
            let start = self.prev_grapheme(cp);
            self.delete_cp_range(start..cp);
            self.sel = Selection::caret(start);
        }
        self.goal_x = None;
        self.commit(cx);
    }

    pub fn delete_forward(&mut self, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        self.begin_edit();
        let r = self.sel.range();
        if !r.is_empty() {
            self.delete_cp_range(r.clone());
            self.sel = Selection::caret(r.start);
            self.commit(cx);
            return;
        }
        let cp = self.sel.head;
        if cp >= self.paras.max_cursor() {
            return;
        }
        let pos = self.paras.locate(cp);
        let Some(cur) = self.paras.get(pos.para).cloned() else { return };
        if pos.cp >= cur.len_cp {
            let Some(next) = self.paras.get(pos.para + 1).cloned() else { return };
            let keep = if cur.block == Block::Break { next.block } else { cur.block };
            self.delete_cp_range(cp..cp + 1);
            self.set_block_at(next.newline_cp() - 1, keep);
        } else {
            let end = self.next_grapheme(cp);
            self.delete_cp_range(cp..end);
        }
        self.goal_x = None;
        self.commit(cx);
    }

    pub fn delete_word_backward(&mut self, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        if !self.sel.is_empty() {
            return self.backspace(cx);
        }
        let cp = self.sel.head;
        let target = self.word_left(cp);
        if self.paras.locate(target).para != self.paras.locate(cp).para {
            return self.backspace(cx);
        }
        self.begin_edit();
        self.delete_cp_range(target..cp);
        self.sel = Selection::caret(target);
        self.commit(cx);
    }

    pub fn delete_word_forward(&mut self, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        if !self.sel.is_empty() {
            return self.delete_forward(cx);
        }
        let cp = self.sel.head;
        let target = self.word_right(cp);
        if self.paras.locate(target).para != self.paras.locate(cp).para {
            return self.delete_forward(cx);
        }
        self.begin_edit();
        self.delete_cp_range(cp..target);
        self.commit(cx);
    }

    /// Set the block type of every paragraph touched by the selection.
    /// Setting the type a paragraph already has resets it to a plain paragraph.
    pub fn set_block(&mut self, block: Block, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        self.begin_edit();
        let r = self.sel.range();
        let first = self.paras.locate(r.start).para;
        let last = self.paras.locate(r.end).para;
        let all_same = (first..=last)
            .filter_map(|i| self.paras.get(i))
            .all(|p| p.block == block);
        let target = if all_same { Block::Paragraph } else { block };
        for i in first..=last {
            if let Some(p) = self.paras.get(i) {
                self.set_block_at(p.newline_cp(), target);
            }
        }
        self.commit(cx);
    }

    /// Toggle an inline mark (bold/italic/underline/strike/…) over the selection.
    pub fn toggle_mark(&mut self, key: &str, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        let r = self.sel.range();
        if r.is_empty() {
            // Collapsed caret: remember the toggle for the next typed text.
            let mut m = self.current_marks();
            match key {
                "bold" => m.bold = !m.bold,
                "italic" => m.italic = !m.italic,
                "underline" => m.underline = !m.underline,
                "strike" => m.strike = !m.strike,
                "smallcaps" => m.smallcaps = !m.smallcaps,
                "highlight" => m.highlight = !m.highlight,
                _ => return,
            }
            self.pending = Some(m);
            cx.emit(EditorEvent::SelectionChanged);
            cx.notify();
            return;
        }
        self.begin_edit();
        let has = self.range_has_mark(r.clone(), key);
        self.set_mark(r, key, !has);
        self.commit(cx);
    }

    fn range_has_mark(&self, r: Range<usize>, key: &str) -> bool {
        let mut any = false;
        for p in self.paras.iter() {
            if p.end_cp() <= r.start || p.start_cp >= r.end {
                continue;
            }
            let mut cp = p.start_cp;
            for run in &p.runs {
                let n = run.text.chars().count();
                let run_r = cp..cp + n;
                cp += n;
                if run_r.end <= r.start || run_r.start >= r.end {
                    continue;
                }
                any = true;
                let on = match key {
                    "bold" => run.marks.bold,
                    "italic" => run.marks.italic,
                    "underline" => run.marks.underline,
                    "strike" => run.marks.strike,
                    "smallcaps" => run.marks.smallcaps,
                    "highlight" => run.marks.highlight,
                    _ => false,
                };
                if !on {
                    return false;
                }
            }
        }
        any
    }

    /// Insert a scene break after the current paragraph and start a new one.
    pub fn insert_scene_break(&mut self, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        self.begin_edit();
        let r = self.sel.range();
        self.delete_cp_range(r.clone());
        let paras = Paragraphs::from_text(&self.text);
        let cp = r.start;
        let pos = paras.locate(cp);
        let Some(p) = paras.get(pos.para) else { return };
        if p.len_cp == 0 {
            // Empty paragraph: it becomes the break, then a fresh paragraph follows.
            let nl = p.newline_cp();
            self.set_block_at(nl, Block::Break);
            self.insert_at(nl + 1, "\n");
            self.sel = Selection::caret(nl + 1);
        } else {
            // Split here, then slip an empty break paragraph between the halves.
            let block = p.block;
            let nl = p.newline_cp();
            self.insert_at(cp, "\n\n");
            // cp: ends the first half; cp+1: the break; nl+2: the second half.
            if block == Block::Quote {
                self.set_block_at(cp, block);
            } else if block.is_heading() {
                self.set_block_at(cp, block);
                self.set_block_at(nl + 2, Block::Paragraph);
            }
            self.set_block_at(cp + 1, Block::Break);
            self.sel = Selection::caret(cp + 2);
        }
        self.goal_x = None;
        self.commit(cx);
    }

    pub fn undo(&mut self, cx: &mut Context<Self>) {
        *self.undo_sel.lock().unwrap() = (self.sel.anchor, self.sel.head);
        *self.undo_restore.lock().unwrap() = None;
        match self.undo.undo() {
            Ok(true) => {
                self.after_history(cx);
            }
            Ok(false) => {}
            Err(e) => tracing::warn!("undo failed: {e}"),
        }
    }

    pub fn redo(&mut self, cx: &mut Context<Self>) {
        *self.undo_sel.lock().unwrap() = (self.sel.anchor, self.sel.head);
        *self.undo_restore.lock().unwrap() = None;
        match self.undo.redo() {
            Ok(true) => {
                self.after_history(cx);
            }
            Ok(false) => {}
            Err(e) => tracing::warn!("redo failed: {e}"),
        }
    }

    fn after_history(&mut self, cx: &mut Context<Self>) {
        self.marked = None;
        if let Some((a, h)) = self.undo_restore.lock().unwrap().take() {
            self.sel = Selection { anchor: a, head: h };
        }
        self.refresh(cx);
        cx.emit(EditorEvent::Edited);
    }

    pub fn can_undo(&self) -> bool {
        self.undo.can_undo()
    }

    // ----- clipboard -------------------------------------------------------

    /// The selection as a formatted fragment.
    pub fn selected_fragment(&self) -> RichFragment {
        let r = self.sel.range();
        let mut paras = Vec::new();
        for p in self.paras.iter() {
            if p.end_cp() <= r.start || p.start_cp >= r.end {
                continue;
            }
            let mut runs = Vec::new();
            let mut cp = p.start_cp;
            for run in &p.runs {
                let n = run.text.chars().count();
                let seg = cp.max(r.start)..(cp + n).min(r.end);
                if !seg.is_empty() {
                    let text: String = run.text.chars().skip(seg.start - cp).take(seg.len()).collect();
                    let mut marks = run.marks.clone();
                    marks.comment = None;
                    runs.push(Run { text, marks });
                }
                cp += n;
            }
            let terminated = p.terminated && r.end > p.newline_cp();
            paras.push(FragmentPara { block: p.block, runs, terminated });
        }
        RichFragment { paras }
    }

    pub fn copy(&mut self, cx: &mut Context<Self>) {
        if self.sel.is_empty() {
            return;
        }
        let fragment = self.selected_fragment();
        let text = fragment.plain_text();
        cx.set_global(RichClipboard { text: text.clone(), fragment });
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    /// Insert a formatted fragment at the selection, replacing it.
    pub fn paste_fragment(&mut self, frag: &RichFragment, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        self.begin_edit();
        let r = self.sel.range();
        self.delete_cp_range(r.clone());
        let plain = frag.plain_text();
        self.insert_at(r.start, &plain);
        let mut cp = r.start;
        for p in &frag.paras {
            for run in &p.runs {
                let n = run.text.chars().count();
                self.apply_marks_in(cp..cp + n, &run.marks);
                cp += n;
            }
            if p.terminated {
                self.set_block_at(cp, p.block);
                cp += 1;
            }
        }
        self.sel = Selection::caret(cp);
        self.marked = None;
        self.goal_x = None;
        self.commit(cx);
    }

    pub fn cut(&mut self, cx: &mut Context<Self>) {
        if self.sel.is_empty() || self.read_only {
            return;
        }
        self.copy(cx);
        self.begin_edit();
        let r = self.sel.range();
        self.delete_cp_range(r.clone());
        self.sel = Selection::caret(r.start);
        self.commit(cx);
    }

    pub fn paste(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|i| i.text()) else { return };
        let rich = cx
            .try_global::<RichClipboard>()
            .filter(|c| c.text == text)
            .map(|c| c.fragment.clone());
        match rich {
            Some(frag) => self.paste_fragment(&frag, cx),
            None => {
                let text = text.replace("\r\n", "\n").replace('\r', "\n");
                let frag = RichFragment {
                    paras: text
                        .split('\n')
                        .enumerate()
                        .map(|(i, line)| FragmentPara {
                            block: Block::Paragraph,
                            runs: vec![Run { text: line.to_string(), marks: Marks::default() }],
                            terminated: i + 1 < text.split('\n').count(),
                        })
                        .collect(),
                };
                if frag.paras.len() == 1 {
                    self.insert_text(&text, cx);
                } else {
                    self.paste_fragment(&frag, cx);
                }
            }
        }
    }

    // ----- comments --------------------------------------------------------

    /// Attach a new, empty comment to the selection and start editing it.
    pub fn add_comment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        let Some(comments) = self.comments.clone() else { return };
        let mut r = self.sel.range();
        if r.is_empty() {
            r = self.word_range_at(self.sel.head);
        }
        // A comment needs visible text to anchor to; bare newlines leave orphans.
        let anchored: String = self.plain.chars().skip(r.start).take(r.len()).collect();
        if anchored.trim().is_empty() {
            return;
        }
        self.begin_edit();
        let id = match comments.add(self.node_label.as_deref(), "") {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!("add comment: {e:#}");
                return;
            }
        };
        if let Err(e) = self.text.mark(r.clone(), "comment", id.as_str()) {
            tracing::warn!("comment mark failed: {e}");
        }
        self.sel = Selection::caret(r.end);
        self.commit(cx);
        self.begin_comment_edit(&id, window, cx);
    }

    /// Open the inline editor for a comment's text.
    pub fn begin_comment_edit(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(comments) = &self.comments else { return };
        let text = comments.get(id).map(|c| c.text).unwrap_or_default();
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(text)
                .placeholder("Add a note…")
        });
        input.update(cx, |s, cx| s.focus(window, cx));
        let id_owned = id.to_string();
        let sub = cx.subscribe_in(&input, window, move |this, input, ev: &InputEvent, window, cx| match ev {
            InputEvent::Change => {
                let value = input.read(cx).value().to_string();
                if let Some(c) = &this.comments {
                    if let Err(e) = c.set_text(&id_owned, &value) {
                        tracing::warn!("comment text: {e:#}");
                    }
                }
                this.commit_meta(cx);
            }
            InputEvent::PressEnter { .. } => this.end_comment_edit(window, cx),
            _ => {}
        });
        self.comment_edit = Some(CommentEdit { id: id.to_string(), input, _sub: sub });
        cx.notify();
    }

    pub fn end_comment_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.comment_edit.take().is_some() {
            window.focus(&self.focus, cx);
            cx.notify();
        }
    }

    pub fn set_comment_resolved(&mut self, id: &str, resolved: bool, cx: &mut Context<Self>) {
        if let Some(c) = &self.comments {
            if let Err(e) = c.set_resolved(id, resolved) {
                tracing::warn!("resolve comment: {e:#}");
            }
        }
        if self.comment_edit.as_ref().map(|e| e.id.as_str()) == Some(id) {
            self.comment_edit = None;
        }
        self.commit_meta(cx);
    }

    /// Remove the comment and its anchor mark (undoable).
    pub fn delete_comment(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        self.begin_edit();
        let ranges: Vec<Range<usize>> =
            self.comment_anchors_with(true).into_iter().filter(|a| a.id == id).map(|a| a.range).collect();
        for r in ranges {
            if let Err(e) = self.text.unmark(r, "comment") {
                tracing::warn!("unmark comment: {e}");
            }
        }
        if let Some(c) = &self.comments {
            if let Err(e) = c.remove(id) {
                tracing::warn!("remove comment: {e:#}");
            }
        }
        if self.comment_edit.as_ref().map(|e| e.id.as_str()) == Some(id) {
            self.comment_edit = None;
        }
        self.commit(cx);
    }

    pub fn toggle_show_resolved(&mut self, cx: &mut Context<Self>) {
        self.show_resolved = !self.show_resolved;
        cx.notify();
    }

    // ----- find / replace --------------------------------------------------

    /// Set (or clear) the search query; matching is case-insensitive.
    pub fn set_search(&mut self, query: Option<String>, cx: &mut Context<Self>) {
        self.search = query.filter(|q| !q.is_empty());
        self.recompute_matches();
        if self.match_ix.is_none() && !self.matches.is_empty() {
            // Jump to the first match at or after the caret.
            self.search_step(1, true, cx);
        } else {
            cx.notify();
        }
    }

    fn recompute_matches(&mut self) {
        self.matches.clear();
        let Some(q) = &self.search else {
            self.match_ix = None;
            return;
        };
        let hay: Vec<char> = self.plain.chars().flat_map(|c| c.to_lowercase()).collect();
        let needle: Vec<char> = q.chars().flat_map(|c| c.to_lowercase()).collect();
        if needle.is_empty() || hay.len() < needle.len() {
            self.match_ix = None;
            return;
        }
        let mut i = 0;
        while i + needle.len() <= hay.len() {
            if hay[i..i + needle.len()] == needle[..] {
                self.matches.push(i..i + needle.len());
                i += needle.len();
            } else {
                i += 1;
            }
        }
        let sel = self.sel.range();
        self.match_ix = self.matches.iter().position(|m| *m == sel);
    }

    /// Select the next (`dir > 0`) or previous match relative to the caret.
    /// With `inclusive`, a match starting at the caret counts as "next".
    pub fn search_step(&mut self, dir: i32, inclusive: bool, cx: &mut Context<Self>) {
        if self.matches.is_empty() {
            cx.notify();
            return;
        }
        let r = self.sel.range();
        let ix = if dir > 0 {
            let from = if inclusive { r.start } else { r.start + 1 };
            self.matches.iter().position(|m| m.start >= from).unwrap_or(0)
        } else {
            self.matches
                .iter()
                .rposition(|m| m.start < r.start)
                .unwrap_or(self.matches.len() - 1)
        };
        let m = self.matches[ix].clone();
        self.goal_x = None;
        self.set_selection(Selection { anchor: m.start, head: m.end }, cx);
        self.match_ix = Some(ix);
    }

    /// Replace the current match (if the selection is one) and move to the next.
    pub fn replace_current(&mut self, with: &str, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        let r = self.sel.range();
        if self.matches.iter().any(|m| *m == r) {
            self.begin_edit();
            self.delete_cp_range(r.clone());
            self.insert_at(r.start, with);
            self.sel = Selection::caret(r.start + with.chars().count());
            self.commit(cx);
        }
        self.search_step(1, true, cx);
    }

    pub fn replace_all(&mut self, with: &str, cx: &mut Context<Self>) {
        if self.read_only || self.matches.is_empty() {
            return;
        }
        self.begin_edit();
        for m in self.matches.clone().into_iter().rev() {
            self.delete_cp_range(m.clone());
            self.insert_at(m.start, with);
        }
        self.sel = Selection::caret(self.sel.head.min(self.paras.max_cursor()));
        self.commit(cx);
    }

    // ----- movement --------------------------------------------------------

    fn prev_grapheme(&self, cp: usize) -> usize {
        let pos = self.paras.locate(cp);
        if pos.cp == 0 {
            return cp.saturating_sub(1);
        }
        let Some(p) = self.paras.get(pos.para) else { return cp.saturating_sub(1) };
        let before = &p.text[..pos.byte];
        let start = before
            .grapheme_indices(true)
            .last()
            .map(|(b, _)| b)
            .unwrap_or(0);
        self.paras.cp_at(pos.para, start)
    }

    fn next_grapheme(&self, cp: usize) -> usize {
        let pos = self.paras.locate(cp);
        let Some(p) = self.paras.get(pos.para) else { return cp };
        if pos.cp >= p.len_cp {
            return if pos.para + 1 < self.paras.len() { cp + 1 } else { cp };
        }
        let len = p.text[pos.byte..]
            .graphemes(true)
            .next()
            .map(|g| g.len())
            .unwrap_or(0);
        self.paras.cp_at(pos.para, pos.byte + len)
    }

    fn word_left(&self, cp: usize) -> usize {
        let pos = self.paras.locate(cp);
        if pos.cp == 0 {
            return cp.saturating_sub(1);
        }
        let Some(p) = self.paras.get(pos.para) else { return cp };
        let start = p
            .text
            .unicode_word_indices()
            .take_while(|(b, _)| *b < pos.byte)
            .last()
            .map(|(b, _)| b)
            .unwrap_or(0);
        self.paras.cp_at(pos.para, start)
    }

    fn word_right(&self, cp: usize) -> usize {
        let pos = self.paras.locate(cp);
        let Some(p) = self.paras.get(pos.para) else { return cp };
        if pos.cp >= p.len_cp {
            return if pos.para + 1 < self.paras.len() { cp + 1 } else { cp };
        }
        let end = p
            .text
            .unicode_word_indices()
            .map(|(b, w)| b + w.len())
            .find(|e| *e > pos.byte)
            .unwrap_or(p.text.len());
        self.paras.cp_at(pos.para, end)
    }

    fn word_range_at(&self, cp: usize) -> Range<usize> {
        let pos = self.paras.locate(cp);
        let Some(p) = self.paras.get(pos.para) else { return cp..cp };
        for (b, w) in p.text.unicode_word_indices() {
            if b <= pos.byte && pos.byte < b + w.len() {
                return self.paras.cp_at(pos.para, b)..self.paras.cp_at(pos.para, b + w.len());
            }
        }
        cp..self.next_grapheme(cp).max(cp)
    }

    fn paragraph_range_at(&self, cp: usize) -> Range<usize> {
        let pos = self.paras.locate(cp);
        match self.paras.get(pos.para) {
            Some(p) => p.start_cp..p.newline_cp(),
            None => cp..cp,
        }
    }

    fn line_start(&self, cp: usize) -> usize {
        let frame = self.frame.borrow();
        frame
            .as_ref()
            .and_then(|f| f.row_bounds_for_cp(cp, &self.paras))
            .map(|(s, _)| s)
            .unwrap_or_else(|| self.paragraph_range_at(cp).start)
    }

    fn line_end(&self, cp: usize) -> usize {
        let frame = self.frame.borrow();
        frame
            .as_ref()
            .and_then(|f| f.row_bounds_for_cp(cp, &self.paras))
            .map(|(_, e)| e)
            .unwrap_or_else(|| self.paragraph_range_at(cp).end)
    }

    fn vertical(&mut self, dir: i32) -> Option<usize> {
        let frame = self.frame.borrow();
        let f = frame.as_ref()?;
        let (pt, lh) = f.point_for_cp(self.sel.head, &self.paras)?;
        let x = *self.goal_x.get_or_insert(pt.x);
        let y = pt.y + lh * 0.5 + lh * dir as f32;
        Some(f.cp_for_point(point(x, y), &self.paras))
    }

    fn move_horizontal(&mut self, dir: i32, extend: bool, cx: &mut Context<Self>) {
        self.goal_x = None;
        let r = self.sel.range();
        let cp = if !extend && !r.is_empty() {
            if dir < 0 { r.start } else { r.end }
        } else if dir < 0 {
            self.prev_grapheme(self.sel.head)
        } else {
            self.next_grapheme(self.sel.head)
        };
        self.move_to(cp, extend, cx);
    }

    fn move_vertical(&mut self, dir: i32, extend: bool, cx: &mut Context<Self>) {
        let goal = self.goal_x;
        let target = match self.vertical(dir) {
            Some(cp) => cp,
            None => {
                if dir < 0 { 0 } else { self.paras.max_cursor() }
            }
        };
        // If we didn't move (first/last line), jump to the document edge.
        let target = if target == self.sel.head {
            if dir < 0 { 0 } else { self.paras.max_cursor() }
        } else {
            target
        };
        self.move_to(target, extend, cx);
        self.goal_x = goal.or(self.goal_x);
    }

    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        let max = self.paras.max_cursor();
        self.set_selection(Selection { anchor: 0, head: max }, cx);
    }

    // ----- mouse -----------------------------------------------------------

    fn cp_for_window_point(&self, pt: Point<Pixels>) -> Option<usize> {
        let frame = self.frame.borrow();
        frame.as_ref().map(|f| f.cp_for_point(pt, &self.paras))
    }

    pub(crate) fn on_mouse_down(&mut self, ev: &MouseDownEvent, cx: &mut Context<Self>) {
        let Some(cp) = self.cp_for_window_point(ev.position) else { return };
        self.comment_edit = None;
        self.goal_x = None;
        match ev.click_count {
            1 => {
                if ev.modifiers.shift {
                    self.move_to(cp, true, cx);
                } else {
                    self.set_selection(Selection::caret(cp), cx);
                }
                self.selecting = true;
            }
            2 => {
                let r = self.word_range_at(cp);
                self.set_selection(Selection { anchor: r.start, head: r.end }, cx);
            }
            _ => {
                let r = self.paragraph_range_at(cp);
                self.set_selection(Selection { anchor: r.start, head: r.end }, cx);
            }
        }
        self.scroll_to_cursor = false;
    }

    pub(crate) fn on_drag(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if !self.selecting {
            return;
        }
        let Some(cp) = self.cp_for_window_point(position) else { return };
        if cp != self.sel.head {
            self.sel.head = cp.min(self.paras.max_cursor());
            cx.emit(EditorEvent::SelectionChanged);
            cx.notify();
        }
    }

    pub(crate) fn on_scroll(&mut self, delta_y: Pixels, cx: &mut Context<Self>) {
        let max = self
            .frame
            .borrow()
            .as_ref()
            .map(|f| f.max_scroll)
            .unwrap_or(px(0.));
        let new = (self.scroll_y - delta_y).max(px(0.)).min(max);
        if new != self.scroll_y {
            self.scroll_y = new;
            self.scroll_to_cursor = false;
            cx.notify();
        }
    }

    // ----- UTF-16 helpers for the platform IME bridge ---------------------

    fn cp_to_utf16(&self, cp: usize) -> usize {
        self.plain.chars().take(cp).map(|c| c.len_utf16()).sum()
    }

    fn utf16_to_cp(&self, u: usize) -> usize {
        let mut acc = 0;
        for (i, c) in self.plain.chars().enumerate() {
            if acc >= u {
                return i;
            }
            acc += c.len_utf16();
        }
        self.plain.chars().count()
    }

    fn range_cp_to_utf16(&self, r: &Range<usize>) -> Range<usize> {
        self.cp_to_utf16(r.start)..self.cp_to_utf16(r.end)
    }

    fn range_utf16_to_cp(&self, r: &Range<usize>) -> Range<usize> {
        self.utf16_to_cp(r.start)..self.utf16_to_cp(r.end)
    }
}

impl EntityInputHandler for ProseEditor {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let r = self.range_utf16_to_cp(&range_utf16);
        let max = self.plain.chars().count();
        let r = r.start.min(max)..r.end.min(max);
        *adjusted_range = Some(self.range_cp_to_utf16(&r));
        Some(self.plain.chars().skip(r.start).take(r.len()).collect())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let r = self.sel.range();
        Some(UTF16Selection {
            range: self.range_cp_to_utf16(&r),
            reversed: self.sel.head < self.sel.anchor,
        })
    }

    fn marked_text_range(&self, _window: &mut Window, _cx: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|m| self.range_cp_to_utf16(m))
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
        let target = range_utf16
            .map(|r| self.range_utf16_to_cp(&r))
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.sel.range());
        let max = self.paras.max_cursor();
        let target = target.start.min(max)..target.end.min(max);
        // Plain typing (no IME composition in flight) goes through
        // `insert_text` so pending marks and smart typography apply.
        if self.marked.is_none() && target == self.sel.range() {
            self.insert_text(text, cx);
            return;
        }
        self.begin_edit();
        self.delete_cp_range(target.clone());
        self.insert_at(target.start, text);
        self.sel = Selection::caret(target.start + text.chars().count());
        self.marked = None;
        self.goal_x = None;
        self.commit(cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
        let target = range_utf16
            .map(|r| self.range_utf16_to_cp(&r))
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.sel.range());
        let max = self.paras.max_cursor();
        let target = target.start.min(max)..target.end.min(max);
        self.begin_edit();
        self.delete_cp_range(target.clone());
        self.insert_at(target.start, new_text);
        let n = new_text.chars().count();
        self.marked = if n == 0 { None } else { Some(target.start..target.start + n) };
        // The platform gives the selection relative to the marked text, in UTF-16.
        let sel = match new_selected_range_utf16 {
            Some(r) => {
                let s = new_text.encode_utf16().take(r.start).count();
                let e = new_text.encode_utf16().take(r.end).count();
                let to_cp = |u: usize| {
                    let mut acc = 0;
                    let mut i = 0;
                    for c in new_text.chars() {
                        if acc >= u {
                            break;
                        }
                        acc += c.len_utf16();
                        i += 1;
                    }
                    i
                };
                let _ = (s, e);
                Selection { anchor: target.start + to_cp(r.start), head: target.start + to_cp(r.end) }
            }
            None => Selection::caret(target.start + n),
        };
        self.sel = sel;
        self.commit(cx);
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let r = self.range_utf16_to_cp(&range_utf16);
        let frame = self.frame.borrow();
        let f = frame.as_ref()?;
        let (start, lh) = f.point_for_cp(r.start, &self.paras)?;
        let end = f.point_for_cp(r.end, &self.paras).map(|(p, _)| p).unwrap_or(start);
        let width = if end.y == start.y { (end.x - start.x).max(px(2.)) } else { px(2.) };
        Some(Bounds { origin: start, size: size(width, lh) })
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let cp = self.cp_for_window_point(point)?;
        Some(self.cp_to_utf16(cp))
    }

    fn text_length_utf16(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> Option<usize> {
        Some(self.plain.encode_utf16().count())
    }
}

impl ProseEditor {
    /// The floating editor for the active comment, positioned over its margin card.
    fn render_comment_overlay(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let edit = self.comment_edit.as_ref()?;
        let frame = self.frame.borrow();
        let f = frame.as_ref()?;
        // Over the margin card when there is one; otherwise just under the
        // anchored text (narrow windows have no margin).
        let (left, top, width, resolved) = match f.cards.iter().find(|c| c.id == edit.id) {
            Some(card) => (
                card.bounds.origin.x - f.bounds_origin.x,
                card.bounds.origin.y - f.bounds_origin.y,
                card.bounds.size.width,
                card.resolved,
            ),
            None => {
                let anchor = self.comment_anchors_with(true).into_iter().find(|a| a.id == edit.id)?;
                let (pt, lh) = f.point_for_cp(anchor.range.start, &self.paras)?;
                let width = px(320.).min(f.wrap_width);
                let left = (pt.x - f.bounds_origin.x).min(f.origin.x + f.wrap_width - width - f.bounds_origin.x);
                (left.max(px(0.)), pt.y + lh + px(4.) - f.bounds_origin.y, width, anchor.resolved)
            }
        };
        let id = edit.id.clone();
        let theme = cx.theme();
        let (bg, border, muted) = (theme.popover, theme.border, theme.muted_foreground);
        let id_resolve = id.clone();
        let id_delete = id.clone();
        Some(
            v_flex()
                .id("comment-overlay")
                .occlude()
                // Enter in the note input must not also split the paragraph.
                .on_action(|_: &NewParagraph, _, _| {})
                .absolute()
                .left(left)
                .top(top)
                .w(width)
                .p_2()
                .gap_1()
                .rounded_md()
                .bg(bg)
                .border_1()
                .border_color(border)
                .shadow_md()
                .child(Input::new(&edit.input).small())
                .child(
                    h_flex()
                        .gap_1()
                        .justify_between()
                        .text_xs()
                        .text_color(muted)
                        .child(if resolved { "Resolved" } else { "" })
                        .child(
                            h_flex()
                                .gap_1()
                                .child(
                                    Button::new("comment-resolve")
                                        .ghost()
                                        .xsmall()
                                        .label(if resolved { "Reopen" } else { "Resolve" })
                                        .on_click(cx.listener(move |e, _, _, cx| {
                                            e.set_comment_resolved(&id_resolve, !resolved, cx)
                                        })),
                                )
                                .child(
                                    Button::new("comment-delete")
                                        .ghost()
                                        .xsmall()
                                        .label("Delete")
                                        .on_click(cx.listener(move |e, _, _, cx| e.delete_comment(&id_delete, cx))),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}

impl Render for ProseEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let overlay = self.render_comment_overlay(cx);
        div()
            .id("prose-editor")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .on_action(cx.listener(|e, _: &Backspace, _, cx| e.backspace(cx)))
            .on_action(cx.listener(|e, _: &Delete, _, cx| e.delete_forward(cx)))
            .on_action(cx.listener(|e, _: &DeleteWordBackward, _, cx| e.delete_word_backward(cx)))
            .on_action(cx.listener(|e, _: &DeleteWordForward, _, cx| e.delete_word_forward(cx)))
            .on_action(cx.listener(|e, _: &NewParagraph, _, cx| e.new_paragraph(cx)))
            .on_action(cx.listener(|e, _: &MoveLeft, _, cx| e.move_horizontal(-1, false, cx)))
            .on_action(cx.listener(|e, _: &MoveRight, _, cx| e.move_horizontal(1, false, cx)))
            .on_action(cx.listener(|e, _: &SelectLeft, _, cx| e.move_horizontal(-1, true, cx)))
            .on_action(cx.listener(|e, _: &SelectRight, _, cx| e.move_horizontal(1, true, cx)))
            .on_action(cx.listener(|e, _: &MoveUp, _, cx| e.move_vertical(-1, false, cx)))
            .on_action(cx.listener(|e, _: &MoveDown, _, cx| e.move_vertical(1, false, cx)))
            .on_action(cx.listener(|e, _: &SelectUp, _, cx| e.move_vertical(-1, true, cx)))
            .on_action(cx.listener(|e, _: &SelectDown, _, cx| e.move_vertical(1, true, cx)))
            .on_action(cx.listener(|e, _: &MoveWordLeft, _, cx| {
                e.goal_x = None;
                let cp = e.word_left(e.sel.head);
                e.move_to(cp, false, cx)
            }))
            .on_action(cx.listener(|e, _: &MoveWordRight, _, cx| {
                e.goal_x = None;
                let cp = e.word_right(e.sel.head);
                e.move_to(cp, false, cx)
            }))
            .on_action(cx.listener(|e, _: &SelectWordLeft, _, cx| {
                let cp = e.word_left(e.sel.head);
                e.move_to(cp, true, cx)
            }))
            .on_action(cx.listener(|e, _: &SelectWordRight, _, cx| {
                let cp = e.word_right(e.sel.head);
                e.move_to(cp, true, cx)
            }))
            .on_action(cx.listener(|e, _: &MoveLineStart, _, cx| {
                e.goal_x = None;
                let cp = e.line_start(e.sel.head);
                e.move_to(cp, false, cx)
            }))
            .on_action(cx.listener(|e, _: &MoveLineEnd, _, cx| {
                e.goal_x = None;
                let cp = e.line_end(e.sel.head);
                e.move_to(cp, false, cx)
            }))
            .on_action(cx.listener(|e, _: &SelectLineStart, _, cx| {
                let cp = e.line_start(e.sel.head);
                e.move_to(cp, true, cx)
            }))
            .on_action(cx.listener(|e, _: &SelectLineEnd, _, cx| {
                let cp = e.line_end(e.sel.head);
                e.move_to(cp, true, cx)
            }))
            .on_action(cx.listener(|e, _: &MoveDocStart, _, cx| e.move_to(0, false, cx)))
            .on_action(cx.listener(|e, _: &MoveDocEnd, _, cx| {
                let m = e.paras.max_cursor();
                e.move_to(m, false, cx)
            }))
            .on_action(cx.listener(|e, _: &SelectDocStart, _, cx| e.move_to(0, true, cx)))
            .on_action(cx.listener(|e, _: &SelectDocEnd, _, cx| {
                let m = e.paras.max_cursor();
                e.move_to(m, true, cx)
            }))
            .on_action(cx.listener(|e, _: &SelectAll, _, cx| e.select_all(cx)))
            .on_action(cx.listener(|e, _: &Undo, _, cx| e.undo(cx)))
            .on_action(cx.listener(|e, _: &Redo, _, cx| e.redo(cx)))
            .on_action(cx.listener(|e, _: &Copy, _, cx| e.copy(cx)))
            .on_action(cx.listener(|e, _: &Cut, _, cx| e.cut(cx)))
            .on_action(cx.listener(|e, _: &Paste, _, cx| e.paste(cx)))
            .on_action(cx.listener(|e, _: &ToggleBold, _, cx| e.toggle_mark("bold", cx)))
            .on_action(cx.listener(|e, _: &ToggleItalic, _, cx| e.toggle_mark("italic", cx)))
            .on_action(cx.listener(|e, _: &ToggleUnderline, _, cx| e.toggle_mark("underline", cx)))
            .on_action(cx.listener(|e, _: &ToggleStrike, _, cx| e.toggle_mark("strike", cx)))
            .on_action(cx.listener(|e, _: &ToggleSmallCaps, _, cx| e.toggle_mark("smallcaps", cx)))
            .on_action(cx.listener(|e, _: &ToggleHighlight, _, cx| e.toggle_mark("highlight", cx)))
            .on_action(cx.listener(|e, _: &AddComment, window, cx| e.add_comment(window, cx)))
            .on_action(cx.listener(|e, _: &ToggleResolvedComments, _, cx| e.toggle_show_resolved(cx)))
            .on_action(cx.listener(|e, _: &Cancel, window, cx| {
                if e.comment_edit.is_some() {
                    e.end_comment_edit(window, cx);
                } else {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|e, _: &EditCommentAtCaret, window, cx| {
                if let Some(id) = e.active_comment() {
                    e.begin_comment_edit(&id, window, cx)
                }
            }))
            .on_action(cx.listener(|e, _: &SetParagraph, _, cx| e.set_block(Block::Paragraph, cx)))
            .on_action(cx.listener(|e, _: &SetHeading1, _, cx| e.set_block(Block::H1, cx)))
            .on_action(cx.listener(|e, _: &SetHeading2, _, cx| e.set_block(Block::H2, cx)))
            .on_action(cx.listener(|e, _: &SetHeading3, _, cx| e.set_block(Block::H3, cx)))
            .on_action(cx.listener(|e, _: &SetQuote, _, cx| e.set_block(Block::Quote, cx)))
            .on_action(cx.listener(|e, _: &InsertSceneBreak, _, cx| e.insert_scene_break(cx)))
            .child(ProseElement::new(cx.entity().clone()))
            .children(overlay)
    }
}
