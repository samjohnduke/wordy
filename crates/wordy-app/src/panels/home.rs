//! Home tab: dashboard (goals, streak, pace, project file), reports (words
//! per day), tasks, the placeholder scan, export, LAN sync, and shortcuts.

use std::net::{SocketAddr, ToSocketAddrs as _};
use std::path::PathBuf;

use gpui_kit::base::StyledExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::chart::BarChart;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::dock::Panel;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Disableable as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::*;
use wordy_doc::chrono::{Duration, NaiveDate};
use wordy_doc::loro::{LoroValue, ValueOrContainer};
use wordy_doc::momentum::{date_str, parse_date, today};
use wordy_doc::{storage, Goals, NodeKind, Space, Status, TreeID};
use wordy_export::{CompileOptions, Format};

use crate::app::SharedProject;
use crate::sync::{SyncManager, SyncStatus};

pub enum HomeEvent {
    /// Open a node in an editor tab.
    Open(TreeID),
    /// Open a node and select `len` code points at `offset`.
    Reveal { id: TreeID, offset: usize, len: usize },
    /// Goals or tasks changed; persist.
    Changed,
    /// The tab became the displayed one.
    Activated,
    /// The tab was closed.
    Closed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Dashboard,
    Reports,
    Tasks,
    Placeholders,
    Export,
    Sync,
    Shortcuts,
}

impl Page {
    const ALL: [Page; 7] = [
        Page::Dashboard,
        Page::Reports,
        Page::Tasks,
        Page::Placeholders,
        Page::Export,
        Page::Sync,
        Page::Shortcuts,
    ];
    fn label(self) -> &'static str {
        match self {
            Page::Dashboard => "Dashboard",
            Page::Reports => "Reports",
            Page::Tasks => "Tasks",
            Page::Placeholders => "Placeholders",
            Page::Export => "Export",
            Page::Sync => "Sync",
            Page::Shortcuts => "Shortcuts",
        }
    }
    fn id(self) -> &'static str {
        match self {
            Page::Dashboard => "home-dashboard",
            Page::Reports => "home-reports",
            Page::Tasks => "home-tasks",
            Page::Placeholders => "home-placeholders",
            Page::Export => "home-export",
            Page::Sync => "home-sync",
            Page::Shortcuts => "home-shortcuts",
        }
    }
}

struct Day {
    label: String,
    words: f64,
}

/// Settings keys for the export page (project settings map).
mod export_keys {
    pub const TITLE: &str = "export.title";
    pub const AUTHOR: &str = "export.author";
    pub const SCENE_TITLES: &str = "export.scene_titles";
}

/// Outcome of the last export, shown under the buttons.
struct ExportStatus {
    message: String,
    path: Option<PathBuf>,
    ok: bool,
}

pub struct HomePanel {
    project: SharedProject,
    sync: Entity<SyncManager>,
    page: Page,
    /// Result line under the "Compact history" button.
    compact_status: Option<String>,
    daily: Entity<InputState>,
    manuscript: Entity<InputState>,
    deadline: Entity<InputState>,
    task: Entity<InputState>,
    export_title: Entity<InputState>,
    export_author: Entity<InputState>,
    export_status: Option<ExportStatus>,
    exporting: bool,
    _export_task: Option<Task<()>>,
    sync_name: Entity<InputState>,
    sync_code: Entity<InputState>,
    sync_addr: Entity<InputState>,
    sync_addr_error: Option<String>,
    pub focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl HomePanel {
    pub fn new(project: SharedProject, sync: Entity<SyncManager>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let goals = project.project.goals();
        let opt = |v: Option<i64>| v.map(|x| x.to_string()).unwrap_or_default();
        let daily = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(opt(goals.daily))
                .placeholder("words / day")
        });
        let manuscript = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(opt(goals.manuscript))
                .placeholder("total words")
        });
        let deadline = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(goals.deadline.map(date_str).unwrap_or_default())
                .placeholder("YYYY-MM-DD")
        });
        let task = cx.new(|cx| InputState::new(window, cx).placeholder("Add a task and press Enter"));
        let settings = project.project.settings_map();
        let project_name = project.project.name();
        let export_title = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(setting_str(&settings, export_keys::TITLE).unwrap_or_default())
                .placeholder(project_name)
        });
        let export_author = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(setting_str(&settings, export_keys::AUTHOR).unwrap_or_default())
                .placeholder("Author name")
        });

        let mut subs = Vec::new();
        for input in [&daily, &manuscript, &deadline] {
            subs.push(cx.subscribe_in(input, window, |this, _, ev: &InputEvent, _, cx| {
                if matches!(ev, InputEvent::Change | InputEvent::PressEnter { .. }) {
                    this.apply_goals(cx);
                }
            }));
        }
        subs.push(
            cx.subscribe_in(&task, window, |this, input, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    let text = input.read(cx).value().trim().to_string();
                    if !text.is_empty() {
                        if let Err(e) = this.project.project.add_task(&text, None) {
                            tracing::error!("add task: {e:#}");
                        }
                        input.update(cx, |s, cx| s.set_value("", window, cx));
                        this.changed(cx);
                    }
                }
            }),
        );

        for (input, key) in [
            (&export_title, export_keys::TITLE),
            (&export_author, export_keys::AUTHOR),
        ] {
            subs.push(
                cx.subscribe_in(input, window, move |this, input, ev: &InputEvent, _, cx| {
                    if matches!(ev, InputEvent::Change) {
                        let v = input.read(cx).value().trim().to_string();
                        let map = this.project.project.settings_map();
                        let r = if v.is_empty() {
                            map.delete(key)
                        } else {
                            map.insert(key, v).map(|_| ())
                        };
                        if let Err(e) = r {
                            tracing::error!("export setting: {e:#}");
                        }
                        this.changed(cx);
                    }
                }),
            );
        }

        let (peer_name, pairing_code) = {
            let m = sync.read(cx);
            (m.config.peer_name.clone(), m.config.pairing_code.clone())
        };
        let sync_name = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(peer_name)
                .placeholder("This machine's name")
        });
        let sync_code = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(pairing_code)
                .placeholder("Same code on both machines")
        });
        let sync_addr = cx.new(|cx| InputState::new(window, cx).placeholder("host:port, e.g. 192.168.1.20:40123"));
        subs.push(
            cx.subscribe_in(&sync_name, window, |this, input, ev: &InputEvent, _, cx| {
                if matches!(ev, InputEvent::Change) {
                    let v = input.read(cx).value().to_string();
                    this.sync.update(cx, |m, cx| m.set_peer_name(&v, cx));
                }
            }),
        );
        subs.push(
            cx.subscribe_in(&sync_code, window, |this, input, ev: &InputEvent, _, cx| {
                if matches!(ev, InputEvent::Change) {
                    let v = input.read(cx).value().to_string();
                    this.sync.update(cx, |m, cx| m.set_pairing_code(&v, cx));
                }
            }),
        );
        subs.push(cx.subscribe_in(&sync_addr, window, |this, _, ev: &InputEvent, _, cx| {
            if matches!(ev, InputEvent::PressEnter { .. }) {
                this.sync_manual(cx);
            }
        }));
        subs.push(cx.observe(&sync, |_, _, cx| cx.notify()));

        Self {
            project,
            sync,
            page: Page::Dashboard,
            compact_status: None,
            daily,
            manuscript,
            deadline,
            task,
            export_title,
            export_author,
            export_status: None,
            exporting: false,
            _export_task: None,
            sync_name,
            sync_code,
            sync_addr,
            sync_addr_error: None,
            focus: cx.focus_handle(),
            _subs: subs,
        }
    }

    fn compile_options(&self, cx: &App) -> CompileOptions {
        let settings = self.project.project.settings_map();
        CompileOptions {
            title: self.export_title.read(cx).value().trim().to_string(),
            author: self.export_author.read(cx).value().trim().to_string(),
            scene_titles: setting_bool(&settings, export_keys::SCENE_TITLES).unwrap_or(false),
            ..Default::default()
        }
    }

    /// Folder the save dialog opens in: next to the project, else home.
    fn export_dir(&self) -> PathBuf {
        self.project
            .dir()
            .and_then(|d| d.parent().map(|p| p.to_path_buf()))
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."))
    }

    fn set_status(&mut self, message: String, path: Option<PathBuf>, ok: bool, cx: &mut Context<Self>) {
        self.exporting = false;
        self.export_status = Some(ExportStatus { message, path, ok });
        cx.notify();
    }

    /// Compile the manuscript and write it in `format` to a path the user picks.
    fn export(&mut self, format: Format, cx: &mut Context<Self>) {
        if self.exporting {
            return;
        }
        let opts = self.compile_options(cx);
        let compiled = wordy_export::compile(&self.project.project, &opts);
        if compiled.chapters.is_empty() {
            self.set_status(
                "Nothing to export: the manuscript has no included scenes with text.".into(),
                None,
                false,
                cx,
            );
            return;
        }
        let name = format!("{}.{}", wordy_export::file_stem(&compiled.title), format.extension());
        let rx = cx.prompt_for_new_path(&self.export_dir(), Some(&name));
        self.exporting = true;
        self.export_status = None;
        cx.notify();
        self._export_task = Some(cx.spawn(async move |this, cx| {
            let path = match rx.await {
                Ok(Ok(Some(path))) => path,
                Ok(Ok(None)) | Err(_) => {
                    this.update(cx, |t, cx| {
                        t.exporting = false;
                        cx.notify();
                    })
                    .ok();
                    return;
                }
                Ok(Err(e)) => {
                    this.update(cx, |t, cx| {
                        t.set_status(format!("Could not open a save dialog: {e:#}"), None, false, cx)
                    })
                    .ok();
                    return;
                }
            };
            let out = path.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    let bytes = wordy_export::render(&compiled, format)?;
                    storage::write_atomic(&out, &bytes)?;
                    Ok::<usize, anyhow::Error>(bytes.len())
                })
                .await;
            this.update(cx, |t, cx| match result {
                Ok(n) => t.set_status(
                    format!("Exported {} ({}) to {}", format.label(), human_size(n), path.display()),
                    Some(path),
                    true,
                    cx,
                ),
                Err(e) => t.set_status(format!("Export failed: {e:#}"), None, false, cx),
            })
            .ok();
        }));
    }

    /// Zip the whole project folder as a backup.
    fn backup(&mut self, cx: &mut Context<Self>) {
        if self.exporting {
            return;
        }
        let Some(dir) = self.project.dir().cloned() else {
            self.set_status("This project is not saved to disk yet.".into(), None, false, cx);
            return;
        };
        // Flush pending edits so the archive is current.
        if let Err(e) = self.project.project.save() {
            tracing::error!("save before backup: {e:#}");
        }
        let name = format!(
            "{}-{}.zip",
            wordy_export::file_stem(&self.project.project.name()),
            today().format("%Y-%m-%d")
        );
        let rx = cx.prompt_for_new_path(&self.export_dir(), Some(&name));
        self.exporting = true;
        self.export_status = None;
        cx.notify();
        self._export_task = Some(cx.spawn(async move |this, cx| {
            let path = match rx.await {
                Ok(Ok(Some(path))) => path,
                Ok(Err(e)) => {
                    this.update(cx, |t, cx| {
                        t.set_status(format!("Could not open a save dialog: {e:#}"), None, false, cx)
                    })
                    .ok();
                    return;
                }
                _ => {
                    this.update(cx, |t, cx| {
                        t.exporting = false;
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };
            let out = path.clone();
            let result = cx
                .background_executor()
                .spawn(async move { wordy_export::archive::zip_project(&dir, &out) })
                .await;
            this.update(cx, |t, cx| match result {
                Ok(n) => t.set_status(
                    format!("Backed up {n} files to {}", path.display()),
                    Some(path),
                    true,
                    cx,
                ),
                Err(e) => t.set_status(format!("Backup failed: {e:#}"), None, false, cx),
            })
            .ok();
        }));
    }

    fn render_export(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let opts = self.compile_options(cx);
        let compiled = wordy_export::compile(&self.project.project, &opts);
        let scene_titles = opts.scene_titles;

        let field = |label: &str, input: &Entity<InputState>| {
            h_flex()
                .items_center()
                .gap_3()
                .child(div().w(px(70.)).text_xs().text_color(muted).child(label.to_string()))
                .child(div().w(px(360.)).child(Input::new(input).small()))
        };
        let manuscript = Self::section("Manuscript", cx)
            .child(field("Title", &self.export_title))
            .child(field("Author", &self.export_author))
            .child(
                Checkbox::new("export-scene-titles")
                    .checked(scene_titles)
                    .label("Show scene titles (otherwise scenes are separated by #)")
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        if let Err(e) = this
                            .project
                            .project
                            .settings_map()
                            .insert(export_keys::SCENE_TITLES, *checked)
                        {
                            tracing::error!("export setting: {e:#}");
                        }
                        this.changed(cx);
                    })),
            )
            .child(div().text_xs().text_color(muted).child(format!(
                "{} chapters · {} scenes · {} words. Scenes and folders with “include in compile” off are skipped.",
                compiled.chapters.len(),
                compiled.scene_count(),
                compiled.word_count()
            )));

        let mut formats = h_flex().gap_2().flex_wrap();
        for f in Format::ALL {
            formats = formats.child(
                Button::new(ElementId::Name(format!("export-{}", f.extension()).into()))
                    .primary()
                    .small()
                    .label(f.label())
                    .disabled(self.exporting)
                    .on_click(cx.listener(move |this, _, _, cx| this.export(f, cx))),
            );
        }
        let mut export_box = Self::section("Export manuscript", cx)
            .child(div().text_sm().text_color(muted).child(
                "Standard manuscript format for Word, a reflowable EPUB for e-readers, a typeset A5 PDF, or plain Markdown.",
            ))
            .child(formats);

        let backup_box = Self::section("Backup", cx)
            .child(div().text_sm().text_color(muted).child(
                "A zip of the whole project folder (document, assets, dictionary, saved snapshots). Unzip it anywhere and open it as a project.",
            ))
            .child(
                h_flex().child(
                    Button::new("export-zip")
                        .small()
                        .label("Project backup (.zip)")
                        .disabled(self.exporting)
                        .on_click(cx.listener(|this, _, _, cx| this.backup(cx))),
                ),
            );

        if self.exporting {
            export_box = export_box.child(div().text_xs().text_color(muted).child("Exporting…"));
        }
        let mut status = v_flex().gap_2();
        if let Some(st) = &self.export_status {
            let color = if st.ok { theme.foreground } else { theme.danger };
            let mut row = h_flex()
                .items_center()
                .gap_3()
                .text_sm()
                .child(div().text_color(color).child(st.message.clone()));
            if let Some(path) = st.path.clone() {
                let p2 = path.clone();
                row = row
                    .child(
                        Button::new("export-reveal")
                            .ghost()
                            .xsmall()
                            .label("Show in folder")
                            .on_click(move |_, _, cx| cx.reveal_path(&path)),
                    )
                    .child(
                        Button::new("export-open")
                            .ghost()
                            .xsmall()
                            .label("Open")
                            .on_click(move |_, _, cx| cx.open_with_system(&p2)),
                    );
            }
            status = status.child(row);
        }

        v_flex()
            .gap_3()
            .w_full()
            .child(manuscript)
            .child(export_box)
            .child(backup_box)
            .child(status)
            .into_any_element()
    }

    /// Sync with the address typed into the manual field.
    fn sync_manual(&mut self, cx: &mut Context<Self>) {
        let text = self.sync_addr.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }
        let addr: Option<SocketAddr> = text.parse().ok().or_else(|| text.to_socket_addrs().ok()?.next());
        match addr {
            Some(addr) => {
                self.sync_addr_error = None;
                self.sync.update(cx, |m, cx| m.sync_with(addr, &text, cx));
            }
            None => self.sync_addr_error = Some(format!("“{text}” is not a host:port address")),
        }
        cx.notify();
    }

    fn render_sync(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let m = self.sync.read(cx);
        let busy = m.busy();
        let ready = m.config.ready();
        let my_project = m.project_id();
        let port = m.port();
        let peers = m.peers.clone();
        let status = m.status.clone();
        let discovery_error = m.discovery_error.clone();

        let field = |label: &str, input: &Entity<InputState>| {
            h_flex()
                .items_center()
                .gap_3()
                .child(div().w(px(90.)).text_xs().text_color(muted).child(label.to_string()))
                .child(div().w(px(360.)).child(Input::new(input).small()))
        };
        let listening = match port {
            Some(p) => format!("Listening on port {p}. Other copies of this project on the network appear below."),
            None => "The sync server could not start; see the log.".to_string(),
        };
        let this_machine = Self::section("This machine", cx)
            .child(field("Name", &self.sync_name))
            .child(field("Pairing code", &self.sync_code))
            .child(div().text_xs().text_color(muted).child(listening))
            .child(div().text_xs().text_color(muted).child(
                "Type the same pairing code on both machines once. Sync merges edits made on both sides, copies missing attachments both ways and unions the custom dictionaries.",
            ));

        let mut peer_box = Self::section("Peers", cx);
        if let Some(err) = discovery_error {
            peer_box = peer_box.child(div().text_xs().text_color(theme.danger).child(err));
        }
        if peers.is_empty() {
            peer_box = peer_box.child(div().text_sm().text_color(muted).child(
                "No other Wordy found yet. Open the same project on the other machine (copy the folder or a backup zip first).",
            ));
        }
        for (i, peer) in peers.into_iter().enumerate() {
            let same = peer.project_id == my_project;
            let addr = peer.addr;
            let name = peer.name.clone();
            let mut row = h_flex()
                .items_center()
                .gap_3()
                .child(div().text_sm().font_semibold().w(px(200.)).child(name.clone()))
                .child(div().text_xs().text_color(muted).w(px(180.)).child(addr.to_string()));
            if same {
                row = row.child(
                    Button::new(ElementId::Name(format!("sync-peer-{i}").into()))
                        .primary()
                        .small()
                        .label("Sync")
                        .disabled(busy || !ready)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let name = name.clone();
                            this.sync.update(cx, |m, cx| m.sync_with(addr, &name, cx));
                        })),
                );
            } else {
                row = row.child(div().text_xs().text_color(muted).child("different project"));
            }
            peer_box = peer_box.child(row);
        }
        let mut manual = h_flex()
            .items_center()
            .gap_3()
            .child(div().w(px(90.)).text_xs().text_color(muted).child("By address"))
            .child(div().w(px(360.)).child(Input::new(&self.sync_addr).small()))
            .child(
                Button::new("sync-manual")
                    .small()
                    .label("Connect")
                    .disabled(busy || !ready)
                    .on_click(cx.listener(|this, _, _, cx| this.sync_manual(cx))),
            );
        if let Some(err) = &self.sync_addr_error {
            manual = manual.child(div().text_xs().text_color(theme.danger).child(err.clone()));
        }
        peer_box = peer_box.child(manual);

        let status_line: Option<(String, bool)> = match status {
            SyncStatus::Idle => None,
            SyncStatus::Busy(s) => Some((s, true)),
            SyncStatus::Done { peer, summary, when } => {
                Some((format!("Synced with {peer} at {when}: {summary}."), true))
            }
            SyncStatus::Failed(e) => Some((format!("Sync failed: {e}"), false)),
        };
        let mut status_box = v_flex().gap_2();
        if let Some((text, ok)) = status_line {
            let color = if ok { theme.foreground } else { theme.danger };
            status_box = status_box.child(div().text_sm().text_color(color).child(text));
        }
        if !ready {
            status_box = status_box.child(
                div()
                    .text_xs()
                    .text_color(theme.danger)
                    .child("Enter a pairing code before syncing."),
            );
        }

        v_flex()
            .gap_3()
            .w_full()
            .child(this_machine)
            .child(peer_box)
            .child(status_box)
            .into_any_element()
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        self.project.project.commit_meta();
        cx.emit(HomeEvent::Changed);
        cx.notify();
    }

    /// Read the three goal inputs; ignore fields that do not parse yet.
    fn apply_goals(&mut self, cx: &mut Context<Self>) {
        let num = |s: &Entity<InputState>, cx: &App| -> Result<Option<i64>, ()> {
            let v = s.read(cx).value().trim().to_string();
            if v.is_empty() {
                Ok(None)
            } else {
                v.parse::<i64>().map(|n| Some(n).filter(|n| *n > 0)).map_err(|_| ())
            }
        };
        let current = self.project.project.goals();
        let daily = num(&self.daily, cx).unwrap_or(current.daily);
        let manuscript = num(&self.manuscript, cx).unwrap_or(current.manuscript);
        let dl = self.deadline.read(cx).value().trim().to_string();
        let deadline = if dl.is_empty() {
            None
        } else {
            match parse_date(&dl) {
                Some(d) => Some(d),
                None => current.deadline,
            }
        };
        let goals = Goals {
            daily,
            manuscript,
            deadline,
        };
        if goals != current {
            if let Err(e) = self.project.project.set_goals(&goals) {
                tracing::error!("set goals: {e:#}");
            }
            self.changed(cx);
        }
    }

    fn set_page(&mut self, page: Page, cx: &mut Context<Self>) {
        self.page = page;
        cx.notify();
    }

    fn section(title: &str, cx: &App) -> Div {
        v_flex()
            .w_full()
            .gap_2()
            .p_3()
            .rounded_md()
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().secondary.opacity(0.35))
            .child(
                div()
                    .text_xs()
                    .font_semibold()
                    .text_color(cx.theme().muted_foreground)
                    .child(title.to_uppercase()),
            )
    }

    /// One chapter line on the dashboard: title, scene count, words.
    #[allow(clippy::too_many_arguments)]
    fn structure_row(
        id: String,
        title: String,
        scenes: usize,
        words: usize,
        muted_title: bool,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
        cx: &App,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        h_flex()
            .id(ElementId::Name(id.into()))
            .w_full()
            .px_1()
            .py_0p5()
            .rounded_sm()
            .cursor_pointer()
            .hover(|s| s.bg(theme.secondary))
            .text_sm()
            .child(
                div()
                    .flex_1()
                    .when(muted_title, |d| d.italic().text_color(muted))
                    .child(title),
            )
            .child(div().w(px(70.)).text_color(muted).child(format!("{scenes} sc")))
            .child(div().w(px(80.)).text_right().child(format!("{words}")))
            .on_click(on_click)
    }

    fn render_dashboard(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = &self.project.project;
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let today = today();
        let goals = p.goals();
        let words_today = p.words_on(today);
        let streak = p.streak(today);
        let manuscript = p.manuscript_word_count();

        let big = |n: String, label: &str| {
            v_flex()
                .gap_0()
                .child(div().text_2xl().font_semibold().child(n))
                .child(div().text_xs().text_color(muted).child(label.to_string()))
        };

        // ---- today ----
        let mut today_box = Self::section("Today", cx).child(
            h_flex()
                .gap_6()
                .child(big(format!("{words_today:+}"), "words today"))
                .child(big(
                    format!("{streak}"),
                    if streak == 1 { "day streak" } else { "days streak" },
                ))
                .child(big(format!("{manuscript}"), "manuscript words")),
        );
        if let Some(d) = goals.daily {
            let pct = ((words_today.max(0) as f32 / d as f32) * 100.).min(100.);
            today_box = today_box.child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(format!("Daily goal: {words_today} / {d}")),
                    )
                    .child(Progress::new("daily-progress").value(pct).color(theme.primary)),
            );
        }

        // ---- manuscript goal ----
        let mut goal_box = Self::section("Goals", cx);
        if let Some(m) = goals.manuscript {
            let pct = ((manuscript as f32 / m as f32) * 100.).min(100.);
            goal_box = goal_box.child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(format!("Manuscript: {manuscript} / {m}")),
                    )
                    .child(Progress::new("ms-progress").value(pct).color(theme.primary)),
            );
            if let Some(dl) = goals.deadline {
                let days = (dl - today).num_days();
                let line = match goals.required_pace(manuscript, today) {
                    Some(pace) if days >= 0 => {
                        format!(
                            "Deadline {} · {} days left · {pace} words/day needed",
                            date_str(dl),
                            days + 1
                        )
                    }
                    _ if days < 0 => format!("Deadline {} passed", date_str(dl)),
                    _ => format!("Deadline {} · goal reached", date_str(dl)),
                };
                goal_box = goal_box.child(div().text_sm().child(line));
            }
        }
        let field = |label: &str, input: &Entity<InputState>, w: f32| {
            h_flex()
                .gap_2()
                .items_center()
                .child(div().w(px(90.)).text_xs().text_color(muted).child(label.to_string()))
                .child(div().w(px(w)).child(Input::new(input).small()))
        };
        goal_box = goal_box
            .child(field("Daily goal", &self.daily, 120.))
            .child(field("Manuscript", &self.manuscript, 120.))
            .child(field("Deadline", &self.deadline, 140.));

        // ---- structure ----
        // Chapters are rows; scenes sitting directly under the root (not yet
        // filed into a chapter) are gathered into one "Unsorted" row.
        let root = p.root(Space::Manuscript);
        let mut rows = v_flex().gap_0().w_full();
        let mut by_status = [0usize; 4];
        let mut unsorted: Vec<(TreeID, usize)> = Vec::new();
        for chapter in p.children(root) {
            let Ok(node) = p.node(chapter) else { continue };
            if node.kind() == NodeKind::Scene {
                let ix = Status::ALL.iter().position(|s| *s == node.status()).unwrap_or(1);
                by_status[ix] += 1;
                unsorted.push((
                    chapter,
                    if node.include_in_compile() {
                        node.word_count()
                    } else {
                        0
                    },
                ));
                continue;
            }
            let mut words = 0usize;
            let mut scenes = 0usize;
            p.walk(chapter, &mut |_, n| {
                if n.kind() == NodeKind::Scene {
                    scenes += 1;
                    if n.include_in_compile() {
                        words += n.word_count();
                    }
                    let ix = Status::ALL.iter().position(|s| *s == n.status()).unwrap_or(1);
                    by_status[ix] += 1;
                }
            });
            let id = chapter;
            rows = rows.child(Self::structure_row(
                format!("dash-ch-{chapter}"),
                node.title(),
                scenes,
                words,
                false,
                cx.listener(move |_, _, _, cx| cx.emit(HomeEvent::Open(id))),
                cx,
            ));
        }
        if let Some((first, _)) = unsorted.first().copied() {
            let words: usize = unsorted.iter().map(|(_, w)| w).sum();
            rows = rows.child(Self::structure_row(
                "dash-unsorted".to_string(),
                "Unsorted scenes".to_string(),
                unsorted.len(),
                words,
                true,
                cx.listener(move |_, _, _, cx| cx.emit(HomeEvent::Open(first))),
                cx,
            ));
        }
        let status_line = Status::ALL
            .iter()
            .zip(by_status)
            .filter(|(_, n)| *n > 0)
            .map(|(s, n)| format!("{n} {}", s.label().to_lowercase()))
            .collect::<Vec<_>>()
            .join(" · ");
        let structure = Self::section("Manuscript", cx)
            .child(div().text_xs().text_color(muted).child(if status_line.is_empty() {
                "No scenes yet.".to_string()
            } else {
                status_line
            }))
            .child(rows);

        // ---- project file ----
        let stats = p.history_stats();
        let backups = self.project.dir().map(|d| storage::backups(d).len()).unwrap_or(0);
        let mut summary = format!(
            "{} on disk · {} changes · {} operations · {} backup{} in snapshots/",
            human_size(stats.file_bytes as usize),
            stats.changes,
            stats.ops,
            backups,
            if backups == 1 { "" } else { "s" }
        );
        if stats.shallow {
            summary.push_str(" · history compacted");
        }
        let file_box = Self::section("Project file", cx)
            .child(div().text_sm().child(summary))
            .child(div().text_xs().text_color(muted).child(
                "Every keystroke is kept as history so the two machines can merge. Compacting drops the history \
                 before now and keeps only the current text. Do it right after a sync: the other machine must sync \
                 again before it edits, or copy this folder across instead.",
            ))
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(
                        Button::new("compact-history")
                            .outline()
                            .small()
                            .label("Compact history")
                            .on_click(cx.listener(|this, _, _, cx| this.compact_history(cx))),
                    )
                    .children(
                        self.compact_status
                            .clone()
                            .map(|t| div().text_xs().text_color(muted).child(t)),
                    ),
            );

        v_flex()
            .gap_3()
            .w_full()
            .child(today_box)
            .child(goal_box)
            .child(structure)
            .child(file_box)
            .into_any_element()
    }

    fn compact_history(&mut self, cx: &mut Context<Self>) {
        self.compact_status = Some(match self.project.project.compact_history() {
            Ok(s) => format!(
                "Compacted: {} on disk, {} operations.",
                human_size(s.file_bytes as usize),
                s.ops
            ),
            Err(e) => format!("Compaction failed: {e:#}"),
        });
        cx.notify();
    }

    fn render_shortcuts(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let (m, alt, shift) = if cfg!(target_os = "macos") {
            ("⌘", "⌥", "⇧")
        } else {
            ("Ctrl", "Alt", "Shift")
        };
        let j = |parts: &[&str]| parts.join(if cfg!(target_os = "macos") { "" } else { "+" });
        let groups: Vec<(&str, Vec<(String, &str)>)> = vec![
            (
                "Navigation",
                vec![
                    (j(&[m, "P"]), "Quick open: jump to any scene, entity, or note"),
                    (
                        j(&[m, "E"]),
                        "Focus the sidebar (arrows move, Enter opens, F2 renames, Delete trashes, Esc returns)",
                    ),
                    (j(&[m, "1"]) + " / 2 / 3", "Manuscript, World, Notes"),
                    (j(&[m, alt, "↓"]) + " / ↑", "Next / previous document in this space"),
                    (
                        j(&["Ctrl", "Tab"]) + " / " + &j(&["Ctrl", shift, "Tab"]),
                        "Next / previous tab",
                    ),
                    (j(&[m, "W"]), "Close tab"),
                    (j(&[m, shift, "H"]), "Home tab"),
                    (j(&[m, shift, "F"]), "Search the project"),
                    (j(&[m, shift, "R"]), "Show / hide the reference pane"),
                ],
            ),
            (
                "Writing",
                vec![
                    (
                        j(&[m, shift, "D"]),
                        "Focus mode: just the page, other paragraphs dimmed",
                    ),
                    (
                        j(&[m, shift, "Y"]),
                        "Typewriter scrolling: the caret line stays centred",
                    ),
                    (j(&[m, "N"]), "New scene / entity / note next to the selection"),
                    (j(&[m, "S"]), "Save now and write a backup to snapshots/"),
                    (
                        j(&[m, "F"]) + " / " + &j(&[m, "H"]),
                        "Find / find and replace in this document",
                    ),
                    (j(&[m, "G"]) + " / " + &j(&[m, shift, "G"]), "Next / previous match"),
                ],
            ),
            (
                "Formatting",
                vec![
                    (j(&[m, "B"]) + " / I / U", "Bold / italic / underline"),
                    (j(&[m, shift, "X"]), "Strikethrough"),
                    (j(&[m, shift, "K"]), "Small caps"),
                    (j(&[m, shift, "H"]) + " (in text)", "Highlight"),
                    (j(&[m, alt, "0"]) + " / 1 / 2 / 3", "Paragraph / heading 1 / 2 / 3"),
                    (j(&[m, shift, "Q"]), "Quote"),
                    (j(&[m, shift, "Enter"]), "Scene break"),
                    (j(&[m, "K"]) + " / " + &j(&[m, shift, "L"]), "Insert link / remove link"),
                    (
                        j(&[m, shift, "M"]) + " / " + &j(&[m, shift, "E"]),
                        "Add comment / edit comment at caret",
                    ),
                    ("@".to_string(), "Mention an entity (type to filter, Enter to insert)"),
                ],
            ),
        ];
        let sections = groups.into_iter().map(|(title, rows)| {
            Self::section(title, cx).children(rows.into_iter().map(|(keys, what)| {
                h_flex()
                    .gap_3()
                    .items_start()
                    .text_sm()
                    .child(div().w(px(200.)).flex_shrink_0().font_semibold().child(keys))
                    .child(div().flex_1().min_w_0().text_color(muted).child(what))
            }))
        });
        v_flex().gap_3().w_full().children(sections).into_any_element()
    }

    fn render_reports(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = &self.project.project;
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let today = today();
        let sessions = p.sessions();
        let mut days: Vec<Day> = Vec::new();
        let mut total = 0i64;
        let mut active = 0usize;
        let mut seconds = 0i64;
        for i in (0..30).rev() {
            let d: NaiveDate = today - Duration::days(i);
            let s = sessions.iter().find(|s| s.date == d);
            let words = s.map(|s| s.words().max(0)).unwrap_or(0);
            if let Some(s) = s {
                seconds += s.seconds;
            }
            if words > 0 {
                active += 1;
            }
            total += words;
            days.push(Day {
                label: d.format("%-d").to_string(),
                words: words as f64,
            });
        }
        let avg = if active > 0 { total / active as i64 } else { 0 };
        let best = sessions.iter().map(|s| s.words()).max().unwrap_or(0);

        let chart = BarChart::new(days)
            .id("words-per-day")
            .band(|d: &Day| d.label.clone())
            .value(|d: &Day| d.words)
            .label_axis(true)
            .value_axis(true)
            .grid(true)
            .tooltip_title(|d: &Day| format!("Day {}", d.label).into())
            .tooltip_value(|_, v| format!("{v:.0} words").into());

        let summary = h_flex()
            .gap_6()
            .text_sm()
            .child(div().child(format!("{total} words in the last 30 days")))
            .child(
                div()
                    .text_color(muted)
                    .child(format!("{active} active days · avg {avg} · best {best}")),
            )
            .child(div().text_color(muted).child(format!("{} h written", seconds / 3600)));

        let mut table = v_flex().gap_0().w_full().text_sm();
        for s in sessions.iter().rev().take(14) {
            table = table.child(
                h_flex()
                    .w_full()
                    .px_1()
                    .py_0p5()
                    .child(div().w(px(110.)).child(date_str(s.date)))
                    .child(div().w(px(90.)).text_right().child(format!("{:+}", s.words())))
                    .child(
                        div()
                            .w(px(90.))
                            .text_right()
                            .text_color(muted)
                            .child(format!("{} min", s.seconds / 60)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_right()
                            .text_color(muted)
                            .child(format!("{}", s.words_end)),
                    ),
            );
        }

        v_flex()
            .gap_3()
            .w_full()
            .child(
                Self::section("Words per day (last 30 days)", cx)
                    .child(summary)
                    .child(div().w_full().h(px(220.)).child(chart)),
            )
            .child(
                Self::section("Sessions", cx)
                    .child(
                        h_flex()
                            .w_full()
                            .px_1()
                            .text_xs()
                            .text_color(muted)
                            .child(div().w(px(110.)).child("Date"))
                            .child(div().w(px(90.)).text_right().child("Words"))
                            .child(div().w(px(90.)).text_right().child("Time"))
                            .child(div().flex_1().text_right().child("Manuscript")),
                    )
                    .child(table),
            )
            .into_any_element()
    }

    fn render_tasks(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = &self.project.project;
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let tasks = p.task_list();
        let open = tasks.iter().filter(|t| !t.done).count();
        let mut list = v_flex().gap_0p5().w_full();
        for t in &tasks {
            let id = t.id.clone();
            let id2 = t.id.clone();
            let done = t.done;
            let node_title = t.node.and_then(|n| p.node(n).ok()).map(|n| n.title());
            let node_id = t.node;
            let weak = cx.weak_entity();
            list = list.child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap_2()
                    .px_1()
                    .py_0p5()
                    .rounded_sm()
                    .hover(|s| s.bg(theme.secondary))
                    .child(
                        Checkbox::new(ElementId::Name(format!("task-{}", t.id).into()))
                            .checked(done)
                            .label(t.text.clone())
                            .on_click(move |checked: &bool, _, cx| {
                                let id = id.clone();
                                let checked = *checked;
                                weak.update(cx, move |this, cx| {
                                    if let Err(e) = this.project.project.set_task_done(&id, checked) {
                                        tracing::error!("task: {e:#}");
                                    }
                                    this.changed(cx);
                                })
                                .ok();
                            }),
                    )
                    .children(node_title.map(|title| {
                        div()
                            .id(ElementId::Name(format!("task-node-{}", t.id).into()))
                            .text_xs()
                            .text_color(theme.primary)
                            .cursor_pointer()
                            .child(title)
                            .on_click(cx.listener(move |_, _, _, cx| {
                                if let Some(n) = node_id {
                                    cx.emit(HomeEvent::Open(n));
                                }
                            }))
                    }))
                    .child(div().flex_1())
                    .child(
                        Button::new(ElementId::Name(format!("task-rm-{}", t.id).into()))
                            .ghost()
                            .xsmall()
                            .label("×")
                            .tooltip("Remove task")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Err(e) = this.project.project.remove_task(&id2) {
                                    tracing::error!("remove task: {e:#}");
                                }
                                this.changed(cx);
                            })),
                    ),
            );
        }
        if tasks.is_empty() {
            list = list.child(div().text_sm().text_color(muted).child("No tasks yet."));
        }
        Self::section(&format!("Tasks · {open} open"), cx)
            .child(div().w(px(360.)).child(Input::new(&self.task).small()))
            .child(list)
            .into_any_element()
    }

    fn render_placeholders(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = &self.project.project;
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let hits = p.placeholders();
        let mut list = v_flex().gap_0p5().w_full();
        let mut last: Option<TreeID> = None;
        for (ix, h) in hits.iter().enumerate() {
            if last != Some(h.node) {
                list = list.child(
                    div()
                        .mt_2()
                        .text_xs()
                        .font_semibold()
                        .text_color(muted)
                        .child(h.title.clone()),
                );
                last = Some(h.node);
            }
            let (id, offset, len) = (h.node, h.offset, h.marker.chars().count());
            list = list.child(
                h_flex()
                    .id(ElementId::Name(format!("ph-{ix}").into()))
                    .w_full()
                    .items_center()
                    .gap_2()
                    .px_1()
                    .py_0p5()
                    .rounded_sm()
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.secondary))
                    .text_sm()
                    .child(
                        div()
                            .w(px(160.))
                            .flex_shrink_0()
                            .font_semibold()
                            .child(h.marker.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_color(muted)
                            .child(h.snippet.clone()),
                    )
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(HomeEvent::Reveal { id, offset, len }))),
            );
        }
        if hits.is_empty() {
            list = list.child(div().text_sm().text_color(muted).child(
                "Nothing to fill in. Markers like [TODO], [TK], [FIX], [CHECK], [?] and TK / TBD / XXX are listed here.",
            ));
        }
        Self::section(&format!("Placeholders · {}", hits.len()), cx)
            .child(list)
            .into_any_element()
    }
}

impl gpui_kit::component::dock::BasePanel for HomePanel {
    fn panel_name(&self) -> &'static str {
        "Home"
    }
    fn closable(&self, _: &App) -> bool {
        true
    }
    fn set_active(&mut self, active: bool, _: &mut Window, cx: &mut Context<Self>) {
        if active {
            cx.emit(HomeEvent::Activated);
            cx.notify();
        }
    }
    fn on_removed(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(HomeEvent::Closed);
    }
}

impl EventEmitter<gpui_kit::component::dock::PanelEvent> for HomePanel {}

impl Focusable for HomePanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for HomePanel {
    fn tab_name(&self, _: &App) -> Option<SharedString> {
        Some("Home".into())
    }
}

impl EventEmitter<HomeEvent> for HomePanel {}

impl Render for HomePanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page = self.page;
        let tabs = h_flex()
            .gap_1()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .children(Page::ALL.into_iter().map(|p| {
                Button::new(p.id())
                    .ghost()
                    .small()
                    .label(p.label())
                    .toggled(p == page)
                    .on_click(cx.listener(move |this, _, _, cx| this.set_page(p, cx)))
            }));
        let body = match page {
            Page::Dashboard => self.render_dashboard(cx),
            Page::Reports => self.render_reports(cx),
            Page::Tasks => self.render_tasks(cx),
            Page::Placeholders => self.render_placeholders(cx),
            Page::Export => self.render_export(cx),
            Page::Sync => self.render_sync(cx),
            Page::Shortcuts => self.render_shortcuts(cx),
        };
        v_flex()
            .size_full()
            .track_focus(&self.focus)
            .bg(cx.theme().background)
            .child(tabs)
            .child(
                div()
                    .id("home-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(div().max_w(px(820.)).w_full().mx_auto().p_4().child(body)),
            )
    }
}

fn setting_str(map: &wordy_doc::loro::LoroMap, key: &str) -> Option<String> {
    match map.get(key) {
        Some(ValueOrContainer::Value(LoroValue::String(s))) => Some(s.to_string()),
        _ => None,
    }
}

fn setting_bool(map: &wordy_doc::loro::LoroMap, key: &str) -> Option<bool> {
    match map.get(key) {
        Some(ValueOrContainer::Value(LoroValue::Bool(b))) => Some(b),
        _ => None,
    }
}

fn human_size(n: usize) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{:.0} KB", n as f64 / 1024.)
    } else {
        format!("{:.1} MB", n as f64 / (1024. * 1024.))
    }
}
