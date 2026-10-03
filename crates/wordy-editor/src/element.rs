//! `ProseElement`: lays out, paints, and hit-tests a `ProseEditor`.
//!
//! Each paragraph is shaped as one wrapped line (gpui caches shaping per
//! frame, so re-shaping unchanged paragraphs is cheap). The resulting
//! geometry is published to the editor as a `FrameLayout` so keyboard
//! movement, mouse clicks, and the IME bridge can all hit-test against the
//! same numbers the painter used.

use std::sync::Arc;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::*;
use wordy_doc::{Block, Paragraph, Paragraphs};

use crate::editor::ProseEditor;
use crate::style::EditorStyle;

pub(crate) struct ParaLayout {
    /// Top of the paragraph relative to the content origin.
    pub y: Pixels,
    pub height: Pixels,
    pub line_height: Pixels,
    pub indent: Pixels,
    pub line: WrappedLine,
    pub align: TextAlign,
}

impl ParaLayout {
    /// Byte offsets at which each wrapped row starts.
    pub fn row_starts(&self) -> Vec<usize> {
        let mut rows = vec![0usize];
        let layout = &self.line.unwrapped_layout;
        for b in self.line.wrap_boundaries.iter() {
            if let Some(glyph) = layout
                .runs
                .get(b.run_ix)
                .and_then(|r| r.glyphs.get(b.glyph_ix))
            {
                rows.push(glyph.index);
            }
        }
        rows
    }
}

/// A comment laid out in the right margin.
pub(crate) struct CommentCard {
    pub id: String,
    /// Window-space bounds (already scrolled).
    pub bounds: Bounds<Pixels>,
    pub resolved: bool,
    pub empty: bool,
    line: WrappedLine,
    line_height: Pixels,
}

pub(crate) struct FrameLayout {
    /// Window-space origin of the element.
    pub bounds_origin: Point<Pixels>,
    /// Window-space origin of the text column (already scrolled).
    pub origin: Point<Pixels>,
    #[allow(dead_code)]
    pub wrap_width: Pixels,
    pub paras: Vec<ParaLayout>,
    pub max_scroll: Pixels,
    pub cards: Vec<CommentCard>,
}

impl FrameLayout {
    /// Window position and line height of the caret for a code-point offset.
    pub fn point_for_cp(&self, cp: usize, paras: &Paragraphs) -> Option<(Point<Pixels>, Pixels)> {
        let pos = paras.locate(cp);
        let pl = self.paras.get(pos.para)?;
        let p = pl.line.position_for_index(pos.byte, pl.line_height)?;
        Some((
            point(self.origin.x + pl.indent + p.x, self.origin.y + pl.y + p.y),
            pl.line_height,
        ))
    }

    /// The code-point offset nearest a window point.
    pub fn cp_for_point(&self, pt: Point<Pixels>, paras: &Paragraphs) -> usize {
        if self.paras.is_empty() {
            return 0;
        }
        let y = pt.y - self.origin.y;
        if y < px(0.) {
            return 0;
        }
        let ix = self
            .paras
            .iter()
            .position(|pl| y < pl.y + pl.height)
            .unwrap_or(self.paras.len() - 1);
        let pl = &self.paras[ix];
        let local_y = (y - pl.y).max(px(0.)).min(pl.height - px(1.));
        let local = point(pt.x - self.origin.x - pl.indent, local_y);
        let byte = match pl.line.closest_index_for_position(local, pl.line_height) {
            Ok(b) | Err(b) => b,
        };
        paras.cp_at(ix, byte)
    }

    /// Code-point range of the wrapped row containing `cp`.
    pub fn row_bounds_for_cp(&self, cp: usize, paras: &Paragraphs) -> Option<(usize, usize)> {
        let pos = paras.locate(cp);
        let pl = self.paras.get(pos.para)?;
        let para = paras.get(pos.para)?;
        let rows = pl.row_starts();
        let mut start = 0;
        let mut end = para.text.len();
        for (i, rs) in rows.iter().enumerate() {
            if pos.byte >= *rs {
                start = *rs;
                end = rows.get(i + 1).copied().unwrap_or(para.text.len());
            }
        }
        // Row end excludes the trailing space that the wrap swallowed.
        let mut end_b = end;
        if end < para.text.len() {
            while end_b > start && para.text[..end_b].ends_with(' ') {
                end_b -= 1;
            }
        }
        Some((paras.cp_at(pos.para, start), paras.cp_at(pos.para, end_b)))
    }
}

struct Palette {
    fg: Hsla,
    muted: Hsla,
    link: Hsla,
    selection: Hsla,
    caret: Hsla,
    highlight: Hsla,
    comment: Hsla,
    comment_active: Hsla,
    search: Hsla,
    search_active: Hsla,
    card_bg: Hsla,
    card_border: Hsla,
    accent: Hsla,
}

impl Palette {
    fn from_theme(cx: &App) -> Self {
        let t = cx.theme();
        Palette {
            fg: t.foreground,
            muted: t.muted_foreground,
            link: t.link,
            selection: t.selection,
            caret: t.caret,
            highlight: hsla(0.14, 0.9, 0.6, 0.45),
            comment: hsla(0.08, 0.9, 0.6, 0.22),
            comment_active: hsla(0.08, 0.9, 0.55, 0.5),
            search: hsla(0.14, 0.9, 0.55, 0.3),
            search_active: hsla(0.08, 0.95, 0.5, 0.6),
            card_bg: t.popover,
            card_border: t.border,
            accent: t.primary,
        }
    }
}

const CARD_MIN_MARGIN: Pixels = px(140.);
const CARD_MAX_WIDTH: Pixels = px(240.);
const CARD_PAD: Pixels = px(8.);
const CARD_GAP: Pixels = px(6.);
const CARD_FONT: Pixels = px(12.);

/// Paint a code-point range as row-by-row quads.
fn paint_cp_range(
    window: &mut Window,
    frame: &FrameLayout,
    paras: &Paragraphs,
    r: std::ops::Range<usize>,
    color: Hsla,
    newline_width: Pixels,
    (top, bottom): (Pixels, Pixels),
) {
    if r.is_empty() {
        return;
    }
    for (ix, para) in paras.iter().enumerate() {
        if para.end_cp() <= r.start || para.start_cp >= r.end {
            continue;
        }
        let Some(pl) = frame.paras.get(ix) else { continue };
        let para_top = frame.origin.y + pl.y;
        if para_top + pl.height < top || para_top > bottom {
            continue;
        }
        let sa = para.cp_to_byte(r.start.max(para.start_cp) - para.start_cp);
        let sb = para.cp_to_byte(r.end.min(para.newline_cp()) - para.start_cp);
        let includes_newline = r.end > para.newline_cp();
        let rows = pl.row_starts();
        let layout = &pl.line.unwrapped_layout;
        for (ri, rs) in rows.iter().enumerate() {
            let re = rows.get(ri + 1).copied().unwrap_or(para.text.len());
            let seg_s = sa.max(*rs);
            let seg_e = sb.min(re);
            if seg_s > seg_e {
                continue;
            }
            let last_row = ri + 1 == rows.len();
            if seg_s == seg_e && !(includes_newline && last_row) {
                continue;
            }
            let row_x = layout.x_for_index(*rs);
            let x1 = layout.x_for_index(seg_s) - row_x;
            let mut x2 = layout.x_for_index(seg_e) - row_x;
            if includes_newline && last_row {
                x2 += newline_width;
            }
            let rect = Bounds {
                origin: point(frame.origin.x + pl.indent + x1, para_top + pl.line_height * ri as f32),
                size: size(x2 - x1, pl.line_height),
            };
            window.paint_quad(fill(rect, color));
        }
    }
}

pub struct ProseElement {
    editor: Entity<ProseEditor>,
}

impl ProseElement {
    pub fn new(editor: Entity<ProseEditor>) -> Self {
        Self { editor }
    }

    fn runs_for(
        para: &Paragraph,
        style: &EditorStyle,
        pal: &Palette,
        active_comment: Option<&str>,
        show_resolved_comments: &dyn Fn(&str) -> bool,
    ) -> (SharedString, Vec<TextRun>) {
        let family = style.font_family.clone();
        let base_weight = if para.block.is_heading() { FontWeight::BOLD } else { FontWeight::NORMAL };
        let base_italic = para.block == Block::Quote;

        if para.block == Block::Break {
            let s: SharedString = "* * *".into();
            let run = TextRun {
                len: s.len(),
                font: Font { family, features: FontFeatures::default(), fallbacks: None, weight: FontWeight::NORMAL, style: FontStyle::Normal },
                color: pal.muted,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            return (s, vec![run]);
        }

        let mut runs = Vec::with_capacity(para.runs.len());
        for r in &para.runs {
            let m = &r.marks;
            let weight = if m.bold { FontWeight::BOLD } else { base_weight };
            let italic = m.italic ^ base_italic;
            let color = if m.link.is_some() { pal.link } else if para.block == Block::Quote { pal.muted } else { pal.fg };
            let features = if m.smallcaps {
                FontFeatures(Arc::new(vec![("smcp".to_string(), 1)]))
            } else {
                FontFeatures::default()
            };
            let background_color = if m.highlight {
                Some(pal.highlight)
            } else {
                match &m.comment {
                    Some(id) if active_comment == Some(id.as_str()) => Some(pal.comment_active),
                    Some(id) if show_resolved_comments(id) => Some(pal.comment),
                    _ => None,
                }
            };
            runs.push(TextRun {
                len: r.text.len(),
                font: Font {
                    family: family.clone(),
                    features,
                    fallbacks: None,
                    weight,
                    style: if italic { FontStyle::Italic } else { FontStyle::Normal },
                },
                color,
                background_color,
                underline: (m.underline || m.link.is_some()).then_some(UnderlineStyle {
                    thickness: px(1.),
                    color: Some(color),
                    wavy: false,
                }),
                strikethrough: m.strike.then_some(StrikethroughStyle { thickness: px(1.), color: Some(color) }),
            });
        }
        (SharedString::from(para.text.clone()), runs)
    }
}

impl IntoElement for ProseElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

pub struct ProsePrepaint {
    hitbox: Hitbox,
}

impl Element for ProseElement {
    type RequestLayoutState = ();
    type PrepaintState = ProsePrepaint;

    fn id(&self) -> Option<ElementId> {
        Some(ElementId::Name("prose-element".into()))
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        style.flex_grow = 1.0;
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> ProsePrepaint {
        let pal = Palette::from_theme(cx);

        let (style, scroll_y, want_scroll, head, frame_cell, active_comment, anchors) = {
            let e = self.editor.read(cx);
            (
                e.style.clone(),
                e.scroll_y,
                e.scroll_to_cursor,
                e.selection().head,
                e.frame.clone(),
                e.active_comment(),
                e.comment_anchors(),
            )
        };
        let paras = self.editor.read(cx).paragraphs().clone();
        let visible_comment = |id: &str| anchors.iter().any(|a| a.id == id);

        let wrap_width = (bounds.size.width - style.padding_x * 2.)
            .min(style.max_width)
            .max(px(120.));
        let column_x = bounds.origin.x + (bounds.size.width - wrap_width) * 0.5;

        let text_system = window.text_system().clone();
        let mut layouts: Vec<ParaLayout> = Vec::with_capacity(paras.len());
        let mut y = px(0.);
        let mut prev: Option<Block> = None;
        for para in paras.iter() {
            let font_size = style.font_size_for(para.block);
            let line_height = style.line_height_for(para.block);
            let (text, runs) = Self::runs_for(para, &style, &pal, active_comment.as_deref(), &visible_comment);
            y += style.space_before(para.block, prev);
            let line = text_system
                .shape_text(text, font_size, &runs, Some(wrap_width), None)
                .ok()
                .and_then(|mut v| v.pop())
                .unwrap_or_default();
            let rows = line.wrap_boundaries.len() + 1;
            let height = line_height * rows as f32;
            let (indent, align) = match para.block {
                Block::Break => ((wrap_width - line.width()) * 0.5, TextAlign::Left),
                Block::Quote => (px(32.), TextAlign::Left),
                _ => (px(0.), TextAlign::Left),
            };
            layouts.push(ParaLayout { y, height, line_height, indent, line, align });
            y += height;
            prev = Some(para.block);
        }
        let text_height = y;
        let content_height = style.padding_top + text_height + bounds.size.height * 0.5;
        let max_scroll = (content_height - bounds.size.height).max(px(0.));

        let mut scroll_y = scroll_y.max(px(0.)).min(max_scroll);
        if want_scroll {
            let pos = paras.locate(head);
            if let Some(pl) = layouts.get(pos.para) {
                if let Some(p) = pl.line.position_for_index(pos.byte, pl.line_height) {
                    let caret_top = style.padding_top + pl.y + p.y;
                    let caret_bottom = caret_top + pl.line_height;
                    let margin = pl.line_height * 1.5;
                    let view_h = bounds.size.height;
                    if caret_top - scroll_y < margin {
                        scroll_y = (caret_top - margin).max(px(0.));
                    } else if caret_bottom - scroll_y > view_h - margin {
                        scroll_y = (caret_bottom - view_h + margin).min(max_scroll);
                    }
                }
            }
        }

        let origin = point(column_x, bounds.origin.y + style.padding_top - scroll_y);

        // Comment cards in the right margin, stacked so they never overlap.
        let mut cards = Vec::new();
        let avail = bounds.origin.x + bounds.size.width - (column_x + wrap_width) - px(24.);
        if !anchors.is_empty() && avail >= CARD_MIN_MARGIN {
            let card_w = CARD_MAX_WIDTH.min(avail - px(8.));
            let card_x = column_x + wrap_width + px(16.);
            let ui_font = window.text_style().font();
            let line_height = CARD_FONT * 1.4;
            let mut next_y = origin.y;
            let frame_tmp = FrameLayout {
                bounds_origin: bounds.origin,
                origin,
                wrap_width,
                paras: layouts,
                max_scroll,
                cards: Vec::new(),
            };
            for a in &anchors {
                let anchor_y = frame_tmp
                    .point_for_cp(a.range.start, &paras)
                    .map(|(p, _)| p.y)
                    .unwrap_or(next_y);
                let empty = a.text.trim().is_empty();
                let text: SharedString = if empty { "Add a note…".into() } else { a.text.clone().into() };
                let run = TextRun {
                    len: text.len(),
                    font: ui_font.clone(),
                    color: if empty || a.resolved { pal.muted } else { pal.fg },
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let line = text_system
                    .shape_text(text, CARD_FONT, &[run], Some(card_w - CARD_PAD * 2.), None)
                    .ok()
                    .and_then(|mut v| v.pop())
                    .unwrap_or_default();
                let rows = line.wrap_boundaries.len() + 1;
                let h = line_height * rows as f32 + CARD_PAD * 2.;
                let y = anchor_y.max(next_y);
                cards.push(CommentCard {
                    id: a.id.clone(),
                    bounds: Bounds { origin: point(card_x, y), size: size(card_w, h) },
                    resolved: a.resolved,
                    empty,
                    line,
                    line_height,
                });
                next_y = y + h + CARD_GAP;
            }
            layouts = frame_tmp.paras;
        }

        let frame = FrameLayout {
            bounds_origin: bounds.origin,
            origin,
            wrap_width,
            paras: layouts,
            max_scroll,
            cards,
        };
        *frame_cell.borrow_mut() = Some(frame);
        self.editor.update(cx, |e, _| {
            e.scroll_y = scroll_y;
            e.scroll_to_cursor = false;
        });

        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        ProsePrepaint { hitbox }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        prepaint: &mut ProsePrepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let pal = Palette::from_theme(cx);

        let (focus, sel, blink_on, frame_cell, paras, font_size, matches, match_ix, active_comment, editing) = {
            let e = self.editor.read(cx);
            (
                e.focus.clone(),
                e.selection(),
                e.blink_on,
                e.frame.clone(),
                e.paragraphs().clone(),
                e.style.font_size,
                e.matches.clone(),
                e.match_ix,
                e.active_comment(),
                e.is_editing_comment(),
            )
        };
        let focused = focus.is_focused(window);

        window.handle_input(&focus, ElementInputHandler::new(bounds, self.editor.clone()), cx);
        window.set_cursor_style(CursorStyle::IBeam, &prepaint.hitbox);

        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            let frame = frame_cell.borrow();
            let Some(frame) = frame.as_ref() else { return };
            let top = bounds.origin.y;
            let bottom = bounds.origin.y + bounds.size.height;

            let nl_w = font_size * 0.35;

            // Search matches, then the selection on top.
            for (i, m) in matches.iter().enumerate() {
                let color = if Some(i) == match_ix { pal.search_active } else { pal.search };
                paint_cp_range(window, frame, &paras, m.clone(), color, nl_w, (top, bottom));
            }
            let on_current_match = match_ix.and_then(|i| matches.get(i)).is_some_and(|m| *m == sel.range());
            if !on_current_match {
                paint_cp_range(window, frame, &paras, sel.range(), pal.selection, nl_w, (top, bottom));
            }

            // Text.
            for pl in frame.paras.iter() {
                let para_top = frame.origin.y + pl.y;
                if para_top + pl.height < top || para_top > bottom {
                    continue;
                }
                let origin = point(frame.origin.x + pl.indent, para_top);
                let _ = pl.line.paint_background(origin, pl.line_height, pl.align, None, window, cx);
                let _ = pl.line.paint(origin, pl.line_height, pl.align, None, window, cx);
            }

            // Caret.
            if focused && blink_on && !editing {
                if let Some((pt, lh)) = frame.point_for_cp(sel.head, &paras) {
                    let rect = Bounds { origin: point(pt.x - px(1.), pt.y), size: size(px(2.), lh) };
                    window.paint_quad(fill(rect, pal.caret));
                }
            }

            // Comment cards.
            for card in frame.cards.iter() {
                let b = card.bounds;
                if b.origin.y + b.size.height < top || b.origin.y > bottom {
                    continue;
                }
                let active = active_comment.as_deref() == Some(card.id.as_str());
                if active && editing {
                    continue; // the overlay editor covers it
                }
                let border = if active { pal.accent } else { pal.card_border };
                window.paint_quad(fill(b, pal.card_bg).corner_radii(px(6.)).border_widths(px(1.)).border_color(border));
                let origin = point(b.origin.x + CARD_PAD, b.origin.y + CARD_PAD);
                let _ = card.line.paint(origin, card.line_height, TextAlign::Left, None, window, cx);
                let _ = card.empty;
            }
        });

        // Mouse.
        let hitbox = prepaint.hitbox.clone();
        let editor = self.editor.clone();
        let focus_for_down = focus.clone();
        let frame_for_down = frame_cell.clone();
        window.on_mouse_event(move |ev: &MouseDownEvent, phase, window, cx| {
            if !phase.bubble() || ev.button != MouseButton::Left || !hitbox.is_hovered(window) {
                return;
            }
            let card = frame_for_down
                .borrow()
                .as_ref()
                .and_then(|f| f.cards.iter().find(|c| c.bounds.contains(&ev.position)).map(|c| c.id.clone()));
            match card {
                Some(id) => editor.update(cx, |e, cx| e.begin_comment_edit(&id, window, cx)),
                None => {
                    window.focus(&focus_for_down, cx);
                    editor.update(cx, |e, cx| e.on_mouse_down(ev, cx));
                }
            }
            cx.stop_propagation();
        });
        let editor = self.editor.clone();
        window.on_mouse_event(move |ev: &MouseMoveEvent, phase, _window, cx| {
            if !phase.bubble() || ev.pressed_button != Some(MouseButton::Left) {
                return;
            }
            editor.update(cx, |e, cx| e.on_drag(ev.position, cx));
        });
        let editor = self.editor.clone();
        window.on_mouse_event(move |_: &MouseUpEvent, phase, _window, cx| {
            if !phase.bubble() {
                return;
            }
            editor.update(cx, |e, _| e.selecting = false);
        });
        let hitbox = prepaint.hitbox.clone();
        let editor = self.editor.clone();
        let line_height = self.editor.read(cx).style.body_line_height();
        window.on_mouse_event(move |ev: &ScrollWheelEvent, phase, window, cx| {
            if !phase.bubble() || !hitbox.is_hovered(window) {
                return;
            }
            let delta = ev.delta.pixel_delta(line_height);
            editor.update(cx, |e, cx| e.on_scroll(delta.y, cx));
            cx.stop_propagation();
        });
    }
}
