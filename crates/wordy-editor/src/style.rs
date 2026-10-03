//! Visual parameters for the prose editor. Colors come from the theme at
//! render time; these are the typographic knobs.

use gpui_kit::{px, Pixels, SharedString};
use wordy_doc::Block;

#[derive(Clone, Debug)]
pub struct EditorStyle {
    pub font_family: SharedString,
    pub font_size: Pixels,
    /// Line height as a multiple of the font size for body text.
    pub line_height: f32,
    /// Maximum measure (text column width).
    pub max_width: Pixels,
    pub padding_x: Pixels,
    pub padding_top: Pixels,
    /// Space after a paragraph, as a multiple of the body line height.
    pub paragraph_spacing: f32,
    /// Indent for the first line of body paragraphs that follow another body paragraph.
    pub first_line_indent: Pixels,
}

impl Default for EditorStyle {
    fn default() -> Self {
        Self {
            font_family: "Libertinus Serif".into(),
            font_size: px(19.),
            line_height: 1.65,
            max_width: px(680.),
            padding_x: px(48.),
            padding_top: px(56.),
            paragraph_spacing: 0.0,
            first_line_indent: px(28.),
        }
    }
}

impl EditorStyle {
    pub fn font_size_for(&self, block: Block) -> Pixels {
        let scale = match block {
            Block::H1 => 1.7,
            Block::H2 => 1.4,
            Block::H3 => 1.15,
            _ => 1.0,
        };
        self.font_size * scale
    }

    pub fn line_height_for(&self, block: Block) -> Pixels {
        let f = match block {
            Block::H1 | Block::H2 | Block::H3 => 1.3,
            _ => self.line_height,
        };
        (self.font_size_for(block) * f).round()
    }

    pub fn body_line_height(&self) -> Pixels {
        self.line_height_for(Block::Paragraph)
    }

    /// Space before a paragraph of this block type.
    pub fn space_before(&self, block: Block, prev: Option<Block>) -> Pixels {
        let lh = self.body_line_height();
        match block {
            Block::H1 => lh * if prev.is_some() { 1.5 } else { 0.0 },
            Block::H2 | Block::H3 => lh * if prev.is_some() { 1.0 } else { 0.0 },
            Block::Break => lh * 0.5,
            Block::Quote => lh * 0.25,
            Block::Paragraph => match prev {
                Some(Block::Break) => lh * 0.5,
                Some(Block::Quote) => lh * 0.25,
                Some(Block::H1 | Block::H2 | Block::H3) => lh * 0.25,
                _ => lh * self.paragraph_spacing,
            },
        }
    }
}
