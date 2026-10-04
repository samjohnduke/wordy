//! Settings tab: the account, export, appearance, text, the project file
//! and the shortcut list. While the tab is in front the sidebar lists the
//! sections in place of the manuscript tree, so each thing has one place.

use std::path::PathBuf;

use gpui_kit::base::StyledExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::dock::Panel;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Disableable as _, IconName, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::*;
use wordy_doc::momentum::today;
use wordy_doc::storage;
use wordy_export::{CompileOptions, Format};

use super::{human_size, section, setting_bool, setting_str};
use crate::app::{CloseTab, SharedProject};
use crate::prefs::{self, Appearance, ParagraphStyle, Prefs, Scrollbars, TextPrefs, ThemeFamily, UserThemes};
use crate::projects;
use crate::sync::{CloudStatus, CloudSyncStatus, SyncManager};

/// What an Appearance choice does when picked.
type Pick = Box<dyn Fn(&mut Window, &mut App)>;

/// The families the font picker offers: the bundled serif first, then every
/// family the text system knows, installed or dropped into the fonts folder.
fn font_names(cx: &App) -> Vec<SharedString> {
    let mut names = cx.text_system().all_font_names();
    names.retain(|n| n != TextPrefs::BUNDLED_FONT && !n.starts_with('.'));
    std::iter::once(TextPrefs::BUNDLED_FONT.to_string())
        .chain(names)
        .map(SharedString::from)
        .collect()
}

pub enum SettingsEvent {
    /// A project setting changed; persist.
    Changed,
    /// The tab became the displayed one.
    Activated,
    /// The tab was closed.
    Closed,
    /// Replace this project with the one in the folder.
    SwitchProject(PathBuf),
    /// Name and create a new project.
    NewProject,
    /// Pick a project folder to open.
    OpenProjectFolder,
}

/// The pages down the left of the tab.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Section {
    Account,
    Export,
    Appearance,
    Text,
    Project,
    Shortcuts,
}

impl Section {
    pub const ALL: [Section; 6] = [
        Section::Account,
        Section::Export,
        Section::Appearance,
        Section::Text,
        Section::Project,
        Section::Shortcuts,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Section::Account => "Account",
            Section::Export => "Export",
            Section::Appearance => "Appearance",
            Section::Text => "Text",
            Section::Project => "Project",
            Section::Shortcuts => "Shortcuts",
        }
    }

    /// The section `delta` places away in the list, clamped at the ends.
    pub fn step(self, delta: isize) -> Section {
        let ix = Section::ALL.iter().position(|s| *s == self).unwrap_or(0) as isize;
        let last = Section::ALL.len() as isize - 1;
        Section::ALL[(ix + delta).clamp(0, last) as usize]
    }

    pub fn id(self) -> &'static str {
        match self {
            Section::Account => "settings-account",
            Section::Export => "settings-export",
            Section::Appearance => "settings-appearance",
            Section::Text => "settings-text",
            Section::Project => "settings-project",
            Section::Shortcuts => "settings-shortcuts",
        }
    }

    /// One line under the page title.
    fn blurb(self) -> &'static str {
        match self {
            Section::Account => "Your Wordy account, the machines linked to it, and syncing this project.",
            Section::Export => "Compile the manuscript to a file, or zip the whole project.",
            Section::Appearance => "How the app looks on this machine.",
            Section::Text => "The type on the page you write on. Per machine, like Appearance.",
            Section::Project => "This project's name and file, and the other projects on this machine.",
            Section::Shortcuts => "Every keyboard shortcut.",
        }
    }
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

pub struct SettingsPanel {
    project: SharedProject,
    sync: Entity<SyncManager>,
    section: Section,
    /// Result line under the "Compact history" button.
    compact_status: Option<String>,
    /// The project's name, edited in place.
    name: Entity<InputState>,
    /// Projects on this machine, read when the Project page is shown.
    projects: Vec<projects::KnownProject>,
    export_title: Entity<InputState>,
    export_author: Entity<InputState>,
    export_status: Option<ExportStatus>,
    exporting: bool,
    _export_task: Option<Task<()>>,
    sync_name: Entity<InputState>,
    sync_server: Entity<InputState>,
    /// The editor font: the bundled serif first, then every installed family.
    font_select: Entity<SelectState<Vec<SharedString>>>,
    /// What the last Save a copy did, shown under the Your themes buttons.
    theme_status: Option<String>,
    /// The only tab in the centre, so the tab bar needs its own close button.
    sole_tab: bool,
    pub focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl SettingsPanel {
    pub fn new(project: SharedProject, sync: Entity<SyncManager>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = project.project.settings_map();
        let project_name = project.project.name();
        let export_title = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(setting_str(&settings, export_keys::TITLE).unwrap_or_default())
                .placeholder(project_name.clone())
        });
        let export_author = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(setting_str(&settings, export_keys::AUTHOR).unwrap_or_default())
                .placeholder("Author name")
        });

        let name = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(project_name.clone())
                .placeholder("Project name")
        });
        let mut subs = Vec::new();
        subs.push(cx.subscribe_in(&name, window, |this, input, ev: &InputEvent, _, cx| {
            if matches!(ev, InputEvent::Change) {
                let value = input.read(cx).value().trim().to_string();
                if !value.is_empty() && value != this.project.project.name() {
                    if let Err(e) = this.project.project.set_name(&value) {
                        tracing::error!("rename project: {e:#}");
                    }
                    cx.emit(SettingsEvent::Changed);
                }
            }
        }));
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

        let (device_name, cloud_server) = {
            let m = sync.read(cx);
            (m.config.device_name.clone(), m.config.cloud_server.clone())
        };
        let sync_server = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(cloud_server)
                .placeholder(wordy_sync::cloud::DEFAULT_SERVER)
        });
        subs.push(
            cx.subscribe_in(&sync_server, window, |this, input, ev: &InputEvent, _, cx| {
                if matches!(ev, InputEvent::Change) {
                    let v = input.read(cx).value().to_string();
                    this.sync.update(cx, |m, cx| m.set_cloud_server(&v, cx));
                }
            }),
        );
        let sync_name = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(device_name)
                .placeholder("This machine's name")
        });
        subs.push(
            cx.subscribe_in(&sync_name, window, |this, input, ev: &InputEvent, _, cx| {
                if matches!(ev, InputEvent::Change) {
                    let v = input.read(cx).value().to_string();
                    this.sync.update(cx, |m, cx| m.set_device_name(&v, cx));
                }
            }),
        );
        subs.push(cx.observe(&sync, |_, _, cx| cx.notify()));

        let current: SharedString = Prefs::global(cx).text.font.clone().into();
        let font_select = cx.new(|cx| {
            let mut s = SelectState::new(font_names(cx), None, window, cx).searchable(true);
            s.set_selected_value(&current, window, cx);
            s
        });
        subs.push(cx.subscribe_in(
            &font_select,
            window,
            |_, _, ev: &SelectEvent<Vec<SharedString>>, window, cx| {
                if let SelectEvent::Confirm(Some(font)) = ev {
                    let font = font.to_string();
                    Prefs::update(Some(window), cx, |p| p.text.font = font);
                }
            },
        ));
        // Text and theme edits change the global, not this panel; redraw.
        subs.push(cx.observe_global::<Prefs>(|_, cx| cx.notify()));
        subs.push(cx.observe_global::<UserThemes>(|_, cx| cx.notify()));

        Self {
            project,
            sync,
            section: Section::Account,
            compact_status: None,
            name,
            projects: Vec::new(),
            export_title,
            export_author,
            export_status: None,
            exporting: false,
            _export_task: None,
            sync_name,
            sync_server,
            font_select,
            theme_status: None,
            sole_tab: false,
            focus: cx.focus_handle(),
            _subs: subs,
        }
    }

    pub fn section_shown(&self) -> Section {
        self.section
    }

    pub fn show_section(&mut self, section: Section, cx: &mut Context<Self>) {
        if section == Section::Project {
            self.projects = projects::known_projects(cx);
        }
        if self.section != section {
            self.section = section;
            cx.notify();
        }
    }

    /// Told by the workspace: this is the only tab in the centre.
    pub fn set_sole_tab(&mut self, sole: bool, cx: &mut Context<Self>) {
        if self.sole_tab != sole {
            self.sole_tab = sole;
            cx.notify();
        }
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        self.project.project.commit_meta();
        cx.emit(SettingsEvent::Changed);
        cx.notify();
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
        // Flush pending edits so the archive is current, project.json included.
        if let Err(e) = self.project.project.save_and_mirror() {
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
        let manuscript = section("Manuscript", cx)
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
        let mut export_box = section("Export manuscript", cx)
            .child(div().text_sm().text_color(muted).child(
                "Standard manuscript format for Word, a reflowable EPUB for e-readers, a typeset A5 PDF, or plain Markdown.",
            ))
            .child(formats);

        let backup_box = section("Backup", cx)
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

    /// The cloud account card: link this machine, or show who is signed in
    /// and the other linked machines.
    fn render_account(&self, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let m = self.sync.read(cx);
        let account = m.config.cloud.clone();
        let status = m.cloud_status.clone();
        let devices = m.cloud_devices.clone();
        let remote = m.cloud_projects.clone();
        let local_ids = m.local_ids.clone();
        let busy = m.cloud_busy();
        let server = m.config.cloud_server();
        let syncable = m.cloud_syncable();
        let project_on = m.cloud_enabled();
        let project_status = m.cloud_sync_status();

        let mut card = section("Account", cx);
        match account {
            None => {
                card = card.child(div().text_xs().text_color(muted).child(
                    "Link this machine to your Wordy account to back projects up and sync them between machines. Sign-in happens in your browser with a passkey; nothing is typed here.",
                ));
                card = card.child(
                    h_flex()
                        .items_center()
                        .gap_3()
                        .child(div().w(px(90.)).text_xs().text_color(muted).child("This machine"))
                        .child(
                            div()
                                .w(px(360.))
                                .child(Input::new(&self.sync_name).small().disabled(busy)),
                        ),
                );
                card = card.child(
                    h_flex()
                        .items_center()
                        .gap_3()
                        .child(div().w(px(90.)).text_xs().text_color(muted).child("Server"))
                        .child(
                            div()
                                .w(px(360.))
                                .child(Input::new(&self.sync_server).small().disabled(busy)),
                        ),
                );
                match &status {
                    CloudStatus::Waiting { user_code, .. } => {
                        card = card.child(
                            h_flex()
                                .items_center()
                                .gap_3()
                                .child(div().text_sm().child("Your code is"))
                                .child(div().text_lg().font_semibold().child(user_code.clone()))
                                .child(div().text_sm().text_color(muted).child("— approve it in the browser.")),
                        );
                        let code = user_code.clone();
                        card = card.child(
                            h_flex()
                                .gap_2()
                                .child(
                                    Button::new("cloud-open")
                                        .small()
                                        .label("Open the browser again")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.sync.update(cx, |m, cx| m.open_verify_url(cx));
                                        })),
                                )
                                .child(
                                    Button::new("cloud-copy")
                                        .small()
                                        .label("Copy code")
                                        .on_click(cx.listener(move |_, _, _, cx| {
                                            cx.write_to_clipboard(ClipboardItem::new_string(code.clone()));
                                        })),
                                )
                                .child(
                                    Button::new("cloud-cancel")
                                        .small()
                                        .label("Cancel")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.sync.update(cx, |m, cx| m.cancel_link(cx));
                                        })),
                                ),
                        );
                    }
                    _ => {
                        card = card.child(
                            h_flex().gap_2().child(
                                Button::new("cloud-link")
                                    .primary()
                                    .small()
                                    .label(if matches!(status, CloudStatus::Starting) {
                                        "Asking for a code…"
                                    } else {
                                        "Link this machine"
                                    })
                                    .disabled(busy)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.sync.update(cx, |m, cx| m.link_cloud(cx));
                                    })),
                            ),
                        );
                    }
                }
            }
            Some(acct) => {
                card = card.child(div().text_sm().child(format!(
                    "Signed in as {} on {server}. This machine is “{}”.",
                    acct.email, acct.device_name
                )));
                let mut rows = v_flex().gap_1();
                for (i, d) in devices.iter().enumerate() {
                    let seen = d.last_seen_at.get(..10).unwrap_or(&d.last_seen_at).to_string();
                    let mut row = h_flex()
                        .items_center()
                        .gap_3()
                        .child(div().text_sm().font_semibold().w(px(200.)).child(d.name.clone()))
                        .child(div().text_xs().text_color(muted).w(px(80.)).child(d.platform.clone()))
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .w(px(140.))
                                .child(format!("last seen {seen}")),
                        );
                    if d.current {
                        row = row.child(div().text_xs().text_color(muted).child("this machine"));
                    } else {
                        let id = d.id.clone();
                        row = row.child(
                            Button::new(ElementId::Name(format!("cloud-remove-{i}").into()))
                                .small()
                                .label("Remove")
                                .disabled(busy)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let id = id.clone();
                                    this.sync.update(cx, |m, cx| m.remove_cloud_device(&id, cx));
                                })),
                        );
                    }
                    rows = rows.child(row);
                }
                if !devices.is_empty() {
                    card = card.child(rows);
                }
                card = card.child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("cloud-refresh")
                                .small()
                                .label("Refresh")
                                .disabled(busy)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.sync.update(cx, |m, cx| m.refresh_cloud(false, cx));
                                })),
                        )
                        .child(
                            Button::new("cloud-unlink")
                                .small()
                                .label("Unlink this machine")
                                .disabled(busy)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.sync.update(cx, |m, cx| m.unlink_cloud(cx));
                                })),
                        ),
                );
                if !remote.is_empty() {
                    card = card.child(div().mt_2().text_sm().font_semibold().child("On the server"));
                    card = card.child(div().text_xs().text_color(muted).child(
                        "Every project this account can reach. A copy lands in your projects folder with cloud sync on, and opens in a new window.",
                    ));
                    let mut rows = v_flex().gap_1();
                    for (i, p) in remote.iter().enumerate() {
                        let here = local_ids.contains(&p.id);
                        let mut row = h_flex()
                            .items_center()
                            .gap_3()
                            .child(div().text_sm().font_semibold().w(px(200.)).child(p.name.clone()))
                            .child(div().text_xs().text_color(muted).w(px(80.)).child(if p.owner {
                                "yours".to_string()
                            } else {
                                format!("shared, {}", p.role)
                            }));
                        if here {
                            row = row.child(div().text_xs().text_color(muted).child("on this machine"));
                        } else {
                            let id = p.id.clone();
                            let name = p.name.clone();
                            row = row.child(
                                Button::new(ElementId::Name(format!("cloud-fetch-{i}").into()))
                                    .small()
                                    .label("Get a copy")
                                    .disabled(busy)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let (id, name) = (id.clone(), name.clone());
                                        this.sync.update(cx, |m, cx| m.fetch_cloud_project(&id, &name, cx));
                                    })),
                            );
                        }
                        rows = rows.child(row);
                    }
                    card = card.child(rows);
                }
                if syncable {
                    card = card.child(div().mt_2().text_sm().font_semibold().child("This project"));
                    let (line, danger) = match &project_status {
                        CloudSyncStatus::Off if project_on => ("Cloud sync is on; starting…".to_string(), false),
                        CloudSyncStatus::Off => (
                            "Kept on this machine only. Turn cloud sync on to keep a live copy on the server and on every linked machine; edits travel as you type."
                                .to_string(),
                            false,
                        ),
                        CloudSyncStatus::Connecting => ("Connecting to the server…".to_string(), false),
                        CloudSyncStatus::Live {
                            devices,
                            pending,
                            read_only,
                        } => (
                            format!(
                                "Live{}: {} {} here{}.",
                                if *read_only { " (read-only: shared with you as a reader)" } else { "" },
                                devices,
                                if *devices == 1 { "machine" } else { "machines" },
                                if *pending { ", changes on their way" } else { "" }
                            ),
                            false,
                        ),
                        CloudSyncStatus::Offline(r) => (
                            format!("Offline ({r}). Edits stay here and go up when the server is back."),
                            false,
                        ),
                        CloudSyncStatus::Failed(r) => (format!("Stopped: {r}"), true),
                    };
                    card = card.child(
                        div()
                            .text_xs()
                            .text_color(if danger { theme.danger } else { muted })
                            .child(line),
                    );
                    let on = project_on;
                    let mut toggle = Button::new("cloud-project")
                        .small()
                        .label(if on {
                            "Stop syncing this project"
                        } else {
                            "Sync this project"
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.sync.update(cx, |m, cx| m.set_cloud_enabled(!on, cx));
                        }));
                    if !on {
                        toggle = toggle.primary();
                    }
                    card = card.child(h_flex().gap_2().child(toggle));
                }
            }
        }
        match status {
            CloudStatus::Busy(s) => card = card.child(div().text_xs().text_color(muted).child(s)),
            CloudStatus::Failed(e) => card = card.child(div().text_xs().text_color(theme.danger).child(e)),
            _ => {}
        }
        card
    }

    /// Theme family, light or dark, and scrollbars, kept per user in
    /// `prefs.json`.
    fn render_appearance(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let danger = cx.theme().danger;
        let prefs = Prefs::global(cx).clone();
        let user_themes = UserThemes::global(cx).clone();
        let choice = |id: &'static str, label: SharedString, on: bool, pick: Pick| {
            Button::new(id)
                .outline()
                .small()
                .label(label)
                .toggled(on)
                .on_click(move |_, window, cx| pick(window, cx))
        };
        let family_box = section("Theme", cx)
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child("Each theme has a light half and a dark half. Light or dark, below, picks which one shows."),
            )
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .children(ThemeFamily::BUNDLED.into_iter().map(|f| {
                        let id: &'static str = match f {
                            ThemeFamily::Default => "family-default",
                            ThemeFamily::Wordy => "family-wordy",
                            ThemeFamily::Catppuccin => "family-catppuccin",
                            ThemeFamily::HighContrast => "family-high-contrast",
                            ThemeFamily::User(_) => "family-user",
                        };
                        choice(
                            id,
                            f.label().into(),
                            prefs.theme == f,
                            Box::new(move |window, cx| Prefs::update(Some(window), cx, |p| p.theme = f.clone())),
                        )
                    }))
                    .children(user_themes.sets.iter().map(|set| {
                        let family = ThemeFamily::User(set.name.clone());
                        let on = prefs.theme == family;
                        Button::new(format!("family-user-{}", set.name))
                            .outline()
                            .small()
                            .label(set.name.clone())
                            .toggled(on)
                            .on_click(move |_, window, cx| {
                                Prefs::update(Some(window), cx, |p| p.theme = family.clone())
                            })
                    })),
            )
            .child(div().text_xs().text_color(muted).child(prefs.theme.blurb(cx)));
        let dir = prefs::themes_dir();
        let dir2 = dir.clone();
        let copy_of = prefs.theme.clone();
        let own_box = section("Your themes", cx)
            .child(div().text_xs().text_color(muted).child(
                "Drop a theme file in this folder and it joins the list above; edit it with Wordy open and the \
                 change shows as you save. Save a copy of the current theme to start from, and point your editor \
                 at theme.schema.json in the folder for the keys.",
            ))
            .child(
                div()
                    .text_sm()
                    .font_family("monospace")
                    .child(dir.display().to_string()),
            )
            .child(div().text_xs().text_color(muted).child(match user_themes.sets.len() {
                0 => "No theme files there yet.".to_string(),
                1 => "1 theme file.".to_string(),
                n => format!("{n} theme files."),
            }))
            .children(
                user_themes
                    .errors
                    .iter()
                    .map(|(file, err)| div().text_xs().text_color(danger).child(format!("{file}: {err}"))),
            )
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        Button::new("themes-open")
                            .outline()
                            .small()
                            .label("Open folder")
                            .on_click(move |_, _, cx| {
                                if let Err(e) = std::fs::create_dir_all(&dir) {
                                    tracing::error!("{}: {e}", dir.display());
                                }
                                cx.open_with_system(&dir);
                            }),
                    )
                    .child(
                        Button::new("themes-reload")
                            .outline()
                            .small()
                            .label("Reload")
                            .on_click(|_, _, cx| prefs::load_user_themes(cx)),
                    )
                    .child(
                        Button::new("themes-copy")
                            .outline()
                            .small()
                            .label(format!("Save a copy of {}", prefs.theme.label()))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.theme_status = Some(match prefs::save_theme_copy(&copy_of, cx) {
                                    Ok(path) => {
                                        let name = path.strip_prefix(&dir2).unwrap_or(&path).display().to_string();
                                        prefs::load_user_themes(cx);
                                        format!("Saved {name}. Edit it there; it is in the list above.")
                                    }
                                    Err(e) => format!("Could not save a copy: {e:#}"),
                                });
                                cx.notify();
                            })),
                    ),
            )
            .when_some(self.theme_status.clone(), |this, status| {
                this.child(div().text_xs().text_color(muted).child(status))
            });
        let theme_box = section("Light or dark", cx)
            .child(div().text_xs().text_color(muted).child(
                "Follow the system's light or dark setting, or pin one. The sun / moon button on the rail and Ctrl-Shift-T flip between light and dark, and pin the result here.",
            ))
            .child(h_flex().gap_2().flex_wrap().children(Appearance::ALL.into_iter().map(|a| {
                let id: &'static str = match a {
                    Appearance::System => "theme-system",
                    Appearance::Light => "theme-light",
                    Appearance::Dark => "theme-dark",
                };
                choice(
                    id,
                    a.label().into(),
                    prefs.appearance == a,
                    Box::new(move |window, cx| Prefs::update(Some(window), cx, |p| p.appearance = a)),
                )
            })));
        let scroll_box = section("Scrollbars", cx)
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child("When a pane's scrollbar is drawn."),
            )
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .children(Scrollbars::ALL.into_iter().map(|s| {
                        let id: &'static str = match s {
                            Scrollbars::Scrolling => "scroll-scrolling",
                            Scrollbars::Hover => "scroll-hover",
                            Scrollbars::Always => "scroll-always",
                        };
                        choice(
                            id,
                            s.label().into(),
                            prefs.scrollbars == s,
                            Box::new(move |window, cx| Prefs::update(Some(window), cx, |p| p.scrollbars = s)),
                        )
                    })),
            );
        v_flex()
            .gap_3()
            .w_full()
            .child(family_box)
            .child(theme_box)
            .child(scroll_box)
            .child(own_box)
            .into_any_element()
    }

    /// Font, size, spacing, measure and paragraph style for the editor.
    fn render_text(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let text = Prefs::global(cx).text.clone();
        let stepper = |dec_id: &'static str, inc_id: &'static str, value: String, dec: Pick, inc: Pick| {
            h_flex()
                .gap_1()
                .items_center()
                .child(
                    Button::new(dec_id)
                        .outline()
                        .small()
                        .label("−")
                        .on_click(move |_, window, cx| dec(window, cx)),
                )
                .child(div().text_sm().w(px(72.)).text_center().child(value))
                .child(
                    Button::new(inc_id)
                        .outline()
                        .small()
                        .label("+")
                        .on_click(move |_, window, cx| inc(window, cx)),
                )
        };
        let row = |label: &'static str, control: Div| {
            h_flex()
                .gap_3()
                .items_center()
                .child(div().w(px(120.)).text_sm().child(label))
                .child(control)
        };
        let step = |field: fn(&mut TextPrefs) -> &mut f32, range: (f32, f32, f32), steps: f32| -> Pick {
            Box::new(move |window, cx| {
                Prefs::update(Some(window), cx, |p| TextPrefs::step(field(&mut p.text), range, steps))
            })
        };
        let preview = div()
            .w_full()
            .p_3()
            .rounded_md()
            .border_1()
            .border_color(theme.border)
            .bg(theme.background)
            .font_family(text.font.clone())
            .text_size(px(text.size))
            .line_height(relative(text.line_height))
            .child(
                "She walked to the window and looked out at the rain, thinking of nothing in particular.                  The quick brown fox jumps over the lazy dog; 0123456789.",
            );
        let font_box = section("Font", cx)
            .child(div().text_xs().text_color(muted).child(
                "Any font installed on this machine. Libertinus Serif comes with Wordy and is what the PDF export uses.",
            ))
            .child(Select::new(&self.font_select).id("text-font").w(px(360.)).placeholder("Font"))
            .child(preview);
        let type_box = section("Size and spacing", cx)
            .child(row(
                "Size",
                stepper(
                    "size-dec",
                    "size-inc",
                    format!("{:.0} px", text.size),
                    step(|t| &mut t.size, TextPrefs::SIZE, -1.),
                    step(|t| &mut t.size, TextPrefs::SIZE, 1.),
                ),
            ))
            .child(row(
                "Line spacing",
                stepper(
                    "line-dec",
                    "line-inc",
                    format!("{:.2}×", text.line_height),
                    step(|t| &mut t.line_height, TextPrefs::LINE_HEIGHT, -1.),
                    step(|t| &mut t.line_height, TextPrefs::LINE_HEIGHT, 1.),
                ),
            ))
            .child(row(
                "Column width",
                stepper(
                    "width-dec",
                    "width-inc",
                    format!("{:.0} px", text.width),
                    step(|t| &mut t.width, TextPrefs::WIDTH, -1.),
                    step(|t| &mut t.width, TextPrefs::WIDTH, 1.),
                ),
            ))
            .child(div().text_xs().text_color(muted).child(
                "Column width is the widest the text gets; it narrows with the window. Headings scale with the size.",
            ));
        let dir = prefs::fonts_dir();
        let files = prefs::user_font_files().len();
        let select = self.font_select.clone();
        let own_box = section("Your fonts", cx)
            .child(div().text_xs().text_color(muted).child(
                "Drop .ttf, .otf or .ttc files in this folder and Wordy loads them at launch, no install needed. \
                 Reload after adding some.",
            ))
            .child(
                div()
                    .text_sm()
                    .font_family("monospace")
                    .child(dir.display().to_string()),
            )
            .child(div().text_xs().text_color(muted).child(match files {
                0 => "No font files there yet.".to_string(),
                1 => "1 font file.".to_string(),
                n => format!("{n} font files."),
            }))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("fonts-open")
                            .outline()
                            .small()
                            .label("Open folder")
                            .on_click(move |_, _, cx| {
                                if let Err(e) = std::fs::create_dir_all(&dir) {
                                    tracing::error!("{}: {e}", dir.display());
                                }
                                cx.open_with_system(&dir);
                            }),
                    )
                    .child(Button::new("fonts-reload").outline().small().label("Reload").on_click(
                        move |_, window, cx| {
                            prefs::load_user_fonts(cx);
                            let current: SharedString = Prefs::global(cx).text.font.clone().into();
                            let names = font_names(cx);
                            select.update(cx, |s, cx| {
                                s.set_items(names, window, cx);
                                s.set_selected_value(&current, window, cx);
                            });
                            // The editors re-resolve the family on their next layout.
                            Prefs::apply(Some(window), cx);
                        },
                    )),
            );
        let para_box = section("Paragraphs", cx)
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child("How one body paragraph is told from the next. Exports have their own setting."),
            )
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .children(ParagraphStyle::ALL.into_iter().map(|s| {
                        let id: &'static str = match s {
                            ParagraphStyle::Indent => "para-indent",
                            ParagraphStyle::Spaced => "para-spaced",
                        };
                        Button::new(id)
                            .outline()
                            .small()
                            .label(s.label())
                            .toggled(text.paragraphs == s)
                            .on_click(move |_, window, cx| Prefs::update(Some(window), cx, |p| p.text.paragraphs = s))
                    })),
            );
        let reset = h_flex().child(
            Button::new("text-reset")
                .outline()
                .small()
                .label("Reset to defaults")
                .disabled(text == TextPrefs::default())
                .on_click(|_, window, cx| Prefs::update(Some(window), cx, |p| p.text = TextPrefs::default())),
        );
        v_flex()
            .gap_3()
            .w_full()
            .child(font_box)
            .child(type_box)
            .child(own_box)
            .child(para_box)
            .child(reset)
            .into_any_element()
    }

    /// The project file on disk: size, history, backups, compaction.
    fn render_project(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = &self.project.project;
        let muted = cx.theme().muted_foreground;
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
        let file_box = section("Project file", cx)
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

        let current = self.project.dir().cloned();
        let name_box = section("Name", cx)
            .child(div().w(px(320.)).child(Input::new(&self.name).small()))
            .child(div().text_xs().text_color(muted).child(
                "Shown in the title bar and the project list, and the default title of an export. \
                     The folder keeps its name.",
            ));

        let mut rows = v_flex().gap_1();
        for known in self.projects.iter() {
            let is_current = Some(&known.dir) == current.as_ref();
            let dir = known.dir.clone();
            let mut row = h_flex()
                .items_center()
                .gap_3()
                .child(div().text_sm().font_semibold().w(px(200.)).child(known.name.clone()))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_xs()
                        .text_color(muted)
                        .child(known.dir.display().to_string()),
                );
            row = if is_current {
                row.child(div().text_xs().text_color(muted).child("open"))
            } else {
                row.child(
                    Button::new(SharedString::from(format!("open-project:{}", known.dir.display())))
                        .outline()
                        .xsmall()
                        .label("Open")
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(SettingsEvent::SwitchProject(dir.clone()));
                        })),
                )
            };
            rows = rows.child(row);
        }
        let projects_box = section("Projects on this machine", cx)
            .child(div().text_xs().text_color(muted).child(format!(
                "Projects you have opened here, then the rest of {}. Opening one replaces this window after saving.",
                storage::projects_root().display()
            )))
            .child(rows)
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(
                        Button::new("new-project")
                            .outline()
                            .small()
                            .label("New project…")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(SettingsEvent::NewProject))),
                    )
                    .child(
                        Button::new("open-folder")
                            .outline()
                            .small()
                            .label("Open a folder…")
                            .tooltip("An existing project folder, or an empty folder to start a project in")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(SettingsEvent::OpenProjectFolder))),
                    )
                    .children(current.clone().map(|dir| {
                        Button::new("reveal-project")
                            .ghost()
                            .small()
                            .label("Show in file manager")
                            .on_click(move |_, _, cx| cx.reveal_path(&dir))
                    })),
            );

        v_flex()
            .gap_3()
            .w_full()
            .child(name_box)
            .child(file_box)
            .child(projects_box)
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
                    (
                        j(&[m, "P"]),
                        "Palette: jump to any scene, entity, or note; type > for commands",
                    ),
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
                    (j(&[m, "0"]), "Home tab"),
                    (j(&[m, ","]), "Settings"),
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
                    (
                        j(&[m, shift, "H"]),
                        "Highlight: cycles yellow, green, blue, pink, grey, off",
                    ),
                    (j(&[m, "\\"]), "Clear formatting"),
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
            section(title, cx).children(rows.into_iter().map(|(keys, what)| {
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
}

impl gpui_kit::component::dock::BasePanel for SettingsPanel {
    fn panel_name(&self) -> &'static str {
        "Settings"
    }
    fn closable(&self, _: &App) -> bool {
        true
    }
    fn set_active(&mut self, active: bool, _: &mut Window, cx: &mut Context<Self>) {
        if active {
            cx.emit(SettingsEvent::Activated);
            cx.notify();
        }
    }
    fn on_removed(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(SettingsEvent::Closed);
    }
}

impl EventEmitter<gpui_kit::component::dock::PanelEvent> for SettingsPanel {}
impl EventEmitter<SettingsEvent> for SettingsPanel {}

impl Focusable for SettingsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for SettingsPanel {
    fn tab_name(&self, _: &App) -> Option<SharedString> {
        Some("Settings".into())
    }

    /// The dock draws no close button on a lone tab; see `HomePanel`.
    fn title_suffix(&mut self, _: &mut Window, _: &mut Context<Self>) -> Option<impl IntoElement> {
        self.sole_tab.then(|| {
            Button::new("close-sole-tab")
                .icon(IconName::Close)
                .xsmall()
                .ghost()
                .tab_stop(false)
                .tooltip("Close tab")
                .on_click(|_, window, cx| window.dispatch_action(Box::new(CloseTab), cx))
        })
    }

    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}

impl Render for SettingsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let current = self.section;
        let body = match current {
            Section::Account => self.render_account(cx).into_any_element(),
            Section::Export => self.render_export(cx),
            Section::Appearance => self.render_appearance(cx),
            Section::Text => self.render_text(cx),
            Section::Project => self.render_project(cx),
            Section::Shortcuts => self.render_shortcuts(cx),
        };
        let page = v_flex()
            .gap_1()
            .max_w(px(820.))
            .w_full()
            .p_4()
            .child(div().text_lg().font_semibold().child(current.label()))
            .child(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(current.blurb()),
            )
            .child(div().h(px(8.)))
            .child(body);
        div()
            .id("settings-scroll")
            .size_full()
            .track_focus(&self.focus)
            .bg(theme.background)
            .overflow_y_scroll()
            .child(page)
    }
}
