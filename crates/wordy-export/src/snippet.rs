//! A quotable PNG of a passage: text set in the bundled serif on a plain
//! card, light or dark, square (1080) or wide (1200x630).

use std::sync::Arc;

use anyhow::{anyhow, Result};
use cosmic_text::{
    fontdb, Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache, Wrap,
};
use tiny_skia::{Paint, Pixmap, PremultipliedColorU8, Rect, Transform};

use crate::fonts;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnippetSize {
    /// 1080 x 1080
    Square,
    /// 1200 x 630
    Wide,
}

impl SnippetSize {
    pub fn pixels(self) -> (u32, u32) {
        match self {
            SnippetSize::Square => (1080, 1080),
            SnippetSize::Wide => (1200, 630),
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            SnippetSize::Square => "Square",
            SnippetSize::Wide => "Wide",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnippetOptions {
    pub size: SnippetSize,
    pub dark: bool,
    /// Small line under the passage, e.g. the book title and author.
    pub attribution: String,
}

impl Default for SnippetOptions {
    fn default() -> Self {
        Self {
            size: SnippetSize::Square,
            dark: false,
            attribution: String::new(),
        }
    }
}

struct Palette {
    bg: tiny_skia::Color,
    fg: Color,
    muted: Color,
    accent: tiny_skia::Color,
}

fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            bg: tiny_skia::Color::from_rgba8(28, 27, 31, 255),
            fg: Color::rgb(236, 231, 222),
            muted: Color::rgb(150, 145, 138),
            accent: tiny_skia::Color::from_rgba8(214, 140, 72, 255),
        }
    } else {
        Palette {
            bg: tiny_skia::Color::from_rgba8(250, 247, 240, 255),
            fg: Color::rgb(36, 33, 30),
            muted: Color::rgb(130, 124, 116),
            accent: tiny_skia::Color::from_rgba8(196, 112, 48, 255),
        }
    }
}

fn font_system() -> FontSystem {
    let mut db = fontdb::Database::new();
    for data in fonts::ALL {
        let src: Arc<dyn AsRef<[u8]> + Send + Sync> = Arc::new(data);
        db.load_font_source(fontdb::Source::Binary(src));
    }
    FontSystem::new_with_locale_and_db("en-US".to_string(), db)
}

/// Render `text` to PNG bytes.
pub fn render(text: &str, opts: &SnippetOptions) -> Result<Vec<u8>> {
    let text = text.trim();
    if text.is_empty() {
        return Err(anyhow!("nothing selected"));
    }
    let (w, h) = opts.size.pixels();
    let pal = palette(opts.dark);
    let (mx, my) = match opts.size {
        SnippetSize::Square => (104.0f32, 104.0f32),
        SnippetSize::Wide => (96.0, 64.0),
    };
    let attr_size = match opts.size {
        SnippetSize::Square => 22.0,
        SnippetSize::Wide => 20.0,
    };
    let has_attr = !opts.attribution.trim().is_empty();
    let attr_h = if has_attr {
        attr_size * 1.4 + 28.0
    } else {
        0.0
    };
    let text_w = w as f32 - 2.0 * mx;
    let avail_h = h as f32 - 2.0 * my - attr_h;

    let mut fs = font_system();
    let attrs = Attrs::new().family(Family::Name(fonts::FAMILY));

    // Largest size whose wrapped height fits.
    let mut chosen: Option<(Buffer, f32)> = None;
    for size in [
        54.0f32, 50.0, 46.0, 42.0, 38.0, 34.0, 30.0, 27.0, 24.0, 21.0, 18.0,
    ] {
        let mut buffer = Buffer::new(&mut fs, Metrics::new(size, size * 1.38));
        buffer.set_size(&mut fs, Some(text_w), None);
        buffer.set_wrap(&mut fs, Wrap::WordOrGlyph);
        buffer.set_text(&mut fs, text, &attrs, Shaping::Advanced);
        buffer.shape_until_scroll(&mut fs, true);
        let height: f32 = buffer.layout_runs().map(|r| r.line_height).sum();
        let fits = height <= avail_h;
        chosen = Some((buffer, height));
        if fits {
            break;
        }
    }
    let (buffer, text_h) = chosen.expect("at least one size tried");

    let mut pixmap = Pixmap::new(w, h).ok_or_else(|| anyhow!("pixmap"))?;
    pixmap.fill(pal.bg);

    // Text block vertically centred in the space above the attribution.
    let top = my + ((avail_h - text_h).max(0.0) * 0.42);
    let mut cache = SwashCache::new();
    draw_buffer(&mut pixmap, &mut fs, &mut cache, &buffer, mx, top, pal.fg);

    if has_attr {
        let base_y = h as f32 - my - attr_size * 1.4;
        // Short accent rule above the attribution.
        let mut paint = Paint::default();
        paint.set_color(pal.accent);
        paint.anti_alias = true;
        if let Some(rect) = Rect::from_xywh(mx, base_y - 18.0, 56.0, 3.0) {
            pixmap.fill_rect(rect, &paint, Transform::identity(), None);
        }
        let mut ab = Buffer::new(&mut fs, Metrics::new(attr_size, attr_size * 1.4));
        ab.set_size(&mut fs, Some(text_w), None);
        ab.set_wrap(&mut fs, Wrap::None);
        ab.set_text(&mut fs, opts.attribution.trim(), &attrs, Shaping::Advanced);
        ab.shape_until_scroll(&mut fs, true);
        draw_buffer(&mut pixmap, &mut fs, &mut cache, &ab, mx, base_y, pal.muted);
    }

    pixmap.encode_png().map_err(|e| anyhow!("png: {e}"))
}

fn draw_buffer(
    pixmap: &mut Pixmap,
    fs: &mut FontSystem,
    cache: &mut SwashCache,
    buffer: &Buffer,
    ox: f32,
    oy: f32,
    color: Color,
) {
    let (w, h) = (pixmap.width() as i32, pixmap.height() as i32);
    let pixels = pixmap.pixels_mut();
    buffer.draw(fs, cache, color, |x, y, gw, gh, c| {
        let a = c.a();
        if a == 0 {
            return;
        }
        for py in 0..gh as i32 {
            for px in 0..gw as i32 {
                let (dx, dy) = (x + px + ox as i32, y + py + oy as i32);
                if dx < 0 || dy < 0 || dx >= w || dy >= h {
                    continue;
                }
                let ix = (dy * w + dx) as usize;
                let dst = pixels[ix];
                let af = a as f32 / 255.0;
                let blend = |d: u8, s: u8| {
                    (d as f32 * (1.0 - af) + s as f32 * af)
                        .round()
                        .clamp(0.0, 255.0) as u8
                };
                // Background is opaque, so the result stays opaque and premultiplied == straight.
                pixels[ix] = PremultipliedColorU8::from_rgba(
                    blend(dst.red(), c.r()),
                    blend(dst.green(), c.g()),
                    blend(dst.blue(), c.b()),
                    255,
                )
                .unwrap_or(dst);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_png_of_requested_size() {
        for (size, dark) in [(SnippetSize::Square, false), (SnippetSize::Wide, true)] {
            let png = render(
                "The lighthouse keeper counted the ships that did not come back, and gave each one a name.",
                &SnippetOptions { size, dark, attribution: "The Lighthouse — Ada Example".into() },
            )
            .unwrap();
            assert_eq!(&png[1..4], b"PNG");
            let decoded = Pixmap::decode_png(&png).unwrap();
            assert_eq!((decoded.width(), decoded.height()), size.pixels());
            // Some pixels must differ from the background: text was drawn.
            let bg = decoded.pixel(2, 2).unwrap();
            assert!(decoded.pixels().iter().any(|p| *p != bg), "blank image");
        }
    }

    #[test]
    fn empty_text_is_an_error() {
        assert!(render("   ", &SnippetOptions::default()).is_err());
    }
}
