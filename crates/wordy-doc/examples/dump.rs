//! Print every scene's paragraphs with their marks: `cargo run -p wordy-doc --example dump -- <project dir>`.

use std::path::PathBuf;

use wordy_doc::{Paragraphs, Project};

fn main() -> anyhow::Result<()> {
    let dir = std::env::args().nth(1).map(PathBuf::from).expect("project dir");
    let project = Project::open(&dir)?;
    println!("project: {}", project.name());
    for id in project.manuscript_scenes() {
        let node = project.node(id)?;
        println!("\n== {} ({} words)", node.title(), node.word_count());
        let body = node.body()?;
        for p in Paragraphs::from_text(&body).iter() {
            print!("  [{:?}]", p.block);
            for r in &p.runs {
                let m = &r.marks;
                let mut tags = Vec::new();
                for (on, t) in [
                    (m.bold, "b"),
                    (m.italic, "i"),
                    (m.underline, "u"),
                    (m.strike, "s"),
                    (m.smallcaps, "sc"),
                    (m.highlight.is_some(), "hl"),
                ] {
                    if on {
                        tags.push(t.to_string());
                    }
                }
                if let Some(l) = &m.link {
                    tags.push(format!("link={l}"));
                }
                if let Some(c) = &m.comment {
                    tags.push(format!("comment={c}"));
                }
                if tags.is_empty() {
                    print!(" {:?}", r.text)
                } else {
                    print!(" {:?}<{}>", r.text, tags.join(","))
                }
            }
            println!();
        }
    }
    let comments = project.comments();
    for id in comments.ids() {
        if let Some(c) = comments.get(&id) {
            println!("comment {id}: {:?} resolved={} node={:?}", c.text, c.resolved, c.node);
        }
    }
    Ok(())
}
