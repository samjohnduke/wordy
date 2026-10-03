//! Flatten the manuscript tree into chapters and scenes, honouring the
//! include-in-compile flag. Every renderer starts from this.

use wordy_doc::{NodeKind, Paragraph, Paragraphs, Project, Space, TreeID};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompileOptions {
    /// Title page / metadata title. Defaults to the project name.
    pub title: String,
    pub author: String,
    /// Print scene titles as sub-headings (otherwise scenes are separated by `separator`).
    pub scene_titles: bool,
    /// Text of the scene separator, e.g. `#` or `* * *`.
    pub separator: String,
}

impl Default for CompileOptions {
    fn default() -> Self {
        Self { title: String::new(), author: String::new(), scene_titles: false, separator: "#".to_string() }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scene {
    pub title: String,
    pub paragraphs: Vec<Paragraph>,
}

impl Scene {
    pub fn word_count(&self) -> usize {
        self.paragraphs.iter().map(|p| p.word_count()).sum()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Chapter {
    pub title: String,
    pub scenes: Vec<Scene>,
}

impl Chapter {
    pub fn word_count(&self) -> usize {
        self.scenes.iter().map(|s| s.word_count()).sum()
    }
}

/// The manuscript, ready to render. `Send` so rendering can leave the UI thread.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Compiled {
    pub title: String,
    pub author: String,
    pub scene_titles: bool,
    pub separator: String,
    pub chapters: Vec<Chapter>,
}

impl Compiled {
    pub fn word_count(&self) -> usize {
        self.chapters.iter().map(|c| c.word_count()).sum()
    }

    pub fn scene_count(&self) -> usize {
        self.chapters.iter().map(|c| c.scenes.len()).sum()
    }
}

/// Walk the manuscript in order and build the chapter list.
///
/// Rules: a `Chapter` node (or any container with no chapters inside it) becomes
/// one chapter holding all its included descendant scenes. Containers that hold
/// chapters (acts, parts) are transparent. A scene sitting directly among
/// chapters becomes a chapter of its own.
pub fn compile(project: &Project, opts: &CompileOptions) -> Compiled {
    let title = if opts.title.trim().is_empty() { project.name() } else { opts.title.trim().to_string() };
    let mut out = Compiled {
        title,
        author: opts.author.trim().to_string(),
        scene_titles: opts.scene_titles,
        separator: opts.separator.clone(),
        chapters: Vec::new(),
    };
    let root = project.root(Space::Manuscript);
    collect(project, root, &mut out.chapters);
    out.chapters.retain(|c| !c.scenes.is_empty());
    out
}

fn collect(project: &Project, parent: TreeID, chapters: &mut Vec<Chapter>) {
    for id in project.children(parent) {
        let Ok(node) = project.node(id) else { continue };
        if !node.include_in_compile() {
            continue;
        }
        match node.kind() {
            NodeKind::Scene => {
                if let Some(scene) = scene_of(project, id) {
                    chapters.push(Chapter { title: scene.title.clone(), scenes: vec![scene] });
                }
            }
            NodeKind::Chapter => chapters.push(chapter_of(project, id, &node.title())),
            NodeKind::Act | NodeKind::Folder => {
                if contains_chapter(project, id) {
                    collect(project, id, chapters);
                } else {
                    chapters.push(chapter_of(project, id, &node.title()));
                }
            }
            _ => {}
        }
    }
}

fn contains_chapter(project: &Project, id: TreeID) -> bool {
    let mut found = false;
    project.walk(id, &mut |_, n| {
        if n.kind() == NodeKind::Chapter {
            found = true;
        }
    });
    found
}

fn chapter_of(project: &Project, id: TreeID, title: &str) -> Chapter {
    let mut scenes = Vec::new();
    collect_scenes(project, id, &mut scenes);
    Chapter { title: title.to_string(), scenes }
}

/// Included scenes under `parent`, skipping excluded subtrees.
fn collect_scenes(project: &Project, parent: TreeID, out: &mut Vec<Scene>) {
    for id in project.children(parent) {
        let Ok(node) = project.node(id) else { continue };
        if !node.include_in_compile() {
            continue;
        }
        if node.kind() == NodeKind::Scene {
            if let Some(s) = scene_of(project, id) {
                out.push(s);
            }
        } else if node.kind().is_container() {
            collect_scenes(project, id, out);
        }
    }
}

fn scene_of(project: &Project, id: TreeID) -> Option<Scene> {
    let node = project.node(id).ok()?;
    let body = node.body().ok()?;
    let mut paragraphs: Vec<Paragraph> = Paragraphs::from_text(&body).iter().cloned().collect();
    // Drop trailing empties so chapters do not end in blank lines.
    while paragraphs.last().map(|p| p.text.trim().is_empty() && p.block != wordy_doc::Block::Break).unwrap_or(false) {
        paragraphs.pop();
    }
    Some(Scene { title: node.title(), paragraphs })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> Project {
        let p = Project::new_in_memory("Book").unwrap();
        let root = p.root(Space::Manuscript);
        let act = p.create_node(root, NodeKind::Act, "Act I").unwrap();
        let ch1 = p.create_node(act, NodeKind::Chapter, "One").unwrap();
        let s1 = p.create_node(ch1, NodeKind::Scene, "Dawn").unwrap();
        p.node(s1).unwrap().body().unwrap().insert(0, "First scene.\n").unwrap();
        let s2 = p.create_node(ch1, NodeKind::Scene, "Noon").unwrap();
        p.node(s2).unwrap().body().unwrap().insert(0, "Second scene.\n").unwrap();
        p.node(s2).unwrap().set_include_in_compile(false).unwrap();
        let ch2 = p.create_node(act, NodeKind::Chapter, "Two").unwrap();
        let s3 = p.create_node(ch2, NodeKind::Scene, "Dusk").unwrap();
        p.node(s3).unwrap().body().unwrap().insert(0, "Third scene.\n\n").unwrap();
        let loose = p.create_node(root, NodeKind::Scene, "Epilogue").unwrap();
        p.node(loose).unwrap().body().unwrap().insert(0, "The end.\n").unwrap();
        p.commit_meta();
        p
    }

    #[test]
    fn compiles_in_order_and_honours_include_flag() {
        let p = project();
        let c = compile(&p, &CompileOptions::default());
        assert_eq!(c.title, "Book");
        let titles: Vec<_> = c.chapters.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(titles, ["One", "Two", "Epilogue"]);
        assert_eq!(c.chapters[0].scenes.len(), 1, "excluded scene is skipped");
        assert_eq!(c.chapters[0].scenes[0].paragraphs[0].text, "First scene.");
        assert_eq!(c.chapters[1].scenes[0].paragraphs.len(), 1, "trailing blank paragraph trimmed");
        assert_eq!(c.scene_count(), 3);
        assert_eq!(c.word_count(), 6);
    }

    #[test]
    fn options_override_title() {
        let p = project();
        let c = compile(&p, &CompileOptions { title: "Real Title".into(), author: "Me".into(), ..Default::default() });
        assert_eq!(c.title, "Real Title");
        assert_eq!(c.author, "Me");
    }

    #[test]
    fn file_stem_is_safe() {
        assert_eq!(crate::file_stem("My: Novel / Draft?"), "My Novel Draft");
        assert_eq!(crate::file_stem("   "), "Manuscript");
    }
}
