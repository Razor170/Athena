//! Draws the Skills panel from the mockup Kevin approved
//! (<https://claude.ai/artifact/Dg4WJqf8WohyLhFRmJ8urB>):
//!
//! - Overview: one card per character with the skill in training and when
//!   the queue ends, in as many columns as fit.
//! - Character: like the in-game window. Skill groups on top, the picked
//!   group's skills below, the training queue on the right.
//! - Settings.
//!
//! Drawing only reads the module's data and records clicks; the queue
//! maths lives in `data.rs`.

use eframe::egui::{
    self, Align, Color32, CornerRadius, FontId, Layout, Margin, Rect, RichText, Sense, Stroke,
    Vec2, text::LayoutJob, text::TextWrapping,
};

use super::config::{MIN_POLL_MINUTES, SortBy};
use super::data::{
    self, CharacterSkills, QueueNow, QueueState, duration_text, injectors_needed,
    large_injector_sp, roman,
};
use super::worker::{Command, InjectorPrices};
use super::{Skills, Status};
use crate::localtime;
use crate::static_data::StaticData;

// Colours from the mockup (the same family as Planetary and Finance).
pub(super) const GREEN: Color32 = Color32::from_rgb(0x7c, 0xc4, 0x7c);
pub(super) const RED: Color32 = Color32::from_rgb(0xef, 0x6b, 0x6b);
pub(super) const YELLOW: Color32 = Color32::from_rgb(0xe0, 0xb0, 0x40);
pub(super) const BRIGHT: Color32 = Color32::from_rgb(0xf0, 0xf0, 0xf0);
pub(super) const TEXT: Color32 = Color32::from_rgb(0xc8, 0xc8, 0xc8);
pub(super) const DIM: Color32 = Color32::from_rgb(0x8a, 0x8a, 0x8a);
pub(super) const FAINT: Color32 = Color32::from_rgb(0x6a, 0x6a, 0x6a);
pub(super) const QUEUE_BLUE: Color32 = Color32::from_rgb(0x4f, 0xa3, 0xd9);
pub(super) const INJECTOR: Color32 = Color32::from_rgb(0xc9, 0xa0, 0xff);
pub(super) const CARD_FILL: Color32 = Color32::from_rgb(0x23, 0x23, 0x23);
pub(super) const CARD_BORDER: Color32 = Color32::from_rgb(0x3a, 0x3a, 0x3a);
const CARD_RED: Color32 = Color32::from_rgb(0x7a, 0x2c, 0x28);
const CARD_YELLOW: Color32 = Color32::from_rgb(0x6b, 0x5c, 0x1f);
pub(super) const CURRENT_FILL: Color32 = Color32::from_rgb(0x1f, 0x2a, 0x33);
const BAR_BACK: Color32 = Color32::from_rgb(0x2b, 0x2b, 0x2b);
const QUEUE_BAR: Color32 = Color32::from_rgb(0x3a, 0x5f, 0x80);
pub(super) const GROUP_FILL: Color32 = Color32::from_rgb(0x2b, 0x3a, 0x48);
pub(super) const GROUP_PICKED: Color32 = Color32::from_rgb(0x1c, 0x4a, 0x63);
pub(super) const GROUP_EDGE: Color32 = Color32::from_rgb(0x2c, 0x7d, 0xa3);
pub(super) const ROW_LINE: Color32 = Color32::from_rgb(0x2f, 0x2f, 0x2f);
pub(super) const LEVEL_EMPTY: Color32 = Color32::from_rgb(0x3a, 0x3a, 0x3a);
pub(super) const LEVEL_TRAINED: Color32 = Color32::from_rgb(0xd8, 0xd8, 0xd8);
/// Trained, but an Alpha clone can't use it.
const LEVEL_LIMITED: Color32 = Color32::from_rgb(0x80, 0x80, 0x80);
pub(super) const CHIP_FILL: Color32 = Color32::from_rgb(0x2e, 0x3a, 0x44);
pub(super) const CHIP_TEXT: Color32 = Color32::from_rgb(0xa9, 0xc4, 0xd8);

/// Overview cards are at least this wide, and at most `CARD_MAX_WIDTH`
/// (Kevin's "at most 480 px per column").
const CARD_MIN_WIDTH: f32 = 340.0;
pub(super) const CARD_MAX_WIDTH: f32 = 480.0;
pub(super) const GAP: f32 = 10.0;
/// The Character view: the queue column, and the groups + skills part
/// left of it (room for two skill columns of at most 480 px).
const QUEUE_WIDTH: f32 = 480.0;
const LEFT_MAX_WIDTH: f32 = 972.0;
const LEFT_MIN_WIDTH: f32 = 520.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum View {
    #[default]
    Overview,
    Character,
    Plans,
    Settings,
}

/// What the panel remembers between frames (not saved).
#[derive(Debug, Default)]
pub struct UiState {
    pub view: View,
    /// The character the Character view shows.
    pub character: Option<u64>,
    /// The skill group picked in the Character view.
    pub group: Option<i64>,
    pub search: String,
    pub show_not_injected: bool,
    /// The Overview also shows hidden characters, dimmed.
    pub show_hidden: bool,
}

/// What a click on an Overview card asks for.
enum CardAction {
    Open,
    /// Hide (true) or unhide (false) from the right-click menu.
    Hide(bool),
}

pub(super) fn small(text: impl Into<String>) -> RichText {
    RichText::new(text).size(12.0)
}

/// The panel.
pub fn show(module: &mut Skills, ui: &mut egui::Ui, now: i64) {
    ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
    view_tabs(module, ui, now);
    for problem in &module.problems {
        ui.colored_label(YELLOW, small(problem));
    }
    if module.ui.view == View::Settings {
        settings_view(module, ui);
        return;
    }
    if module.ui.view == View::Plans {
        match module.static_data.clone() {
            Some(static_data) => super::plans_ui::show(module, ui, &static_data, now),
            None => {
                ui.colored_label(YELLOW, "Skill plans need static.sqlite.");
            }
        }
        return;
    }
    if !crate::auth::is_configured() {
        ui.colored_label(YELLOW, "EVE login isn't set up in this build.");
        return;
    }
    if module.characters.is_empty() {
        ui.label("Log in your characters on the Characters tab, and their skills show up here.");
        return;
    }
    let Some(static_data) = module.static_data.clone() else {
        return;
    };
    if !module.loaded {
        ui.label(small("Reading saved skill data…").color(DIM));
    }
    match module.ui.view {
        View::Overview => overview(module, ui, now),
        View::Character => character_view(module, ui, &static_data, now),
        View::Plans | View::Settings => {}
    }
}

fn view_tabs(module: &mut Skills, ui: &mut egui::Ui, now: i64) {
    ui.horizontal(|ui| {
        let view = &mut module.ui.view;
        ui.selectable_value(view, View::Overview, "Overview");
        ui.selectable_value(view, View::Character, "Character");
        ui.selectable_value(view, View::Plans, "Plans");
        ui.selectable_value(view, View::Settings, "Settings");
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui
                .add_enabled(module.can_refresh(now), egui::Button::new(small("Refresh")))
                .on_hover_text("Ask ESI for every character's skills now.")
                .on_disabled_hover_text("Just refreshed. Try again in a minute.")
                .clicked()
            {
                module.refresh_now();
            }
            let text = match module.checked {
                Some(at) => format!(
                    "Asked ESI {} · every {} min",
                    ago(now - at),
                    module.settings.poll_secs() / 60
                ),
                None => "Asking ESI…".to_string(),
            };
            ui.label(small(text).color(DIM));
        });
    });
}

// ---------------------------------------------------------------------------
// Overview

fn overview(module: &mut Skills, ui: &mut egui::Ui, now: i64) {
    // Sort and tag filter.
    let mut tags: Vec<String> = module
        .characters
        .iter()
        .flat_map(|c| c.tags.iter().cloned())
        .collect();
    tags.sort();
    tags.dedup();
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        ui.label(small("Sort").color(DIM));
        let sort = &mut module.settings.sort;
        changed |= ui
            .selectable_value(sort, SortBy::QueueEnd, small("Queue end"))
            .on_hover_text("Not training first, then the queue that ends soonest")
            .changed();
        changed |= ui
            .selectable_value(sort, SortBy::Name, small("Name"))
            .changed();
        changed |= ui
            .selectable_value(sort, SortBy::SkillPoints, small("SP"))
            .changed();
        if !tags.is_empty() {
            ui.add_space(10.0);
            ui.label(small("Tag").color(DIM));
            for tag in &tags {
                let picked = module.settings.tag.as_ref() == Some(tag);
                if ui
                    .add(egui::Button::selectable(picked, small(tag)).corner_radius(8))
                    .on_hover_text("Only show characters with this tag (click again for all)")
                    .clicked()
                {
                    module.settings.tag = if picked { None } else { Some(tag.clone()) };
                    changed = true;
                }
            }
        }
        // Only there while something is hidden. With nothing hidden it
        // switches off, so the next character you hide disappears.
        let hidden = module.hidden_count();
        if hidden == 0 {
            module.ui.show_hidden = false;
        }
        if hidden > 0 {
            ui.add_space(10.0);
            let on = module.ui.show_hidden;
            if ui
                .add(egui::Button::selectable(
                    on,
                    small(format!("Show hidden ({hidden})")),
                ))
                .on_hover_text("Show hidden characters too, dimmed, so you can unhide them")
                .clicked()
            {
                module.ui.show_hidden = !on;
            }
        }
    });
    if changed {
        module.save_settings();
    }

    let shown: Vec<(u64, String, Vec<String>)> = module
        .shown_characters(now, module.ui.show_hidden)
        .into_iter()
        .map(|c| (c.id, c.name.clone(), c.tags.clone()))
        .collect();
    // Hidden characters never count, even while "Show hidden" is on.
    let count = |which: Status| {
        shown
            .iter()
            .filter(|(id, _, _)| !module.is_hidden(*id) && module.training(*id, now) == which)
            .count()
    };
    let visible = shown
        .iter()
        .filter(|(id, _, _)| !module.is_hidden(*id))
        .count();
    let hidden = module.hidden_count();
    ui.horizontal_wrapped(|ui| {
        ui.label(small(format!("{visible} characters")).color(DIM));
        dot_label(ui, GREEN, format!("{} training", count(Status::Training)));
        dot_label(
            ui,
            YELLOW,
            format!(
                "{} ends within {} h",
                count(Status::EndsSoon),
                module.settings.soon_hours
            ),
        );
        dot_label(ui, RED, format!("{} not training", count(Status::Stopped)));
        if hidden > 0 {
            ui.label(small(format!("· {hidden} hidden")).color(FAINT));
        }
    });

    let mut open = None;
    let mut hide = None;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let available = ui.available_width();
            let columns = (((available + GAP) / (CARD_MIN_WIDTH + GAP)).floor() as usize)
                .clamp(1, shown.len().max(1));
            let total =
                (CARD_MAX_WIDTH * columns as f32 + GAP * (columns as f32 - 1.0)).min(available);
            ui.set_max_width(total);
            // `columns` splits the width; cards go left to right, then the
            // next row, so the sort order reads across.
            ui.columns(columns, |cols| {
                for (index, (id, name, tags)) in shown.iter().enumerate() {
                    let col = &mut cols[index % columns];
                    col.spacing_mut().item_spacing = Vec2::new(6.0, GAP);
                    match character_card(module, col, *id, name, tags, now) {
                        Some(CardAction::Open) => open = Some(*id),
                        Some(CardAction::Hide(on)) => hide = Some((*id, on)),
                        None => {}
                    }
                }
            });
        });
    if let Some((id, on)) = hide {
        module.set_hidden(id, on);
    }
    if let Some(id) = open {
        open_character(module, id);
    }
}

fn open_character(module: &mut Skills, id: u64) {
    module.ui.character = Some(id);
    module.ui.view = View::Character;
}

/// One Overview card. Returns what a click or the right-click menu asks
/// for. A hidden card (only drawn with "Show hidden" on) is dimmed and
/// has no warning colour.
fn character_card(
    module: &Skills,
    ui: &mut egui::Ui,
    id: u64,
    name: &str,
    tags: &[String],
    now: i64,
) -> Option<CardAction> {
    let hidden = module.is_hidden(id);
    let training = module.training(id, now);
    let queue = module.queue_now(id, now);
    let skills = module.skills.get(&id);
    let problem = module.character_problems.get(&id);
    let border = match training {
        _ if hidden => FAINT,
        Status::Stopped => CARD_RED,
        Status::EndsSoon => CARD_YELLOW,
        Status::Training => CARD_BORDER,
    };
    let response = egui::Frame::new()
        .fill(CARD_FILL)
        .stroke(Stroke::new(1.0, border))
        .corner_radius(CornerRadius::same(4))
        .inner_margin(Margin::symmetric(10, 7))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = Vec2::new(6.0, 4.0);
            if hidden {
                ui.set_opacity(0.5);
            }
            // Line 1: name, SP, tags, status. Tags that don't fit become
            // a "+N" chip, so the card never gets wider than its column.
            ui.horizontal(|ui| {
                let (mut status, mut status_color) =
                    status_text(training, queue.as_ref(), problem, now);
                if hidden {
                    status = format!("Hidden · {status}");
                    status_color = DIM;
                }
                let status_width = text_width(ui, &status, 12.0) + 8.0;
                let name_room = (ui.available_width() - status_width - 70.0).max(60.0);
                elided_label(ui, name, name_room, BRIGHT, 14.0);
                if let Some(skills) = skills {
                    ui.label(small(short_sp(skills.total_sp)).color(FAINT));
                }
                let gap = ui.spacing().item_spacing.x;
                let plus_width = chip_width(ui, "+9") + gap;
                let mut room = ui.available_width() - status_width;
                let mut shown = 0;
                for (index, tag) in tags.iter().enumerate() {
                    let width = chip_width(ui, tag) + gap;
                    let more_after = index + 1 < tags.len();
                    if width + if more_after { plus_width } else { 0.0 } > room {
                        break;
                    }
                    chip(ui, tag);
                    room -= width;
                    shown += 1;
                }
                if shown < tags.len() {
                    chip(ui, &format!("+{}", tags.len() - shown)).on_hover_text(tags.join(", "));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let label = ui.label(small(status).color(status_color));
                    if let Some(problem) = problem {
                        label.on_hover_text(problem);
                    }
                });
            });
            let Some(queue) = queue else {
                let text = if problem.is_some_and(|p| p.starts_with("Login expired")) {
                    "Log in again on the Characters tab to see skills."
                } else {
                    "Waiting for ESI…"
                };
                ui.label(small(text).color(FAINT).italics());
                return;
            };
            let Some(first) = queue.rows.first() else {
                ui.horizontal(|ui| {
                    ui.label(small("Nothing in the queue").color(RED));
                    if let Some(skills) = skills
                        && skills.unallocated_sp > 0
                    {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(
                                small(format!(
                                    "{} unallocated SP",
                                    group_digits(skills.unallocated_sp)
                                ))
                                .color(DIM),
                            );
                        });
                    }
                });
                return;
            };
            let paused = queue.state == QueueState::Paused;
            // Line 2: the skill in training.
            ui.horizontal(|ui| {
                let time = if paused {
                    "paused".to_string()
                } else {
                    first
                        .finish
                        .map_or(String::new(), |f| duration_text(f - now))
                };
                let right = 110.0 + 6.0 + text_width(ui, &time, 12.0) + 4.0;
                let name_width = (ui.available_width() - right).max(40.0);
                let skill = skill_name(module, first.skill_id);
                elided_label(
                    ui,
                    &format!("{skill} {}", roman(first.level)),
                    name_width,
                    TEXT,
                    12.5,
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(small(time).color(DIM));
                    bar(ui, 110.0, 6.0, first.progress, QUEUE_BLUE);
                });
            });
            // Line 3: the whole queue.
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.label(small(format!("{} in queue ·", queue.rows.len())).color(DIM));
                match (queue.state, queue.seconds_left) {
                    (QueueState::Training { ends }, Some(left)) => {
                        ui.label(small("ends").color(DIM));
                        ui.label(small(when_text(ends, now)).color(TEXT));
                        let soon = training == Status::EndsSoon;
                        ui.label(small("·").color(DIM));
                        ui.label(small(duration_text(left)).color(if soon { YELLOW } else { DIM }));
                    }
                    (_, Some(left)) => {
                        ui.label(
                            small(format!("paused, about {} left", duration_text(left))).color(RED),
                        );
                    }
                    _ => {
                        ui.label(small("paused").color(RED));
                    }
                }
            });
            // Line 4: queue length against `bar_days`.
            let full = f64::from(module.settings.bar_days.max(1)) * 86_400.0;
            let share = queue.seconds_left.map_or(0.0, |s| s as f64 / full) as f32;
            bar(ui, ui.available_width(), 4.0, share.min(1.0), QUEUE_BAR);
            // With saved data, a problem goes below instead of replacing
            // the status.
            if let Some(problem) = problem {
                let age = skills.map_or(String::new(), |s| {
                    format!(", showing data from {}", ago(now - s.fetched_at))
                });
                ui.label(small(format!("{}{age}", problem_note(problem))).color(YELLOW))
                    .on_hover_text(problem);
            }
        })
        .response;
    let click = ui.interact(
        response.rect,
        ui.id().with(("skills-card", id)),
        Sense::click(),
    );
    let mut action = None;
    // Right-click menu. `context_menu` keeps the menu open across frames
    // by itself; the closure runs only while it is open.
    click.context_menu(|ui| {
        if ui.button("Open in Character view").clicked() {
            action = Some(CardAction::Open);
            ui.close();
        }
        ui.separator();
        let text = if hidden {
            "Unhide"
        } else {
            "Hide from Overview"
        };
        if ui.button(text).clicked() {
            action = Some(CardAction::Hide(!hidden));
            ui.close();
        }
    });
    let hint = if hidden {
        "Open this character (right-click to unhide)"
    } else {
        "Open this character (right-click to hide)"
    };
    if click
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(hint)
        .clicked()
    {
        action = Some(CardAction::Open);
    }
    action
}

/// "Login expired" or "ESI problem", short for a card.
fn problem_note(problem: &str) -> &'static str {
    if problem.starts_with("Login expired") {
        "Login expired"
    } else {
        "ESI problem"
    }
}

/// The status on the right of a card.
fn status_text(
    training: Status,
    queue: Option<&QueueNow>,
    problem: Option<&String>,
    now: i64,
) -> (String, Color32) {
    if let Some(problem) = problem
        && queue.is_none()
    {
        return (problem_note(problem).to_string(), RED);
    }
    match (training, queue.map(|q| q.state)) {
        (_, None) => (String::new(), DIM),
        (_, Some(QueueState::Paused)) => ("Paused".into(), RED),
        (_, Some(QueueState::Empty)) => ("Queue empty".into(), RED),
        (Status::EndsSoon, Some(QueueState::Training { ends })) => {
            (format!("Ends in {}", duration_text(ends - now)), YELLOW)
        }
        _ => ("Training".into(), GREEN),
    }
}

// ---------------------------------------------------------------------------
// Character

fn character_view(module: &mut Skills, ui: &mut egui::Ui, static_data: &StaticData, now: i64) {
    // The picked character, or the first one.
    let picked = module
        .ui
        .character
        .filter(|id| module.characters.iter().any(|c| c.id == *id))
        .or_else(|| module.characters.first().map(|c| c.id));
    let Some(id) = picked else {
        return;
    };
    module.ui.character = Some(id);

    ui.horizontal(|ui| {
        ui.label(small("Character").color(DIM));
        let name = module
            .characters
            .iter()
            .find(|c| c.id == id)
            .map(|c| c.name.clone())
            .unwrap_or_default();
        egui::ComboBox::from_id_salt("skills-character")
            .selected_text(name)
            .width(200.0)
            .show_ui(ui, |ui| {
                for c in &module.characters {
                    ui.selectable_value(&mut module.ui.character, Some(c.id), &c.name);
                }
            });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.checkbox(&mut module.ui.show_not_injected, small("Show not injected"));
            ui.add(
                egui::TextEdit::singleline(&mut module.ui.search)
                    .hint_text("Search all skills")
                    .desired_width(180.0),
            );
        });
    });

    let Some(skills) = module.skills.get(&id) else {
        let text = match module.character_problems.get(&id) {
            Some(problem) => problem.clone(),
            None => "Waiting for ESI…".to_string(),
        };
        ui.colored_label(YELLOW, small(text));
        return;
    };
    if let Some(problem) = module.character_problems.get(&id) {
        ui.colored_label(
            YELLOW,
            small(format!(
                "{problem} Showing the data from {}.",
                ago(now - skills.fetched_at)
            )),
        );
    }
    let queue = module.queue_now(id, now).unwrap_or(QueueNow {
        state: QueueState::Empty,
        rows: Vec::new(),
        seconds_left: Some(0),
        sp_left: 0,
    });
    // The first group, until one is picked.
    if module.ui.group.is_none() {
        module.ui.group = static_data.skill_groups().first().map(|g| g.group_id);
    }

    let mut picked_group = None;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let available = ui.available_width();
            let side_by_side = available >= LEFT_MIN_WIDTH + GAP + 360.0;
            if side_by_side {
                let queue_width = QUEUE_WIDTH.min(available - LEFT_MIN_WIDTH - GAP);
                let left_width = (available - queue_width - GAP).min(LEFT_MAX_WIDTH);
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = GAP;
                    ui.allocate_ui_with_layout(
                        Vec2::new(left_width, 0.0),
                        Layout::top_down(Align::Min),
                        |ui| {
                            ui.set_width(left_width);
                            picked_group = left_part(module, ui, static_data, skills, now);
                        },
                    );
                    ui.allocate_ui_with_layout(
                        Vec2::new(queue_width, 0.0),
                        Layout::top_down(Align::Min),
                        |ui| {
                            ui.set_width(queue_width);
                            queue_panel(module, ui, skills, &queue, now);
                        },
                    );
                });
            } else {
                ui.set_max_width(available);
                picked_group = left_part(module, ui, static_data, skills, now);
                queue_panel(module, ui, skills, &queue, now);
            }
        });
    if let Some(group) = picked_group {
        module.ui.group = Some(group);
        module.ui.search.clear();
    }
}

/// Groups and the picked group's skills. Returns a group that was clicked.
fn left_part(
    module: &Skills,
    ui: &mut egui::Ui,
    static_data: &StaticData,
    skills: &CharacterSkills,
    now: i64,
) -> Option<i64> {
    ui.spacing_mut().item_spacing.y = GAP;
    let mut clicked = None;
    panel(ui, |ui| {
        panel_head(
            ui,
            "Skills",
            &format!("{} total SP", group_digits(skills.total_sp)),
        );
        let columns = if ui.available_width() >= 600.0 { 3 } else { 2 };
        let groups = static_data.skill_groups();
        ui.columns(columns, |cols| {
            // Row by row, like the game: A B C / D E F.
            for (index, group) in groups.iter().enumerate() {
                let col = &mut cols[index % columns];
                col.spacing_mut().item_spacing.y = 3.0;
                if group_cell(module, col, static_data, skills, group, now) {
                    clicked = Some(group.group_id);
                }
            }
        });
        ui.add_space(4.0);
        line(ui);
        ui.horizontal(|ui| {
            ui.label(small("Total").color(DIM));
            ui.label(small(group_digits(skills.total_sp)).color(BRIGHT).strong());
            ui.label(small("SP").color(DIM));
            if skills.unallocated_sp > 0 {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        small(format!(
                            "{} unallocated SP",
                            group_digits(skills.unallocated_sp)
                        ))
                        .color(YELLOW),
                    );
                });
            }
        });
    });
    panel(ui, |ui| skill_list(module, ui, static_data, skills, now));
    clicked
}

/// One skill group with its bar and count. Returns true when clicked.
fn group_cell(
    module: &Skills,
    ui: &mut egui::Ui,
    static_data: &StaticData,
    skills: &CharacterSkills,
    group: &crate::static_data::SkillGroup,
    now: i64,
) -> bool {
    let mut possible = 0;
    let mut have = 0;
    let mut injected = 0;
    let mut queued = false;
    for &skill_id in &group.skill_ids {
        let rank = static_data.skill(skill_id).map_or(1, |s| s.rank);
        possible += data::sp_for_level(rank, 5);
        if let Some(skill) = skills.skill(skill_id) {
            injected += 1;
            have += skill.sp.min(data::sp_for_level(rank, 5));
        }
        queued |= skills.queued_level(skill_id, now).is_some();
    }
    let picked = module.ui.search.trim().is_empty() && module.ui.group == Some(group.group_id);
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), Sense::click());
    let painter = ui.painter_at(rect);
    let share = if possible > 0 {
        have as f32 / possible as f32
    } else {
        0.0
    };
    let fill = Rect::from_min_size(
        rect.min,
        Vec2::new((rect.width() * share).max(2.0), rect.height()),
    );
    painter.rect_filled(fill, 3.0, if picked { GROUP_PICKED } else { GROUP_FILL });
    if picked {
        painter.rect_stroke(
            rect,
            3.0,
            Stroke::new(1.0, GROUP_EDGE),
            egui::StrokeKind::Inside,
        );
    } else if response.hovered() {
        painter.rect_stroke(
            rect,
            3.0,
            Stroke::new(1.0, CARD_BORDER),
            egui::StrokeKind::Inside,
        );
    }
    let count = painter.text(
        rect.right_center() - Vec2::new(6.0, 0.0),
        egui::Align2::RIGHT_CENTER,
        injected.to_string(),
        FontId::proportional(11.5),
        DIM,
    );
    let mut name_right = count.left() - 6.0;
    if queued {
        let square = Rect::from_center_size(
            egui::pos2(count.left() - 9.0, rect.center().y),
            Vec2::splat(6.0),
        );
        painter.rect_filled(square, 0.0, QUEUE_BLUE);
        name_right = square.left() - 4.0;
    }
    let galley = one_line(
        &painter,
        &group.name,
        12.5,
        TEXT,
        name_right - rect.left() - 6.0,
    );
    painter.galley(
        egui::pos2(rect.left() + 6.0, rect.center().y - galley.size().y / 2.0),
        galley,
        TEXT,
    );
    let response = if queued {
        response.on_hover_text("Has skills in the training queue")
    } else {
        response
    };
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
}

/// The skills of the picked group, or of the search, in two columns.
fn skill_list(
    module: &Skills,
    ui: &mut egui::Ui,
    static_data: &StaticData,
    skills: &CharacterSkills,
    now: i64,
) {
    let search = module.ui.search.trim().to_lowercase();
    let (title, ids): (String, Vec<i64>) = if search.is_empty() {
        let group = static_data
            .skill_groups()
            .iter()
            .find(|g| Some(g.group_id) == module.ui.group);
        match group {
            Some(g) => (g.name.clone(), g.skill_ids.clone()),
            None => (String::new(), Vec::new()),
        }
    } else {
        let mut ids: Vec<i64> = static_data
            .skill_groups()
            .iter()
            .flat_map(|g| g.skill_ids.iter().copied())
            .filter(|id| {
                static_data
                    .skill(*id)
                    .is_some_and(|s| s.name.to_lowercase().contains(&search))
            })
            .collect();
        ids.sort_by_key(|id| static_data.skill(*id).map(|s| s.name.clone()));
        (format!("Search \"{}\"", module.ui.search.trim()), ids)
    };
    let injected = ids.iter().filter(|id| skills.skill(**id).is_some()).count();
    let at_five = ids
        .iter()
        .filter(|id| skills.level_at(**id, now) >= 5)
        .count();
    panel_head(ui, &title, &format!("{injected} injected · {at_five} at V"));
    let shown: Vec<i64> = ids
        .into_iter()
        .filter(|id| module.ui.show_not_injected || skills.skill(*id).is_some())
        .collect();
    if shown.is_empty() {
        let text = if search.is_empty() {
            "No skills injected in this group."
        } else {
            "No skill matches."
        };
        ui.label(small(text).color(FAINT).italics());
        return;
    }
    // Two columns when there's room, filled top to bottom like the game.
    let columns = if ui.available_width() >= 600.0 { 2 } else { 1 };
    let per_column = shown.len().div_ceil(columns);
    ui.columns(columns, |cols| {
        for (index, id) in shown.iter().enumerate() {
            let col = &mut cols[index / per_column];
            col.spacing_mut().item_spacing.y = 2.0;
            skill_row(col, static_data, skills, *id, now);
        }
    });
}

/// One skill: level squares, name, queued level, rank and SP.
fn skill_row(
    ui: &mut egui::Ui,
    static_data: &StaticData,
    skills: &CharacterSkills,
    skill_id: i64,
    now: i64,
) {
    let Some(info) = static_data.skill(skill_id) else {
        return;
    };
    let have = skills.skill(skill_id);
    let level = skills.level_at(skill_id, now);
    // Active: what an Alpha clone may use. Levels finished since ESI's
    // copy count as active.
    let active = have.map_or(0, |s| {
        if level > s.trained_level {
            level
        } else {
            s.active_level
        }
    });
    let queued = skills.queued_level(skill_id, now).unwrap_or(0);
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 20.0), Sense::hover());
    let painter = ui.painter_at(rect);
    let squares_right = level_squares(
        &painter,
        egui::pos2(rect.left() + 2.0, rect.center().y),
        LevelMarks {
            trained: level,
            active,
            queued,
            injected: have.is_some(),
        },
    );
    // Right side first: SP, rank, queued note.
    let (sp_text, sp_color) = match have {
        Some(s) => (format!("{} SP", group_digits(s.sp)), DIM),
        None => ("not injected".to_string(), FAINT),
    };
    let sp = painter.text(
        rect.right_center() - Vec2::new(2.0, 0.0),
        egui::Align2::RIGHT_CENTER,
        sp_text,
        FontId::proportional(11.5),
        sp_color,
    );
    let rank = painter.text(
        egui::pos2(sp.left() - 8.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        format!("×{}", info.rank),
        FontId::proportional(11.0),
        FAINT,
    );
    let mut name_right = rank.left() - 8.0;
    if queued > level {
        let note = painter.text(
            egui::pos2(name_right, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            format!("queued to {}", roman(queued)),
            FontId::proportional(11.5),
            QUEUE_BLUE,
        );
        name_right = note.left() - 8.0;
    }
    let color = if have.is_some() { TEXT } else { FAINT };
    let galley = one_line(
        &painter,
        &info.name,
        12.5,
        color,
        name_right - squares_right - 8.0,
    );
    painter.galley(
        egui::pos2(squares_right + 8.0, rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
    if have.is_some_and(|s| s.active_level < s.trained_level) {
        response.on_hover_text("Grey levels are trained but can't be used with an Alpha clone.");
    }
}

/// What the five level squares show.
struct LevelMarks {
    trained: i64,
    active: i64,
    queued: i64,
    injected: bool,
}

/// Draws five level squares starting at `left_center`, returns their right
/// edge. White = trained, grey = trained but limited (Alpha), blue =
/// queued, dark = not trained, outline only = not injected.
fn level_squares(painter: &egui::Painter, left_center: egui::Pos2, marks: LevelMarks) -> f32 {
    const SIZE: f32 = 8.0;
    const STEP: f32 = 10.0;
    for level in 1..=5 {
        let center = egui::pos2(
            left_center.x + SIZE / 2.0 + (level - 1) as f32 * STEP,
            left_center.y,
        );
        let square = Rect::from_center_size(center, Vec2::splat(SIZE));
        if !marks.injected {
            painter.rect_stroke(
                square,
                0.0,
                Stroke::new(1.0, LEVEL_EMPTY),
                egui::StrokeKind::Inside,
            );
            continue;
        }
        let color = if level <= marks.active {
            LEVEL_TRAINED
        } else if level <= marks.trained {
            LEVEL_LIMITED
        } else if level <= marks.queued {
            QUEUE_BLUE
        } else {
            LEVEL_EMPTY
        };
        painter.rect_filled(square, 0.0, color);
    }
    left_center.x + 4.0 * STEP + SIZE
}

/// The training queue with its footer.
fn queue_panel(
    module: &Skills,
    ui: &mut egui::Ui,
    skills: &CharacterSkills,
    queue: &QueueNow,
    now: i64,
) {
    panel(ui, |ui| {
        panel_head(
            ui,
            "Training queue",
            &format!("{} / {}", queue.rows.len(), data::QUEUE_MAX),
        );
        match queue.state {
            QueueState::Paused => banner(
                ui,
                YELLOW,
                "Queue is paused. Start it in EVE to continue training.",
            ),
            QueueState::Empty => banner(
                ui,
                RED,
                "Nothing in the queue. This character is not training.",
            ),
            QueueState::Training { .. } => {}
        }
        for (index, row) in queue.rows.iter().enumerate() {
            queue_row(module, ui, index, row, queue.state, now);
        }
        if queue.rows.is_empty() {
            return;
        }
        ui.add_space(4.0);
        line(ui);
        ui.label(small("Training time").color(DIM));
        let total = match (queue.state, queue.seconds_left) {
            (QueueState::Paused, Some(s)) => format!("paused, about {}", duration_text(s)),
            (QueueState::Paused, None) => "paused".to_string(),
            (_, Some(s)) => duration_text(s),
            (_, None) => String::new(),
        };
        ui.label(RichText::new(total).size(18.0).color(BRIGHT).strong());
        ui.horizontal(|ui| {
            let ends = match queue.state {
                QueueState::Training { ends } => format!("ends {}", when_text(ends, now)),
                _ => "paused".to_string(),
            };
            ui.label(small(ends).color(DIM));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(small(format!("{} SP in queue", group_digits(queue.sp_left))).color(DIM));
            });
        });
        line(ui);
        let injectors = injectors_needed(skills.total_sp, skills.unallocated_sp, queue.sp_left);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.label(small("Injectors to finish now:").color(DIM));
            if injectors.missing_sp == 0 {
                ui.label(
                    small(format!(
                        "none, {} unallocated SP is enough",
                        group_digits(skills.unallocated_sp)
                    ))
                    .color(DIM),
                );
                return;
            }
            let mut parts = Vec::new();
            if injectors.large > 0 {
                parts.push(format!("{} large", injectors.large));
            }
            if injectors.small > 0 {
                parts.push(format!("{} small", injectors.small));
            }
            ui.label(small(parts.join(" + ")).color(INJECTOR).strong());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let after = if skills.unallocated_sp > 0 {
                    format!(
                        ", after {} unallocated",
                        group_digits(skills.unallocated_sp)
                    )
                } else {
                    String::new()
                };
                ui.label(
                    small(format!("{} SP{after}", group_digits(injectors.missing_sp))).color(DIM),
                );
            });
        });
        if injectors.missing_sp > 0 {
            injector_cost(ui, module.injector_prices, injectors, now);
            ui.label(
                RichText::new(format!(
                    "A large injector gives {} SP at {} SP (less from 5m, 50m and 80m SP on), \
                     a small one a fifth.",
                    group_digits(large_injector_sp(skills.total_sp + skills.unallocated_sp)),
                    short_sp(skills.total_sp + skills.unallocated_sp).trim_end_matches(" SP"),
                ))
                .size(11.0)
                .color(FAINT),
            );
        }
    });
}

/// "≈ 2.41b ISK at Jita sell: 3 × 745.2m + 4 × 160.1m · prices from 08:12".
fn injector_cost(
    ui: &mut egui::Ui,
    prices: Option<InjectorPrices>,
    injectors: data::Injectors,
    now: i64,
) {
    let Some(prices) = prices else {
        ui.label(small("Jita injector prices: asking ESI…").color(FAINT));
        return;
    };
    let missing = (injectors.large > 0 && prices.large.is_none())
        || (injectors.small > 0 && prices.small.is_none());
    if missing {
        ui.label(small("No Jita sell order for one of the injectors right now.").color(FAINT));
        return;
    }
    let large = prices.large.unwrap_or(0.0);
    let small_price = prices.small.unwrap_or(0.0);
    let total = large * f64::from(injectors.large) + small_price * f64::from(injectors.small);
    let mut parts = Vec::new();
    if injectors.large > 0 {
        parts.push(format!("{} × {}", injectors.large, isk_text(large)));
    }
    if injectors.small > 0 {
        parts.push(format!("{} × {}", injectors.small, isk_text(small_price)));
    }
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.label(small("≈").color(DIM));
        ui.label(
            small(format!("{} ISK", isk_text(total)))
                .color(INJECTOR)
                .strong(),
        );
        ui.label(small(format!("at Jita sell: {}", parts.join(" + "))).color(DIM));
        let age = if now - prices.fetched_at > 2 * 3600 {
            format!("· prices {}", ago(now - prices.fetched_at))
        } else {
            format!("· prices from {}", clock_text(prices.fetched_at))
        };
        ui.label(small(age).color(FAINT))
            .on_hover_text("The lowest sell order at Jita 4-4, checked once an hour.");
    });
}

/// "2.41b", "745.2m", "950k", "12,000".
fn isk_text(isk: f64) -> String {
    if isk >= 1e9 {
        format!("{:.2}b", isk / 1e9)
    } else if isk >= 1e6 {
        format!("{:.1}m", isk / 1e6)
    } else if isk >= 1e4 {
        format!("{:.0}k", isk / 1e3)
    } else {
        group_digits(isk.round() as i64)
    }
}

/// "08:12" in local time.
fn clock_text(unix: i64) -> String {
    let seconds = localtime::to_local(unix).rem_euclid(86_400);
    format!("{:02}:{:02}", seconds / 3600, seconds % 3600 / 60)
}

/// One queue entry: position, name and level squares, time; then progress
/// (first entry) and when it's done.
fn queue_row(
    module: &Skills,
    ui: &mut egui::Ui,
    index: usize,
    row: &data::QueueRow,
    state: QueueState,
    now: i64,
) {
    let first = index == 0;
    let frame = egui::Frame::new()
        .fill(if first {
            CURRENT_FILL
        } else {
            Color32::TRANSPARENT
        })
        .corner_radius(CornerRadius::same(4))
        .inner_margin(Margin::symmetric(4, 4));
    frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing = Vec2::new(6.0, 2.0);
        let duration = match (state, row.seconds_left) {
            (QueueState::Paused, Some(s)) => format!("~{}", duration_text(s)),
            (_, Some(s)) => duration_text(s),
            (_, None) => String::new(),
        };
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 18.0), Sense::hover());
        let painter = ui.painter_at(rect);
        painter.text(
            egui::pos2(rect.left() + 18.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            (index + 1).to_string(),
            FontId::proportional(11.0),
            FAINT,
        );
        let time = painter.text(
            rect.right_center() - Vec2::new(2.0, 0.0),
            egui::Align2::RIGHT_CENTER,
            duration,
            FontId::proportional(12.0),
            DIM,
        );
        // Name first (elided to leave room for the squares), then squares.
        let name = format!("{} {}", skill_name(module, row.skill_id), roman(row.level));
        let name_left = rect.left() + 26.0;
        let max_name = (time.left() - 12.0 - 50.0 - 8.0 - name_left).max(20.0);
        let galley = one_line(&painter, &name, 12.5, TEXT, max_name);
        let name_width = galley.size().x;
        painter.galley(
            egui::pos2(name_left, rect.center().y - galley.size().y / 2.0),
            galley,
            TEXT,
        );
        level_squares(
            &painter,
            egui::pos2(name_left + name_width + 8.0, rect.center().y),
            LevelMarks {
                trained: row.level - 1,
                active: row.level - 1,
                queued: row.level,
                injected: true,
            },
        );
        // Second line.
        ui.horizontal(|ui| {
            ui.add_space(26.0);
            ui.spacing_mut().item_spacing.x = 6.0;
            if first {
                bar(ui, 140.0, 6.0, row.progress, QUEUE_BLUE);
                ui.label(
                    small(format!("{:.0}% ·", row.progress * 100.0))
                        .size(11.5)
                        .color(FAINT),
                );
            }
            let done = match row.finish {
                Some(finish) => format!("done {}", when_text(finish, now)),
                None => "paused".to_string(),
            };
            ui.label(RichText::new(done).size(11.5).color(FAINT));
        });
    });
}

// ---------------------------------------------------------------------------
// Settings

fn settings_view(module: &mut Skills, ui: &mut egui::Ui) {
    let mut changed = false;
    let mut poll_changed = false;
    egui::Frame::new()
        .fill(CARD_FILL)
        .stroke(Stroke::new(1.0, CARD_BORDER))
        .corner_radius(CornerRadius::same(4))
        .inner_margin(Margin::same(10))
        .show(ui, |ui| {
            ui.set_max_width(CARD_MAX_WIDTH);
            egui::Grid::new("skills-settings")
                .num_columns(2)
                .spacing(Vec2::new(12.0, 8.0))
                .show(ui, |ui| {
                    let s = &mut module.settings;
                    ui.label(small("Queue ends soon (yellow)").color(DIM));
                    ui.horizontal(|ui| {
                        changed |= ui
                            .add(egui::DragValue::new(&mut s.soon_hours).range(1..=240))
                            .changed();
                        ui.label(small("hours"));
                    });
                    ui.end_row();

                    ui.label(small("Queue bar scale").color(DIM))
                        .on_hover_text("The thin bar on Overview cards is full at this length");
                    ui.horizontal(|ui| {
                        changed |= ui
                            .add(egui::DragValue::new(&mut s.bar_days).range(1..=90))
                            .changed();
                        ui.label(small("days"));
                    });
                    ui.end_row();

                    ui.label(small("Badge on Skills button").color(DIM));
                    changed |= ui
                        .checkbox(&mut s.badge, small("number of characters not training"))
                        .changed();
                    ui.end_row();

                    ui.label(small("Check ESI every").color(DIM));
                    ui.horizontal(|ui| {
                        let edit = ui.add(
                            egui::DragValue::new(&mut s.poll_minutes).range(MIN_POLL_MINUTES..=120),
                        );
                        // Only on release, so dragging doesn't send a
                        // command per pixel.
                        if edit.changed() {
                            changed = true;
                        }
                        if edit.drag_stopped() || (edit.changed() && !edit.dragged()) {
                            poll_changed = true;
                        }
                        ui.label(small("minutes"));
                    });
                    ui.end_row();

                    ui.label(small("Characters").color(DIM));
                    changed |= ui
                        .checkbox(
                            &mut s.hide_without_data,
                            small("hide characters with no skills data (login expired)"),
                        )
                        .changed();
                    ui.end_row();
                });
        });
    if changed {
        module.save_settings();
    }
    hidden_list(module, ui);
    if poll_changed {
        module
            .worker
            .send(Command::SetPollSecs(module.settings.poll_secs()));
    }
}

/// "Hidden on Overview": one checkbox per character.
fn hidden_list(module: &mut Skills, ui: &mut egui::Ui) {
    if module.characters.is_empty() {
        return;
    }
    ui.add_space(8.0);
    let mut toggled = None;
    egui::Frame::new()
        .fill(CARD_FILL)
        .stroke(Stroke::new(1.0, CARD_BORDER))
        .corner_radius(CornerRadius::same(4))
        .inner_margin(Margin::same(10))
        .show(ui, |ui| {
            ui.set_max_width(CARD_MAX_WIDTH);
            ui.label(RichText::new("Hidden on Overview").strong().color(BRIGHT));
            ui.label(
                small(
                    "Hidden characters get no red or yellow card and don't count in the \
                     badge. Their skills stay visible in the Character tab.",
                )
                .color(DIM),
            );
            ui.add_space(4.0);
            for c in &module.characters {
                let mut on = module.is_hidden(c.id);
                ui.horizontal(|ui| {
                    if ui.checkbox(&mut on, small(&c.name)).changed() {
                        toggled = Some((c.id, on));
                    }
                    if !c.tags.is_empty() {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(small(c.tags.join(", ")).color(FAINT));
                        });
                    }
                });
            }
        });
    if let Some((id, on)) = toggled {
        module.set_hidden(id, on);
    }
}

// ---------------------------------------------------------------------------
// Small helpers

pub(super) fn panel(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(CARD_FILL)
        .stroke(Stroke::new(1.0, CARD_BORDER))
        .corner_radius(CornerRadius::same(4))
        .inner_margin(Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = Vec2::new(6.0, 4.0);
            add(ui);
        });
}

pub(super) fn panel_head(ui: &mut egui::Ui, title: &str, sub: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).size(14.0).color(BRIGHT).strong());
        ui.label(small(sub).color(FAINT));
    });
    line(ui);
}

pub(super) fn line(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, ROW_LINE);
}

pub(super) fn banner(ui: &mut egui::Ui, color: Color32, text: &str) {
    egui::Frame::new()
        .stroke(Stroke::new(1.0, color))
        .corner_radius(CornerRadius::same(4))
        .inner_margin(Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(small(text).color(color));
        });
}

/// A thin progress bar `share` (0..1) full.
fn bar(ui: &mut egui::Ui, width: f32, height: f32, share: f32, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height.max(6.0)), Sense::hover());
    let rect = Rect::from_center_size(rect.center(), Vec2::new(width, height));
    let painter = ui.painter();
    painter.rect_filled(rect, height / 2.0, BAR_BACK);
    let filled = Rect::from_min_size(
        rect.min,
        Vec2::new(rect.width() * share.clamp(0.0, 1.0), height),
    );
    if filled.width() > 0.5 {
        painter.rect_filled(filled, height / 2.0, color);
    }
}

pub(super) fn chip(ui: &mut egui::Ui, text: &str) -> egui::Response {
    egui::Frame::new()
        .fill(CHIP_FILL)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(6, 0))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(10.5).color(CHIP_TEXT));
        })
        .response
}

/// How wide `chip` draws `text`.
fn chip_width(ui: &egui::Ui, text: &str) -> f32 {
    text_width(ui, text, 10.5) + 12.0
}

/// A small coloured dot and a dim label after it (the font has no "●").
fn dot_label(ui: &mut egui::Ui, color: Color32, text: String) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let (rect, _) = ui.allocate_exact_size(Vec2::new(8.0, 14.0), Sense::hover());
        ui.painter().circle_filled(rect.center(), 3.5, color);
        ui.label(small(text).color(DIM));
    });
}

/// A label cut with "…" to `width`.
pub(super) fn elided_label(ui: &mut egui::Ui, text: &str, width: f32, color: Color32, size: f32) {
    let galley = one_line(ui.painter(), text, size, color, width);
    let (rect, response) = ui.allocate_exact_size(galley.size(), Sense::hover());
    let elided = galley.elided;
    ui.painter().galley(rect.min, galley, color);
    if elided {
        response.on_hover_text(text);
    }
}

pub(super) fn text_width(ui: &egui::Ui, text: &str, size: f32) -> f32 {
    ui.painter()
        .layout_no_wrap(text.to_string(), FontId::proportional(size), DIM)
        .size()
        .x
}

/// One line of text that ends in "…" if it doesn't fit in `width`.
pub(super) fn one_line(
    painter: &egui::Painter,
    text: &str,
    size: f32,
    color: Color32,
    width: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = LayoutJob::simple_singleline(text.to_string(), FontId::proportional(size), color);
    job.wrap = TextWrapping {
        max_width: width.max(1.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    painter.layout_job(job)
}

fn skill_name(module: &Skills, skill_id: i64) -> String {
    module
        .static_data
        .as_deref()
        .and_then(|d| d.skill(skill_id))
        .map_or_else(|| format!("Skill {skill_id}"), |s| s.name.clone())
}

/// "124.8m SP".
fn short_sp(sp: i64) -> String {
    format!("{:.1}m SP", sp as f64 / 1_000_000.0)
}

/// "124,779,397".
pub(super) fn group_digits(value: i64) -> String {
    let digits = value.unsigned_abs().to_string();
    let mut out = String::new();
    for (index, c) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if value < 0 {
        out.insert(0, '-');
    }
    out
}

/// "just now", "4 min ago", "5 h ago", "2 d ago".
pub(super) fn ago(seconds: i64) -> String {
    let minutes = seconds.max(0) / 60;
    if minutes < 1 {
        "just now".to_string()
    } else if minutes < 60 {
        format!("{minutes} min ago")
    } else if minutes < 48 * 60 {
        format!("{} h ago", minutes / 60)
    } else {
        format!("{} d ago", minutes / (24 * 60))
    }
}

/// "today 18:48", "tomorrow 02:20" or "Sat 28 Nov 19:08", in local time.
pub(super) fn when_text(unix: i64, now: i64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    // 1 Jan 1970 was a Thursday.
    const WEEKDAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    let local = localtime::to_local(unix);
    let day = local.div_euclid(86_400);
    let today = localtime::to_local(now).div_euclid(86_400);
    let seconds = local.rem_euclid(86_400);
    let clock = format!("{:02}:{:02}", seconds / 3600, seconds % 3600 / 60);
    if day == today {
        return format!("today {clock}");
    }
    if day == today + 1 {
        return format!("tomorrow {clock}");
    }
    let (_, month, day_of_month) = localtime::civil_from_days(day);
    format!(
        "{} {day_of_month} {} {clock}",
        WEEKDAYS[day.rem_euclid(7) as usize],
        MONTHS[(month - 1) as usize]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isk_reads_short() {
        assert_eq!(isk_text(2_412_000_000.0), "2.41b");
        assert_eq!(isk_text(745_230_000.0), "745.2m");
        assert_eq!(isk_text(950_000.0), "950k");
        assert_eq!(isk_text(1_500.0), "1,500");
    }

    #[test]
    fn digits_are_grouped() {
        assert_eq!(group_digits(124_779_397), "124,779,397");
        assert_eq!(group_digits(355_000), "355,000");
        assert_eq!(group_digits(999), "999");
        assert_eq!(group_digits(0), "0");
    }

    #[test]
    fn when_text_names_today_and_tomorrow() {
        // 2026-11-18 08:00 UTC (local time is UTC in Linux test builds).
        let now = 1_794_960_000 + 8 * 3600;
        assert_eq!(when_text(now + 3600, now), "today 09:00");
        assert_eq!(when_text(now + 20 * 3600, now), "tomorrow 04:00");
        assert_eq!(when_text(now + 10 * 86_400, now), "Sat 28 Nov 08:00");
    }
}
