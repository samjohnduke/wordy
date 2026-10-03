//! Make a test project with one very long scene for performance checks:
//! `cargo run -p wordy-doc --example gen_long -- <new project dir> [words]`.

use std::path::PathBuf;

use wordy_doc::{NodeKind, Project, Space};

fn main() -> anyhow::Result<()> {
    let dir = std::env::args().nth(1).map(PathBuf::from).expect("project dir");
    let words: usize = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(30_000);
    let project = Project::create(&dir, "Long Haul")?;
    let root = project.root(Space::Manuscript);
    let chapter = project.create_node(root, NodeKind::Chapter, "Chapter One")?;
    let scene = project.create_node(chapter, NodeKind::Scene, "The Long Scene")?;
    let short = project.create_node(chapter, NodeKind::Scene, "A Short One")?;
    let body = project.node(scene)?.body()?;
    let lexicon = [
        "the", "harbour", "lamps", "guttered", "as", "Mara", "counted", "the", "boats", "again,", "and", "found",
        "one", "missing.", "Nobody", "spoke", "of", "it", "at", "supper;", "the", "silence", "had", "its", "own",
        "weather.", "Later", "she", "walked", "the", "quay", "with", "a", "lantern", "that", "would", "not", "stay",
        "lit,", "and", "listened", "to", "the", "water", "working", "at", "the", "stones.",
    ];
    let mut text = String::new();
    let mut n = 0;
    let mut i = 0;
    while n < words {
        let mut para = String::new();
        let len = 60 + (i * 7) % 90;
        for k in 0..len {
            if k > 0 {
                para.push(' ');
            }
            para.push_str(lexicon[(i * 13 + k * 7) % lexicon.len()]);
        }
        n += len;
        i += 1;
        text.push_str(&para);
        text.push('\n');
    }
    body.insert(0, &text)?;
    project
        .node(short)?
        .body()?
        .insert(0, "A short scene to switch to.\n")?;
    project.commit_meta();
    project.save()?;
    println!("{} words in {} paragraphs at {}", n, i, dir.display());
    Ok(())
}
