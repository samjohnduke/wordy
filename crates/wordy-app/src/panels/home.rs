//! Home tab: one page with today's count, goals, the manuscript's shape,
//! tasks, the placeholder scan and the writing reports. Export, the account
//! and the rest live in the Settings tab.

use gpui_kit::base::StyledExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::chart::BarChart;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::dock::Panel;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, IconName, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::*;
use wordy_doc::chrono::{Datelike as _, Duration, NaiveDate};
use wordy_doc::momentum::{date_str, parse_date, today};
use wordy_doc::{Goals, NodeKind, Space, Status, TreeID};

use super::section;
use crate::app::{CloseTab, SharedProject};

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

/// Sessions listed on the page; the charts cover every one.
const RECENT_SESSIONS: usize = 30;

struct Day {
    label: String,
    words: f64,
}

pub struct HomePanel {
    project: SharedProject,
    daily: Entity<InputState>,
    manuscript: Entity<InputState>,
    deadline: Entity<InputState>,
    task: Entity<InputState>,
    /// The only tab in the centre, so the tab bar needs its own close button.
    sole_tab: bool,
    pub focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl HomePanel {
    pub fn new(project: SharedProject, window: &mut Window, cx: &mut Context<Self>) -> Self {
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

        Self {
            project,
            daily,
            manuscript,
            deadline,
            task,
            sole_tab: false,
            focus: cx.focus_handle(),
            _subs: subs,
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
        let mut today_box = section("Today", cx).child(
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
        let mut goal_box = section("Goals", cx);
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
        let structure = section("Manuscript", cx)
            .child(div().text_xs().text_color(muted).child(if status_line.is_empty() {
                "No scenes yet.".to_string()
            } else {
                status_line
            }))
            .child(rows);

        v_flex()
            .gap_3()
            .w_full()
            .child(today_box)
            .child(goal_box)
            .child(structure)
            .into_any_element()
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

        // ---- per week, 26 weeks, Monday-based ----
        let this_monday = today - Duration::days(today.weekday().num_days_from_monday() as i64);
        let mut weeks: Vec<Day> = Vec::new();
        let mut week_total = 0i64;
        let mut active_weeks = 0i64;
        for i in (0..26).rev() {
            let start = this_monday - Duration::weeks(i);
            let end = start + Duration::days(7);
            let words: i64 = sessions
                .iter()
                .filter(|s| s.date >= start && s.date < end)
                .map(|s| s.words().max(0))
                .sum();
            week_total += words;
            if words > 0 {
                active_weeks += 1;
            }
            weeks.push(Day {
                label: start.format("%-d %b").to_string(),
                words: words as f64,
            });
        }
        let week_chart = BarChart::new(weeks)
            .id("words-per-week")
            .band(|d: &Day| d.label.clone())
            .value(|d: &Day| d.words)
            .tick_margin(4)
            .label_axis(true)
            .value_axis(true)
            .grid(true)
            .tooltip_title(|d: &Day| format!("Week of {}", d.label).into())
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

        // ---- every session, newest first ----
        let mut table = v_flex().gap_0().w_full().text_sm();
        for s in sessions.iter().rev().take(RECENT_SESSIONS) {
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
        let table_head = |cx: &App| {
            h_flex()
                .w_full()
                .px_1()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(div().w(px(110.)).child("Date"))
                .child(div().w(px(90.)).text_right().child("Words"))
                .child(div().w(px(90.)).text_right().child("Time"))
                .child(div().flex_1().text_right().child("Manuscript"))
        };

        // ---- chapters with goal progress ----
        let root = p.root(Space::Manuscript);
        let mut chapters = v_flex().gap_0().w_full().text_sm();
        let mut any_goal = false;
        let mut n_chapters = 0usize;
        for chapter in p.children(root) {
            let Ok(node) = p.node(chapter) else { continue };
            if node.kind() != NodeKind::Chapter {
                continue;
            }
            n_chapters += 1;
            let mut words = 0usize;
            let mut scenes = 0usize;
            p.walk(chapter, &mut |_, n| {
                if n.kind() == NodeKind::Scene {
                    scenes += 1;
                    if n.include_in_compile() {
                        words += n.word_count();
                    }
                }
            });
            let goal = node.word_goal().filter(|g| *g > 0);
            any_goal |= goal.is_some();
            let id = chapter;
            let bar: AnyElement = match goal {
                Some(g) => {
                    let pct = (words as f32 / g as f32 * 100.).min(100.);
                    let color = if words as i64 >= g { theme.green } else { theme.primary };
                    h_flex()
                        .w(px(220.))
                        .gap_2()
                        .items_center()
                        .child(
                            div()
                                .flex_1()
                                .child(Progress::new(format!("rep-goal-{chapter}")).value(pct).color(color)),
                        )
                        .child(
                            div()
                                .w(px(80.))
                                .text_right()
                                .text_xs()
                                .text_color(muted)
                                .child(format!("{words} / {g}")),
                        )
                        .into_any_element()
                }
                None => div()
                    .w(px(220.))
                    .text_right()
                    .text_xs()
                    .text_color(muted)
                    .child("no goal")
                    .into_any_element(),
            };
            chapters = chapters.child(
                h_flex()
                    .id(ElementId::Name(format!("rep-ch-{chapter}").into()))
                    .w_full()
                    .px_1()
                    .py_0p5()
                    .gap_3()
                    .items_center()
                    .rounded_sm()
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.secondary))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(node.title()),
                    )
                    .child(
                        div()
                            .w(px(50.))
                            .text_right()
                            .text_color(muted)
                            .child(format!("{scenes} sc")),
                    )
                    .child(div().w(px(70.)).text_right().child(format!("{words}")))
                    .child(bar)
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(HomeEvent::Open(id)))),
            );
        }

        v_flex()
            .gap_3()
            .w_full()
            .child(
                section("Words per day (last 30 days)", cx)
                    .child(summary)
                    .child(div().w_full().h(px(220.)).child(chart)),
            )
            .child(
                section("Words per week (last 26 weeks)", cx)
                    .child(div().text_sm().text_color(muted).child(format!(
                        "{week_total} words · {active_weeks} active week{} · avg {} per active week",
                        if active_weeks == 1 { "" } else { "s" },
                        week_total / active_weeks.max(1)
                    )))
                    .child(div().w_full().h(px(200.)).child(week_chart)),
            )
            .child(
                section("Chapters", cx)
                    .child(div().text_xs().text_color(muted).child(if n_chapters == 0 {
                        "No chapters yet.".to_string()
                    } else if any_goal {
                        "Set a chapter's goal in its meta strip; the bar fills as its scenes grow.".to_string()
                    } else {
                        "No chapter has a word goal yet. Set one in the chapter's meta strip.".to_string()
                    }))
                    .child(
                        h_flex()
                            .w_full()
                            .px_1()
                            .gap_3()
                            .text_xs()
                            .text_color(muted)
                            .child(div().flex_1().child("Chapter"))
                            .child(div().w(px(50.)).text_right().child("Scenes"))
                            .child(div().w(px(70.)).text_right().child("Words"))
                            .child(div().w(px(220.)).text_right().child("Goal")),
                    )
                    .child(chapters),
            )
            .child(
                section("Sessions", cx)
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(if sessions.len() > RECENT_SESSIONS {
                                format!(
                                    "The last {RECENT_SESSIONS} of {} sessions, newest first.",
                                    sessions.len()
                                )
                            } else {
                                format!(
                                    "{} session{}, newest first.",
                                    sessions.len(),
                                    if sessions.len() == 1 { "" } else { "s" }
                                )
                            }),
                    )
                    .child(table_head(cx))
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
        section(&format!("Tasks · {open} open"), cx)
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
        section(&format!("Placeholders · {}", hits.len()), cx)
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

    /// The dock draws no close button on a lone tab, since it will not empty
    /// itself through one. The app allows an empty centre, so the sole tab
    /// offers its own close at the end of the tab bar.
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

    /// The panel draws its own header, so the extra gap the tab group adds
    /// under the tab bar once a second tab opens would only shift the layout.
    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}

impl EventEmitter<HomeEvent> for HomePanel {}

impl Render for HomePanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = v_flex()
            .gap_3()
            .w_full()
            .child(self.render_dashboard(cx))
            .child(self.render_tasks(cx))
            .child(self.render_placeholders(cx))
            .child(self.render_reports(cx));
        v_flex()
            .size_full()
            .track_focus(&self.focus)
            .bg(cx.theme().background)
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
