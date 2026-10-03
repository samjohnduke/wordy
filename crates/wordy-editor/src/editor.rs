//! `ProseEditor`: editing state and behavior for one rich-text body.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_kit::prelude::*;
use gpui_kit::*;
use unicode_segmentation::UnicodeSegmentation;
use wordy_doc::loro::{LoroDoc, LoroText, LoroValue, UndoItemMeta, UndoManager};
use wordy_doc::{Block, Marks, Paragraphs};

use crate::element::{FrameLayout, ProseElement};
use crate::style::EditorStyle;
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
        let mut undo = UndoManager::new(&doc);
        undo.set_merge_interval(700);
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
        };
        this.restart_blink(cx);
        this
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

    /// Marks at the selection head (inherited from the preceding character).
    pub fn current_marks(&self) -> Marks {
        let pos = self.paras.locate(self.sel.head);
        self.paras
            .get(pos.para)
            .map(|p| p.marks_before(pos.cp))
            .unwrap_or_default()
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
        self.doc.commit();
        self.refresh(cx);
        cx.emit(EditorEvent::Edited);
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
        self.scroll_to_cursor = true;
        self.restart_blink(cx);
        cx.notify();
    }

    fn set_selection(&mut self, sel: Selection, cx: &mut Context<Self>) {
        let max = self.paras.max_cursor();
        let sel = Selection { anchor: sel.anchor.min(max), head: sel.head.min(max) };
        if sel != self.sel {
            self.sel = sel;
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

    // ----- editing ---------------------------------------------------------

    /// Insert typed or pasted text at the selection, replacing it.
    pub fn insert_text(&mut self, s: &str, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        let s = s.replace("\r\n", "\n").replace('\r', "\n");
        self.begin_edit();
        let r = self.sel.range();
        self.delete_cp_range(r.clone());
        self.insert_at(r.start, &s);
        self.sel = Selection::caret(r.start + s.chars().count());
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
            return; // pending marks for a caret arrive with Phase 2
        }
        self.begin_edit();
        let has = self.range_has_mark(r.clone(), key);
        let res = if has {
            self.text.unmark(r, key)
        } else {
            self.text.mark(r, key, true)
        };
        if let Err(e) = res {
            tracing::warn!("toggle mark failed: {e}");
        }
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

    pub fn copy(&mut self, cx: &mut Context<Self>) {
        if self.sel.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(self.selected_text()));
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
        if let Some(text) = cx.read_from_clipboard().and_then(|i| i.text()) {
            self.insert_text(&text, cx);
        }
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
        if text.contains('\n') && target == self.sel.range() {
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

impl Render for ProseEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("prose-editor")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
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
            .on_action(cx.listener(|e, _: &SetParagraph, _, cx| e.set_block(Block::Paragraph, cx)))
            .on_action(cx.listener(|e, _: &SetHeading1, _, cx| e.set_block(Block::H1, cx)))
            .on_action(cx.listener(|e, _: &SetHeading2, _, cx| e.set_block(Block::H2, cx)))
            .on_action(cx.listener(|e, _: &SetHeading3, _, cx| e.set_block(Block::H3, cx)))
            .on_action(cx.listener(|e, _: &SetQuote, _, cx| e.set_block(Block::Quote, cx)))
            .on_action(cx.listener(|e, _: &InsertSceneBreak, _, cx| e.insert_scene_break(cx)))
            .child(ProseElement::new(cx.entity().clone()))
    }
}
