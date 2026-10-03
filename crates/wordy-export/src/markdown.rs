//! Plain Markdown: handy for pasting into anything else.

use std::fmt::Write as _;

use wordy_doc::{Block, Marks};

use crate::compile::Compiled;

pub fn render(c: &Compiled) -> String {
    let mut s = format!("# {}\n\n", c.title);
    if !c.author.is_empty() {
        let _ = writeln!(s, "*{}*\n", c.author);
    }
    for chapter in &c.chapters {
        let _ = writeln!(s, "## {}\n", chapter.title);
        for (i, scene) in chapter.scenes.iter().enumerate() {
            if c.scene_titles && (chapter.scenes.len() > 1 || scene.title != chapter.title) {
                let _ = writeln!(s, "### {}\n", scene.title);
            } else if i > 0 {
                s.push_str("* * *\n\n");
            }
            for p in &scene.paragraphs {
                let inner: String = p.runs.iter().map(|r| styled(&esc(&r.text), &r.marks)).collect();
                match p.block {
                    Block::Paragraph => {
                        let _ = writeln!(s, "{inner}\n");
                    }
                    Block::H1 => {
                        let _ = writeln!(s, "### {inner}\n");
                    }
                    Block::H2 | Block::H3 => {
                        let _ = writeln!(s, "#### {inner}\n");
                    }
                    Block::Quote => {
                        let _ = writeln!(s, "> {inner}\n");
                    }
                    Block::Break => s.push_str("* * *\n\n"),
                }
            }
        }
    }
    s
}

fn styled(text: &str, m: &Marks) -> String {
    if text.trim().is_empty() {
        return text.to_string();
    }
    // Keep surrounding whitespace outside the markers so `** x**` never happens.
    let start = text.len() - text.trim_start().len();
    let end = text.trim_end().len();
    let (lead, core, trail) = (&text[..start], &text[start..end], &text[end..]);
    let mut s = core.to_string();
    if m.underline {
        s = format!("<u>{s}</u>");
    }
    if m.strike {
        s = format!("~~{s}~~");
    }
    if m.highlight.is_some() {
        s = format!("=={s}==");
    }
    if m.italic {
        s = format!("*{s}*");
    }
    if m.bold {
        s = format!("**{s}**");
    }
    format!("{lead}{s}{trail}")
}

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if matches!(ch, '\\' | '*' | '_' | '`' | '[' | ']' | '#' | '<' | '>') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn markdown_marks_and_structure() {
        let md = super::render(&crate::docx::tests::sample());
        assert!(md.starts_with("# The Lighthouse & the Sea\n\n*Ada Example*\n"));
        assert!(md.contains("## One\n"));
        assert!(md.contains("Hello \\<world\\> ***loud***\n"));
        assert!(md.contains("> Quoted"));
        assert!(md.contains("* * *"));
    }
}
