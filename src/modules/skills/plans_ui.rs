//! The Plans view, from the mockup Kevin approved
//! (<https://claude.ai/artifact/VuUXZde27EWymHoDVijyzg>, v3):
//!
//! - a menu bar: Plan (New, Saved plans, Import, Copy for EVE, Export),
//!   Edit (Rename, Duplicate, Remove trained skills, Delete), Save, Undo;
//! - three columns: the skill groups and the picked group's skills (click
//!   a level square to add or remove it), the open plan, and the check
//!   against a character with implant sets and the best remap.
//!
//! Plans are saved in `skills_plans.toml`. The open plan is a working
//! copy: changes stay unsaved until Save, and Undo steps back through
//! them. The maths lives in `plans.rs`.

use eframe::egui::{
    self, Align, Color32, CornerRadius, FontId, Key, KeyboardShortcut, Layout, Margin, Modifiers,
    Rect, RichText, Sense, Stroke, Vec2,
};

use super::Skills;
use super::data::{duration_text, roman};
use super::plans::{
    self, ATTRIBUTE_NAMES, AttributeSet, Draft, FRESH_ATTRIBUTES, Plan, PlansFile, Standing,
};
use super::ui::{
    BRIGHT, CARD_BORDER, CARD_MAX_WIDTH, CURRENT_FILL, DIM, FAINT, GAP, GREEN, GROUP_EDGE,
    GROUP_FILL, GROUP_PICKED, INJECTOR, LEVEL_EMPTY, LEVEL_TRAINED, QUEUE_BLUE, RED, ROW_LINE,
    TEXT, YELLOW, group_digits, line, one_line, panel, panel_head, small, when_text,
};
use crate::static_data::StaticData;

/// Each column is at most this wide (Kevin's 480 px), at least this.
const COLUMN_MIN: f32 = 340.0;
/// Pasted plan text bigger than this isn't read.
const MAX_IMPORT_BYTES: usize = 256 * 1024;
/// Short messages ("Copied …") stay this long.
const NOTE_SECS: i64 = 6;
/// The folder (in the data folder) exported plans go to.
const EXPORT_FOLDER: &str = "skill plans";

/// The implant set the plan is checked with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImplantChoice {
    /// +N to all five attributes (0 = none).
    Preset(i64),
    /// The character's active clone.
    Active,
    /// A jump clone, by id.
    Jump(i64),
}

/// What waits for the "unsaved changes" question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pending {
    Open(usize),
    New,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RenameTarget {
    /// The open plan (an undoable edit).
    Open,
    /// A saved plan in the Saved plans window.
    Saved(usize),
}

enum Popup {
    Saved {
        search: String,
        selected: Option<usize>,
    },
    Rename {
        target: RenameTarget,
        text: String,
    },
    Delete {
        index: usize,
    },
    Import {
        text: String,
        name: String,
    },
    Unsaved {
        then: Pending,
    },
}

/// The open plan.
struct Editor {
    /// Which saved plan it is (`None`: never saved).
    index: Option<usize>,
    plan: Plan,
    /// Earlier versions, newest last.
    undo: Vec<Plan>,
}

/// Everything the Plans view keeps.
pub struct PlansState {
    file: PlansFile,
    /// The file couldn't be read or copied aside: don't overwrite it.
    read_only: bool,
    editor: Editor,
    group: Option<i64>,
    search: String,
    /// The character to check against (`None`: from scratch).
    character: Option<u64>,
    implants: ImplantChoice,
    use_remap: bool,
    popup: Option<Popup>,
    /// A short message and until when it shows.
    note: Option<(String, i64)>,
    /// The entry being dragged.
    drag: Option<usize>,
}

impl PlansState {
    /// Reads `skills_plans.toml`. Returns a problem to show, if any.
    pub fn load(data_dir: &std::path::Path, now: i64) -> (PlansState, Option<String>) {
        let (mut file, problem, read_only) = PlansFile::load(data_dir, now);
        // An unsaved plan from last time comes back unsaved.
        let editor = match file.draft.take() {
            Some(Draft { index, plan }) => Editor {
                index: index.filter(|i| *i < file.plans.len()),
                plan,
                undo: Vec::new(),
            },
            None => match file.plans.first() {
                Some(plan) => Editor {
                    index: Some(0),
                    plan: plan.clone(),
                    undo: Vec::new(),
                },
                None => new_editor(),
            },
        };
        let state = PlansState {
            file,
            read_only,
            editor,
            group: None,
            search: String::new(),
            character: None,
            implants: ImplantChoice::Preset(0),
            use_remap: false,
            popup: None,
            note: None,
            drag: None,
        };
        (state, problem)
    }

    /// True when the open plan differs from its saved copy.
    fn dirty(&self) -> bool {
        match self.editor.index.and_then(|i| self.file.plans.get(i)) {
            Some(saved) => {
                saved.name != self.editor.plan.name || saved.entries != self.editor.plan.entries
            }
            None => !self.editor.plan.entries.is_empty(),
        }
    }
}

fn new_editor() -> Editor {
    Editor {
        index: None,
        plan: Plan {
            name: "New plan".to_string(),
            changed: 0,
            entries: Vec::new(),
        },
        undo: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Saving and the plan list.

fn write_file(module: &mut Skills) {
    if module.plans.read_only {
        return;
    }
    if let Ok(text) = toml::to_string(&module.plans.file) {
        module
            .saver
            .save(PlansFile::file_path(&module.data_dir), text);
    }
}

/// On exit: an unsaved open plan is kept as a draft for next time.
pub fn on_exit(module: &mut Skills) {
    let state = &mut module.plans;
    let draft = state.dirty().then(|| Draft {
        index: state.editor.index,
        plan: state.editor.plan.clone(),
    });
    if draft != state.file.draft {
        state.file.draft = draft;
        write_file(module);
    }
}

fn save_open_plan(module: &mut Skills, now: i64) {
    let state = &mut module.plans;
    state.editor.plan.changed = now;
    let plan = state.editor.plan.clone();
    match state.editor.index.filter(|i| *i < state.file.plans.len()) {
        Some(i) => state.file.plans[i] = plan,
        None => {
            state.file.plans.push(plan);
            state.editor.index = Some(state.file.plans.len() - 1);
        }
    }
    state.file.draft = None;
    note(state, now, "Plan saved.");
    write_file(module);
}

/// Does `what` now, or first asks about unsaved changes.
fn switch(module: &mut Skills, what: Pending) {
    if module.plans.dirty() {
        module.plans.popup = Some(Popup::Unsaved { then: what });
    } else {
        do_switch(module, what);
    }
}

fn do_switch(module: &mut Skills, what: Pending) {
    let state = &mut module.plans;
    state.drag = None;
    state.editor = match what {
        Pending::Open(i) => match state.file.plans.get(i) {
            Some(plan) => Editor {
                index: Some(i),
                plan: plan.clone(),
                undo: Vec::new(),
            },
            None => return,
        },
        Pending::New => new_editor(),
    };
}

/// Every change to the open plan goes through here, so Undo can step back.
fn edit(state: &mut PlansState, change: impl FnOnce(&mut Plan)) {
    let before = state.editor.plan.clone();
    change(&mut state.editor.plan);
    if state.editor.plan != before {
        state.editor.undo.push(before);
        if state.editor.undo.len() > plans::UNDO_MAX {
            state.editor.undo.remove(0);
        }
    }
}

fn undo(state: &mut PlansState) {
    if let Some(plan) = state.editor.undo.pop() {
        state.editor.plan = plan;
    }
}

fn delete_saved(module: &mut Skills, index: usize) {
    let state = &mut module.plans;
    if index >= state.file.plans.len() {
        return;
    }
    let name = state.file.plans.remove(index).name;
    match state.editor.index {
        Some(i) if i == index => {
            state.editor = match state.file.plans.first() {
                Some(plan) => Editor {
                    index: Some(0),
                    plan: plan.clone(),
                    undo: Vec::new(),
                },
                None => new_editor(),
            };
        }
        Some(i) if i > index => state.editor.index = Some(i - 1),
        _ => {}
    }
    let now = crate::auth::now_unix();
    note(state, now, &format!("Deleted \"{name}\"."));
    write_file(module);
}

fn note(state: &mut PlansState, now: i64, text: &str) {
    state.note = Some((text.to_string(), now + NOTE_SECS));
}

/// A name not used by another saved plan: "Name", "Name (2)", ...
fn free_name(file: &PlansFile, name: &str) -> String {
    let taken = |n: &str| file.plans.iter().any(|p| p.name.eq_ignore_ascii_case(n));
    if !taken(name) {
        return name.to_string();
    }
    (2..)
        .map(|k| format!("{name} ({k})"))
        .find(|n| !taken(n))
        .unwrap_or_else(|| name.to_string())
}

// ---------------------------------------------------------------------------
// The view.

pub fn show(module: &mut Skills, ui: &mut egui::Ui, data: &StaticData, now: i64) {
    shortcuts(module, ui, now);
    menu_bar(module, ui, data, now);
    // A short message shows in the menu bar until it runs out.
    if let Some((_, until)) = &module.plans.note {
        if *until > now {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs((until - now) as u64));
        } else {
            module.plans.note = None;
        }
    }
    if module.plans.group.is_none() {
        module.plans.group = data.skill_groups().first().map(|g| g.group_id);
    }
    let check = Check::new(module, data, now);

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let available = ui.available_width();
            if available >= 3.0 * COLUMN_MIN + 2.0 * GAP {
                let width = ((available - 2.0 * GAP) / 3.0).min(CARD_MAX_WIDTH);
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = GAP;
                    column(ui, width, |ui| catalogue(module, ui, data, &check));
                    column(ui, width, |ui| editor(module, ui, data, &check));
                    column(ui, width, |ui| check_panel(module, ui, data, &check, now));
                });
            } else {
                ui.set_max_width(available.min(CARD_MAX_WIDTH));
                ui.spacing_mut().item_spacing.y = GAP;
                catalogue(module, ui, data, &check);
                editor(module, ui, data, &check);
                check_panel(module, ui, data, &check, now);
            }
        });
    popups(module, ui, data, now);
}

fn column(ui: &mut egui::Ui, width: f32, add: impl FnOnce(&mut egui::Ui)) {
    ui.allocate_ui_with_layout(Vec2::new(width, 0.0), Layout::top_down(Align::Min), |ui| {
        ui.set_width(width);
        add(ui);
    });
}

fn shortcuts(module: &mut Skills, ui: &mut egui::Ui, now: i64) {
    if module.plans.popup.is_some() {
        return;
    }
    let save = KeyboardShortcut::new(Modifiers::COMMAND, Key::S);
    let new = KeyboardShortcut::new(Modifiers::COMMAND, Key::N);
    let open = KeyboardShortcut::new(Modifiers::COMMAND, Key::O);
    let undo_key = KeyboardShortcut::new(Modifiers::COMMAND, Key::Z);
    // Ctrl+Z in a text box undoes the typing, not the plan.
    let typing = ui.ctx().text_edit_focused();
    let (s, n, o, z) = ui.input_mut(|i| {
        (
            i.consume_shortcut(&save),
            i.consume_shortcut(&new),
            i.consume_shortcut(&open),
            !typing && i.consume_shortcut(&undo_key),
        )
    });
    if s && !module.plans.read_only {
        save_open_plan(module, now);
    }
    if n {
        switch(module, Pending::New);
    }
    if o {
        open_saved_window(&mut module.plans);
    }
    if z {
        undo(&mut module.plans);
    }
}

fn open_saved_window(state: &mut PlansState) {
    state.popup = Some(Popup::Saved {
        search: String::new(),
        selected: state.editor.index,
    });
}

fn menu_bar(module: &mut Skills, ui: &mut egui::Ui, data: &StaticData, now: i64) {
    let dirty = module.plans.dirty();
    egui::Frame::new()
        .fill(Color32::from_rgb(0x1c, 0x1c, 0x1c))
        .stroke(Stroke::new(1.0, CARD_BORDER))
        .corner_radius(CornerRadius::same(4))
        .inner_margin(Margin::symmetric(4, 2))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.menu_button("Plan", |ui| {
                    if ui.button("New plan          Ctrl+N").clicked() {
                        switch(module, Pending::New);
                        ui.close();
                    }
                    if ui.button("Saved plans…    Ctrl+O").clicked() {
                        open_saved_window(&mut module.plans);
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Import…").clicked() {
                        module.plans.popup = Some(Popup::Import {
                            text: String::new(),
                            name: "Imported plan".to_string(),
                        });
                        ui.close();
                    }
                    if ui
                        .button("Copy for EVE")
                        .on_hover_text(
                            "Copies the plan as one \"Skill name level\" line per skill. \
                             Paste it into EVE's skill plan import.",
                        )
                        .clicked()
                    {
                        let text = plans::export_text(&module.plans.editor.plan, data);
                        ui.ctx().copy_text(text);
                        let count = module.plans.editor.plan.entries.len();
                        note(
                            &mut module.plans,
                            now,
                            &format!("Copied {count} lines for EVE's skill plan import."),
                        );
                        ui.close();
                    }
                    if ui.button("Export as .txt file").clicked() {
                        export_file(module, data, now);
                        ui.close();
                    }
                });
                ui.menu_button("Edit", |ui| {
                    if ui.button("Rename…").clicked() {
                        module.plans.popup = Some(Popup::Rename {
                            target: RenameTarget::Open,
                            text: module.plans.editor.plan.name.clone(),
                        });
                        ui.close();
                    }
                    if ui
                        .button("Duplicate")
                        .on_hover_text(
                            "Saves a copy of the open plan (with its changes) and opens it",
                        )
                        .clicked()
                    {
                        duplicate_open(module, now);
                        ui.close();
                    }
                    let can_remove = module.plans.character.is_some();
                    if ui
                        .add_enabled(can_remove, egui::Button::new("Remove trained skills"))
                        .on_disabled_hover_text("Pick a character in Check first")
                        .clicked()
                    {
                        let standing = module
                            .plans
                            .character
                            .and_then(|id| module.skills.get(&id))
                            .map(|s| Standing::of(s, now))
                            .unwrap_or_default();
                        edit(&mut module.plans, |plan| {
                            plan.entries = plans::without_trained(plan, &|id| standing.level(id));
                        });
                        ui.close();
                    }
                    ui.separator();
                    let index = module.plans.editor.index;
                    if ui
                        .add_enabled(
                            index.is_some(),
                            egui::Button::new(RichText::new("Delete plan…").color(RED)),
                        )
                        .clicked()
                    {
                        if let Some(index) = index {
                            module.plans.popup = Some(Popup::Delete { index });
                        }
                        ui.close();
                    }
                });
                if ui.button("Saved plans…").clicked() {
                    open_saved_window(&mut module.plans);
                }
                ui.separator();
                let can_save = dirty && !module.plans.read_only;
                let save = egui::Button::new(if can_save {
                    RichText::new("Save").color(BRIGHT)
                } else {
                    RichText::new("Save")
                })
                .fill(if can_save {
                    GROUP_PICKED
                } else {
                    Color32::TRANSPARENT
                });
                if ui
                    .add_enabled(can_save, save)
                    .on_hover_text("Save the open plan (Ctrl+S)")
                    .on_disabled_hover_text(if module.plans.read_only {
                        "skills_plans.toml couldn't be read, so nothing is saved"
                    } else {
                        "No changes to save"
                    })
                    .clicked()
                {
                    save_open_plan(module, now);
                }
                if ui
                    .add_enabled(
                        !module.plans.editor.undo.is_empty(),
                        egui::Button::new("Undo"),
                    )
                    .on_hover_text("Undo the last change (Ctrl+Z)")
                    .clicked()
                {
                    undo(&mut module.plans);
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if dirty {
                        ui.label(small("unsaved").color(YELLOW));
                    }
                    ui.label(
                        RichText::new(&module.plans.editor.plan.name)
                            .size(12.5)
                            .color(BRIGHT)
                            .strong(),
                    );
                    ui.label(small("Plan:").color(DIM));
                    // Uses what room is left, cut with "…" if needed.
                    if let Some((text, _)) = &module.plans.note {
                        ui.separator();
                        ui.add(egui::Label::new(small(text).color(GREEN)).truncate())
                            .on_hover_text(text);
                    }
                });
            });
        });
}

fn duplicate_open(module: &mut Skills, now: i64) {
    let state = &mut module.plans;
    let mut copy = state.editor.plan.clone();
    copy.name = free_name(&state.file, &format!("{} (copy)", copy.name));
    copy.changed = now;
    state.file.plans.push(copy.clone());
    state.editor = Editor {
        index: Some(state.file.plans.len() - 1),
        plan: copy,
        undo: Vec::new(),
    };
    let name = state.editor.plan.name.clone();
    note(state, now, &format!("Saved a copy as \"{name}\"."));
    write_file(module);
}

/// Writes the open plan to `<data folder>/skill plans/<name>.txt` on the
/// saver thread.
fn export_file(module: &mut Skills, data: &StaticData, now: i64) {
    let folder = module.data_dir.join(EXPORT_FOLDER);
    // Creating a folder is quick, and only happens on this click.
    if let Err(e) = std::fs::create_dir_all(&folder) {
        note(
            &mut module.plans,
            now,
            &format!("Couldn't create {}: {e}", folder.display()),
        );
        return;
    }
    let path = folder.join(plans::export_file_name(&module.plans.editor.plan));
    let text = plans::export_text(&module.plans.editor.plan, data);
    module.saver.save(path.clone(), text);
    note(
        &mut module.plans,
        now,
        &format!("Exported to {}", path.display()),
    );
}

// ---------------------------------------------------------------------------
// What the check column picked, worked out once per frame.

struct ImplantRow {
    choice: ImplantChoice,
    title: String,
    detail: String,
    bonuses: AttributeSet,
}

struct Check {
    standing: Standing,
    /// Attributes without implants.
    base: AttributeSet,
    rows: Vec<ImplantRow>,
    bonuses: AttributeSet,
    /// The best remap (base attributes) and the plan time with it.
    remap: (AttributeSet, f64),
    /// The attributes the times use: base (or remap) plus implants.
    attributes: AttributeSet,
    has_character: bool,
}

impl Check {
    fn new(module: &mut Skills, data: &StaticData, now: i64) -> Check {
        let state = &mut module.plans;
        // A character that left the roster: back to "from scratch".
        if let Some(id) = state.character
            && !module.characters.iter().any(|c| c.id == id)
        {
            state.character = None;
        }
        let skills = state.character.and_then(|id| module.skills.get(&id));
        let standing = skills.map(|s| Standing::of(s, now)).unwrap_or_default();
        let clones = skills.and_then(|s| s.clones.as_ref());
        let active = clones.map_or([0; 5], |c| plans::implant_bonuses(data, &c.active_implants));
        // ESI's attributes include the active clone's implants.
        let base = match skills.and_then(|s| s.attributes) {
            Some(a) => {
                let with = plans::attribute_set(&a);
                std::array::from_fn(|i| with[i] - active[i])
            }
            None => FRESH_ATTRIBUTES,
        };
        let mut rows: Vec<ImplantRow> = (0..=5)
            .map(|k| ImplantRow {
                choice: ImplantChoice::Preset(k),
                title: if k == 0 {
                    "No implants".to_string()
                } else {
                    format!("+{k} set")
                },
                detail: if k == 0 {
                    "clean clone".to_string()
                } else {
                    format!("+{k} to all five attributes")
                },
                bonuses: [k; 5],
            })
            .collect();
        if let Some(clones) = clones {
            rows.push(ImplantRow {
                choice: ImplantChoice::Active,
                title: "Active clone".to_string(),
                detail: plans::bonuses_text(&active),
                bonuses: active,
            });
            for jump in &clones.jump_clones {
                let bonuses = plans::implant_bonuses(data, &jump.implants);
                let place = module
                    .location_names
                    .get(&jump.location_id)
                    .cloned()
                    .unwrap_or_else(|| "location not known yet".to_string());
                rows.push(ImplantRow {
                    choice: ImplantChoice::Jump(jump.id),
                    title: match &jump.name {
                        Some(name) => format!("\"{name}\""),
                        None => "Jump clone".to_string(),
                    },
                    detail: format!("{place} · {}", plans::bonuses_text(&bonuses)),
                    bonuses,
                });
            }
        }
        // The picked set is gone (other character): no implants.
        if !rows.iter().any(|r| r.choice == state.implants) {
            state.implants = ImplantChoice::Preset(0);
        }
        let bonuses = rows
            .iter()
            .find(|r| r.choice == state.implants)
            .map_or([0; 5], |r| r.bonuses);
        let remap = plans::best_remap(data, &state.editor.plan, &standing, &bonuses);
        let has_character = state.character.is_some();
        let from = if state.use_remap && has_character {
            remap.0
        } else {
            base
        };
        let attributes = std::array::from_fn(|i| from[i] + bonuses[i]);
        Check {
            standing,
            base,
            rows,
            bonuses,
            remap,
            attributes,
            has_character,
        }
    }

    /// The attributes for one implant set (with the remap if it's shown).
    fn attributes_with(&self, bonuses: &AttributeSet, use_remap: bool) -> AttributeSet {
        let from = if use_remap && self.has_character {
            self.remap.0
        } else {
            self.base
        };
        std::array::from_fn(|i| from[i] + bonuses[i])
    }
}

// ---------------------------------------------------------------------------
// Left: skill groups and the picked group's skills.

fn catalogue(module: &mut Skills, ui: &mut egui::Ui, data: &StaticData, check: &Check) {
    panel(ui, |ui| {
        let sub = match module.plans.character {
            Some(id) => module
                .characters
                .iter()
                .find(|c| c.id == id)
                .map(|c| c.name.clone())
                .unwrap_or_default(),
            None => "from scratch".to_string(),
        };
        panel_head(ui, "Skills", &sub);
        let plan = &module.plans.editor.plan;
        let groups = data.skill_groups();
        let mut picked = None;
        let columns = if ui.available_width() >= 420.0 { 3 } else { 2 };
        ui.columns(columns, |cols| {
            for (index, group) in groups.iter().enumerate() {
                let col = &mut cols[index % columns];
                col.spacing_mut().item_spacing.y = 3.0;
                let adds = group
                    .skill_ids
                    .iter()
                    .filter(|id| plans::planned_level(plan, **id) > check.standing.level(**id))
                    .count();
                if group_button(
                    col,
                    &group.name,
                    adds,
                    module.plans.group == Some(group.group_id),
                ) {
                    picked = Some(group.group_id);
                }
            }
        });
        if let Some(group) = picked {
            module.plans.group = Some(group);
        }
        ui.add_space(4.0);
        line(ui);
        let Some(group) = groups
            .iter()
            .find(|g| Some(g.group_id) == module.plans.group)
        else {
            return;
        };
        let mut click: Option<(i64, u8)> = None;
        for &skill_id in &group.skill_ids {
            if let Some(level) = catalogue_row(ui, data, plan, &check.standing, skill_id) {
                click = Some((skill_id, level));
            }
        }
        line(ui);
        ui.horizontal_wrapped(|ui| {
            legend(ui, LEVEL_TRAINED, "trained");
            legend(ui, QUEUE_BLUE, "in plan");
            legend(ui, LEVEL_EMPTY, "click to add up to that level");
        });
        if let Some((skill_id, level)) = click {
            let in_plan = plans::planned_level(&module.plans.editor.plan, skill_id);
            if level <= in_plan {
                edit(&mut module.plans, |plan| {
                    plans::remove_from(plan, data, skill_id, level)
                });
            } else {
                edit(&mut module.plans, |plan| {
                    // Plans are shared, so the full prerequisite chain goes in even when the
                    // picked character already has it (trained rows are marked, not hidden).
                    plans::add_with_prereqs(plan, data, skill_id, level, &|_| 0)
                });
            }
        }
    });
}

/// One skill group button with the number of skills the plan adds there.
fn group_button(ui: &mut egui::Ui, name: &str, adds: usize, picked: bool) -> bool {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), Sense::click());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 3.0, if picked { GROUP_PICKED } else { GROUP_FILL });
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
    let mut name_right = rect.right() - 6.0;
    if adds > 0 {
        let count = painter.text(
            rect.right_center() - Vec2::new(6.0, 0.0),
            egui::Align2::RIGHT_CENTER,
            format!("+{adds}"),
            FontId::proportional(11.0),
            QUEUE_BLUE,
        );
        name_right = count.left() - 4.0;
    }
    let galley = one_line(&painter, name, 12.0, TEXT, name_right - rect.left() - 6.0);
    painter.galley(
        egui::pos2(rect.left() + 6.0, rect.center().y - galley.size().y / 2.0),
        galley,
        TEXT,
    );
    let response = if adds > 0 {
        response.on_hover_text(format!("The plan adds {adds} skill(s) here"))
    } else {
        response
    };
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
}

/// One skill with clickable level squares. Returns the level clicked.
fn catalogue_row(
    ui: &mut egui::Ui,
    data: &StaticData,
    plan: &Plan,
    standing: &Standing,
    skill_id: i64,
) -> Option<u8> {
    const SIZE: f32 = 12.0;
    const STEP: f32 = 15.0;
    let info = data.skill(skill_id)?;
    let trained = standing.level(skill_id);
    let planned = plans::planned_level(plan, skill_id);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 20.0), Sense::hover());
    let squares_left = rect.right() - 2.0 - 5.0 * STEP + (STEP - SIZE);
    let mut clicked = None;
    for level in 1..=5u8 {
        let square = Rect::from_min_size(
            egui::pos2(
                squares_left + f32::from(level - 1) * STEP,
                rect.center().y - SIZE / 2.0,
            ),
            Vec2::splat(SIZE),
        );
        let id = ui.id().with(("plan-level", skill_id, level));
        let response = ui.interact(square, id, Sense::click());
        let (fill, hint) = if level <= trained {
            (LEVEL_TRAINED, "Trained")
        } else if level <= planned {
            (
                QUEUE_BLUE,
                "In the plan: click to take it out (from this level up)",
            )
        } else {
            (LEVEL_EMPTY, "Click to add up to this level")
        };
        let painter = ui.painter();
        painter.rect_filled(square, 2.0, fill);
        if response.hovered() && level > trained {
            painter.rect_stroke(
                square,
                2.0,
                Stroke::new(1.0, BRIGHT),
                egui::StrokeKind::Inside,
            );
        }
        let response =
            response.on_hover_text(format!("{} {}: {hint}", info.name, roman(i64::from(level))));
        if response.clicked() && level > trained {
            clicked = Some(level);
        }
    }
    let painter = ui.painter_at(rect);
    let rank = painter.text(
        egui::pos2(squares_left - 8.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        format!("×{}", info.rank),
        FontId::proportional(11.0),
        FAINT,
    );
    let color = if planned > trained { BRIGHT } else { TEXT };
    let galley = one_line(
        &painter,
        &info.name,
        12.5,
        color,
        rank.left() - rect.left() - 10.0,
    );
    painter.galley(
        egui::pos2(rect.left() + 2.0, rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
    clicked
}

fn legend(ui: &mut egui::Ui, color: Color32, text: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let (rect, _) = ui.allocate_exact_size(Vec2::new(10.0, 14.0), Sense::hover());
        ui.painter().rect_filled(
            Rect::from_center_size(rect.center(), Vec2::splat(9.0)),
            2.0,
            color,
        );
        ui.label(RichText::new(text).size(11.0).color(FAINT));
    });
}

// ---------------------------------------------------------------------------
// Middle: the open plan.

fn editor(module: &mut Skills, ui: &mut egui::Ui, data: &StaticData, check: &Check) {
    panel(ui, |ui| {
        let state = &module.plans;
        let sub = match state.editor.index.and_then(|i| state.file.plans.get(i)) {
            Some(saved) if saved.changed > 0 => {
                format!(
                    "saved {}",
                    when_text(saved.changed, crate::auth::now_unix())
                )
            }
            _ => "not saved yet".to_string(),
        };
        panel_head(ui, &state.editor.plan.name.clone(), &sub);
        add_box(module, ui, data, check);
        entry_list(module, ui, data, check);
    });
}

/// "Add skill": type a name, pick a level.
fn add_box(module: &mut Skills, ui: &mut egui::Ui, data: &StaticData, check: &Check) {
    ui.add(
        egui::TextEdit::singleline(&mut module.plans.search)
            .hint_text("Add skill: type a name…")
            .desired_width(f32::INFINITY),
    );
    let search = module.plans.search.trim().to_lowercase();
    if search.is_empty() {
        return;
    }
    let mut hits: Vec<i64> = data
        .skill_groups()
        .iter()
        .flat_map(|g| g.skill_ids.iter().copied())
        .filter(|id| {
            data.skill(*id)
                .is_some_and(|s| s.name.to_lowercase().contains(&search))
        })
        .collect();
    hits.sort_by_key(|id| data.skill(*id).map(|s| s.name.clone()));
    hits.truncate(8);
    let mut add = None;
    egui::Frame::new()
        .fill(CURRENT_FILL)
        .corner_radius(CornerRadius::same(4))
        .inner_margin(Margin::symmetric(6, 4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if hits.is_empty() {
                ui.label(small("No skill matches.").color(FAINT));
            }
            for id in &hits {
                let Some(info) = data.skill(*id) else {
                    continue;
                };
                ui.horizontal(|ui| {
                    ui.label(small(&info.name));
                    ui.label(
                        RichText::new(format!("×{}", info.rank))
                            .size(11.0)
                            .color(FAINT),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        for level in (1..=5u8).rev() {
                            let trained = check.standing.level(*id) >= level;
                            if ui
                                .add_enabled(
                                    !trained,
                                    egui::Button::new(small(roman(i64::from(level)))).small(),
                                )
                                .on_hover_text(format!("Add up to level {level}"))
                                .on_disabled_hover_text("Already trained")
                                .clicked()
                            {
                                add = Some((*id, level));
                            }
                        }
                    });
                });
            }
        });
    if let Some((id, level)) = add {
        // Full chain, as in the catalogue: the plan is shared by all characters.
        edit(&mut module.plans, |plan| {
            plans::add_with_prereqs(plan, data, id, level, &|_| 0)
        });
        module.plans.search.clear();
    }
}

/// What a row in the plan shows besides name and time.
#[derive(PartialEq)]
enum EntryMark {
    None,
    Trained,
    Partly,
    Prereq,
}

fn entry_list(module: &mut Skills, ui: &mut egui::Ui, data: &StaticData, check: &Check) {
    let plan = module.plans.editor.plan.clone();
    if plan.entries.is_empty() {
        ui.label(
            small("Empty. Add skills above, or click level squares on the left.")
                .color(FAINT)
                .italics(),
        );
        return;
    }
    let mut remove = None;
    let mut rects: Vec<Rect> = Vec::with_capacity(plan.entries.len());
    let mut total = 0.0;
    let mut missing_sp = 0;
    let mut trained_count = 0;
    for (index, &entry) in plan.entries.iter().enumerate() {
        let Some(info) = data.skill(entry.skill_id) else {
            continue;
        };
        let sp = plans::entry_sp(data, entry, &check.standing);
        let full = super::data::sp_for_level(info.rank, i64::from(entry.level))
            - super::data::sp_for_level(info.rank, i64::from(entry.level) - 1);
        let secs = plans::entry_secs(data, entry, &check.standing, &check.attributes);
        total += secs;
        missing_sp += sp;
        let mark = if module.plans.character.is_some() && sp == 0 {
            trained_count += 1;
            EntryMark::Trained
        } else if module.plans.character.is_some() && sp < full {
            EntryMark::Partly
        } else if is_prereq(&plan, data, index) {
            EntryMark::Prereq
        } else {
            EntryMark::None
        };
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 20.0), Sense::hover());
        rects.push(rect);
        let painter = ui.painter_at(rect);
        if module.plans.drag == Some(index) {
            painter.rect_filled(rect, 2.0, CURRENT_FILL);
        }
        // Drag handle.
        let handle = Rect::from_min_size(rect.min, Vec2::new(16.0, rect.height()));
        let handle_response = ui
            .interact(handle, ui.id().with(("plan-drag", index)), Sense::drag())
            .on_hover_cursor(egui::CursorIcon::Grab)
            .on_hover_text("Drag to reorder");
        if handle_response.drag_started() {
            module.plans.drag = Some(index);
        }
        // Three short lines: a grip (the font has no handle glyph).
        for dy in [-3.0, 0.0, 3.0] {
            let y = handle.center().y + dy;
            painter.hline(
                (handle.center().x - 4.0)..=(handle.center().x + 4.0),
                y,
                Stroke::new(1.0, FAINT),
            );
        }
        painter.text(
            egui::pos2(rect.left() + 38.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            (index + 1).to_string(),
            FontId::proportional(11.0),
            FAINT,
        );
        // Right side: remove, time, mark.
        let x_rect = Rect::from_center_size(
            egui::pos2(rect.right() - 8.0, rect.center().y),
            Vec2::splat(16.0),
        );
        let x = ui
            .interact(x_rect, ui.id().with(("plan-remove", index)), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("Remove (and what needs it)");
        painter.text(
            x_rect.center(),
            egui::Align2::CENTER_CENTER,
            "×",
            FontId::proportional(13.0),
            if x.hovered() { RED } else { FAINT },
        );
        if x.clicked() {
            remove = Some(index);
        }
        let time = painter.text(
            egui::pos2(x_rect.left() - 6.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            if sp == 0 {
                "–".to_string()
            } else {
                duration_text(secs.round() as i64)
            },
            FontId::proportional(12.0),
            DIM,
        );
        let mut name_right = time.left() - 8.0;
        let chip = match mark {
            EntryMark::Trained => Some(("trained", GREEN, Color32::from_rgb(0x21, 0x33, 0x21))),
            EntryMark::Partly => Some(("partly", YELLOW, Color32::from_rgb(0x2f, 0x2a, 0x14))),
            EntryMark::Prereq => Some(("prereq", DIM, Color32::from_rgb(0x2b, 0x2b, 0x2b))),
            EntryMark::None => None,
        };
        if let Some((text, color, fill)) = chip {
            let galley =
                painter.layout_no_wrap(text.to_string(), FontId::proportional(10.5), color);
            let chip_rect = Rect::from_min_size(
                egui::pos2(name_right - galley.size().x - 12.0, rect.center().y - 7.0),
                Vec2::new(galley.size().x + 12.0, 14.0),
            );
            painter.rect_filled(chip_rect, 7.0, fill);
            painter.galley(
                egui::pos2(
                    chip_rect.left() + 6.0,
                    rect.center().y - galley.size().y / 2.0,
                ),
                galley,
                color,
            );
            name_right = chip_rect.left() - 6.0;
        }
        let name = format!("{} {}", info.name, roman(i64::from(entry.level)));
        let color = match mark {
            EntryMark::Trained => FAINT,
            EntryMark::Prereq => DIM,
            _ => TEXT,
        };
        let galley = one_line(
            &painter,
            &name,
            12.5,
            color,
            name_right - rect.left() - 46.0,
        );
        painter.galley(
            egui::pos2(rect.left() + 46.0, rect.center().y - galley.size().y / 2.0),
            galley.clone(),
            color,
        );
        if mark == EntryMark::Trained {
            let y = rect.center().y;
            painter.line_segment(
                [
                    egui::pos2(rect.left() + 46.0, y),
                    egui::pos2(rect.left() + 46.0 + galley.size().x, y),
                ],
                Stroke::new(1.0, FAINT),
            );
        }
        let rank = painter.text(
            egui::pos2(rect.left() + 50.0 + galley.size().x, rect.center().y),
            egui::Align2::LEFT_CENTER,
            format!("×{}", info.rank),
            FontId::proportional(10.5),
            FAINT,
        );
        let _ = rank;
        // A thin line between rows.
        ui.painter().hline(
            rect.x_range(),
            rect.bottom() + 1.0,
            Stroke::new(1.0, ROW_LINE),
        );
    }
    drag_and_drop(module, ui, data, &rects);
    if let Some(index) = remove {
        edit(&mut module.plans, |plan| {
            plans::remove_entry(plan, data, index)
        });
    }
    line(ui);
    ui.horizontal_wrapped(|ui| {
        ui.label(small("Total").color(DIM));
        ui.label(
            small(duration_text(total.round() as i64))
                .color(BRIGHT)
                .strong(),
        );
        ui.label(
            small(format!(
                "· {} SP{}",
                group_digits(missing_sp),
                if module.plans.character.is_some() {
                    " missing"
                } else {
                    ""
                }
            ))
            .color(DIM),
        );
        if module.plans.character.is_some() {
            ui.label(
                small(format!(
                    "· {trained_count} of {} already trained",
                    plan.entries.len()
                ))
                .color(DIM),
            );
        }
        if module.plans.use_remap && check.has_character {
            ui.label(small("· with remap").color(GREEN));
        }
    });
}

/// True when the entry is there for a later one: a lower level of the
/// same skill, or a prerequisite of a later entry.
fn is_prereq(plan: &Plan, data: &StaticData, index: usize) -> bool {
    let entry = plan.entries[index];
    plan.entries[index + 1..].iter().any(|later| {
        (later.skill_id == entry.skill_id && later.level > entry.level)
            || data.skill(later.skill_id).is_some_and(|s| {
                s.requirements
                    .iter()
                    .any(|&(req, level)| req == entry.skill_id && level >= entry.level)
            })
    })
}

/// While an entry is dragged: a line where it would land, and the move
/// when the mouse button is let go.
fn drag_and_drop(module: &mut Skills, ui: &mut egui::Ui, data: &StaticData, rects: &[Rect]) {
    let Some(from) = module.plans.drag else {
        return;
    };
    let pointer = ui.ctx().pointer_interact_pos();
    // The slot between rows nearest the pointer: 0 = above the first row.
    let slot = pointer.map(|p| {
        rects
            .iter()
            .position(|r| p.y < r.center().y)
            .unwrap_or(rects.len())
    });
    if let (Some(slot), Some(first), Some(last)) = (slot, rects.first(), rects.last()) {
        let y = if slot < rects.len() {
            rects[slot].top() - 1.0
        } else {
            last.bottom() + 1.0
        };
        ui.painter()
            .hline(first.x_range(), y, Stroke::new(2.0, QUEUE_BLUE));
    }
    if ui.input(|i| i.pointer.any_released()) {
        module.plans.drag = None;
        if let Some(slot) = slot {
            // Moving down: the slot counts the row itself too.
            let to = if slot > from { slot - 1 } else { slot };
            if to != from {
                let mut moved = false;
                edit(&mut module.plans, |plan| {
                    moved = plans::move_entry(plan, data, from, to);
                });
                if !moved {
                    let now = crate::auth::now_unix();
                    note(
                        &mut module.plans,
                        now,
                        "Can't move it there: a skill must come after its prerequisites and \
                         lower levels.",
                    );
                }
            }
        }
    } else {
        ui.ctx().request_repaint();
    }
}

// ---------------------------------------------------------------------------
// Right: check against a character, implant sets, attributes, remap.

fn check_panel(module: &mut Skills, ui: &mut egui::Ui, data: &StaticData, check: &Check, now: i64) {
    panel(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Check").size(14.0).color(BRIGHT).strong());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let name = match module.plans.character {
                    Some(id) => module
                        .characters
                        .iter()
                        .find(|c| c.id == id)
                        .map(|c| c.name.clone())
                        .unwrap_or_default(),
                    None => "No character (from scratch)".to_string(),
                };
                let before = module.plans.character;
                egui::ComboBox::from_id_salt("plans-character")
                    .selected_text(name)
                    .width(220.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut module.plans.character,
                            None,
                            "No character (from scratch)",
                        );
                        for c in &module.characters {
                            ui.selectable_value(&mut module.plans.character, Some(c.id), &c.name);
                        }
                    });
                if module.plans.character != before {
                    module.plans.use_remap = false;
                    module.plans.implants = if module.plans.character.is_some() {
                        ImplantChoice::Active
                    } else {
                        ImplantChoice::Preset(0)
                    };
                }
            });
        });
        line(ui);
        implant_sets(module, ui, data, check);
        line(ui);
        attribute_table(module, ui, data, check);
        remap_box(module, ui, data, check, now);
    });
}

fn section(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text.to_uppercase()).size(10.5).color(FAINT));
}

fn implant_sets(module: &mut Skills, ui: &mut egui::Ui, data: &StaticData, check: &Check) {
    let plan = &module.plans.editor.plan;
    let use_remap = module.plans.use_remap;
    let times: Vec<f64> = check
        .rows
        .iter()
        .map(|r| {
            plans::plan_secs(
                data,
                plan,
                &check.standing,
                &check.attributes_with(&r.bonuses, use_remap),
            )
        })
        .collect();
    let fastest = times.iter().copied().fold(f64::INFINITY, f64::min);
    section(ui, "Implant sets");
    let mut picked = None;
    for (index, row) in check.rows.iter().enumerate() {
        if index == 6 {
            let name = module
                .plans
                .character
                .and_then(|id| module.characters.iter().find(|c| c.id == id))
                .map(|c| c.name.clone())
                .unwrap_or_default();
            section(ui, &format!("{name}'s clones"));
        }
        let selected = row.choice == module.plans.implants;
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 30.0), Sense::click());
        let painter = ui.painter_at(rect);
        if selected {
            painter.rect_filled(rect, 3.0, GROUP_PICKED);
        } else if response.hovered() {
            painter.rect_filled(rect, 3.0, CURRENT_FILL);
        }
        let dot = egui::pos2(rect.left() + 9.0, rect.center().y);
        if selected {
            painter.circle_filled(dot, 4.5, QUEUE_BLUE);
        } else {
            painter.circle_stroke(dot, 4.5, Stroke::new(1.0, DIM));
        }
        let secs = times[index];
        let best = (secs - fastest).abs() < 1.0 && !plan.entries.is_empty();
        let time = painter.text(
            rect.right_center() - Vec2::new(6.0, 0.0),
            egui::Align2::RIGHT_CENTER,
            duration_text(secs.round() as i64),
            FontId::proportional(12.0),
            if best { GREEN } else { DIM },
        );
        let width = time.left() - rect.left() - 30.0;
        let title = one_line(&painter, &row.title, 12.5, TEXT, width);
        painter.galley(
            egui::pos2(rect.left() + 20.0, rect.top() + 2.0),
            title,
            TEXT,
        );
        let detail = one_line(&painter, &row.detail, 11.0, FAINT, width);
        painter.galley(
            egui::pos2(rect.left() + 20.0, rect.top() + 16.0),
            detail,
            FAINT,
        );
        let response = response.on_hover_text(&row.detail);
        if response.clicked() {
            picked = Some(row.choice);
        }
    }
    if module.plans.character.is_none() {
        section(ui, "Pick a character to see their clones");
    } else if check.rows.len() == 6 {
        ui.label(
            small("Clones not loaded yet (ESI is asked with the skills).")
                .color(FAINT)
                .italics(),
        );
    }
    if let Some(choice) = picked {
        module.plans.implants = choice;
    }
}

fn attribute_table(module: &Skills, ui: &mut egui::Ui, data: &StaticData, check: &Check) {
    let shares = plans::attribute_shares(data, &module.plans.editor.plan, &check.standing);
    let remap_shown = module.plans.use_remap && check.has_character;
    egui::Grid::new("plans-attributes")
        .num_columns(5)
        .spacing(Vec2::new(14.0, 3.0))
        .show(ui, |ui| {
            for head in ["Attribute", "Base", "Implants", "Total", "Used by plan"] {
                ui.label(RichText::new(head).size(11.0).color(FAINT));
            }
            ui.end_row();
            for i in 0..5 {
                let base = if remap_shown {
                    check.remap.0[i]
                } else {
                    check.base[i]
                };
                ui.label(small(ATTRIBUTE_NAMES[i]));
                ui.label(small(base.to_string()).color(if remap_shown { GREEN } else { TEXT }));
                let bonus = check.bonuses[i];
                ui.label(
                    small(if bonus > 0 {
                        format!("+{bonus}")
                    } else {
                        "–".to_string()
                    })
                    .color(INJECTOR),
                );
                ui.label(small((base + bonus).to_string()).color(BRIGHT).strong());
                ui.horizontal(|ui| {
                    let share = shares[i] as f32;
                    let (rect, _) = ui.allocate_exact_size(Vec2::new(70.0, 10.0), Sense::hover());
                    let bar = Rect::from_min_size(
                        egui::pos2(rect.left(), rect.center().y - 3.0),
                        Vec2::new((70.0 * share).max(1.0), 6.0),
                    );
                    ui.painter()
                        .rect_filled(bar, 3.0, Color32::from_rgb(0x3a, 0x5f, 0x80));
                    ui.label(
                        RichText::new(format!("{:.0}%", share * 100.0))
                            .size(11.0)
                            .color(FAINT),
                    );
                });
                ui.end_row();
            }
        });
    if !check.has_character {
        ui.label(
            small("From scratch: a fresh character's attributes (20 each, 19 Charisma).")
                .color(FAINT),
        );
    }
}

fn remap_box(module: &mut Skills, ui: &mut egui::Ui, data: &StaticData, check: &Check, now: i64) {
    ui.add_space(4.0);
    let Some(id) = module.plans.character else {
        frame_box(ui, CARD_BORDER, |ui| {
            ui.label(small("Pick a character to see the best remap for this plan.").color(DIM));
        });
        return;
    };
    let remap = module.skills.get(&id).and_then(|s| s.remap);
    let available = remap.is_some_and(|r| r.available(now));
    let status = match remap {
        None => "Remap state not loaded yet.".to_string(),
        Some(r) if r.available(now) => {
            let mut parts = Vec::new();
            if r.bonus_remaps > 0 {
                parts.push(format!(
                    "{} bonus remap{}",
                    r.bonus_remaps,
                    if r.bonus_remaps == 1 { "" } else { "s" }
                ));
            }
            if r.cooldown_until.is_none_or(|u| u <= now) {
                parts.push("yearly remap ready".to_string());
            }
            format!("Remap available now: {}", parts.join(", "))
        }
        Some(r) => format!(
            "No remap available. Next yearly remap {}.",
            r.cooldown_until
                .map_or("unknown".to_string(), |u| when_text(u, now))
        ),
    };
    let plan = &module.plans.editor.plan;
    let now_secs = plans::plan_secs(
        data,
        plan,
        &check.standing,
        &check.attributes_with(&check.bonuses, false),
    );
    let (best, best_secs) = check.remap;
    let saves = (now_secs - best_secs).max(0.0);
    let edge = if available {
        Color32::from_rgb(0x2f, 0x4a, 0x2f)
    } else {
        CARD_BORDER
    };
    let mut toggle = false;
    frame_box(ui, edge, |ui| {
        ui.label(small(status).color(TEXT));
        if plan.entries.is_empty() {
            return;
        }
        if saves < 60.0 {
            ui.label(
                small("Your current attributes are already the best for this plan.").color(DIM),
            );
            return;
        }
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.label(small("Best remap for this plan:").color(DIM));
            let text = (0..5)
                .map(|i| format!("{} {}", &ATTRIBUTE_NAMES[i][..3], best[i]))
                .collect::<Vec<_>>()
                .join(" · ");
            ui.label(small(text).color(BRIGHT).strong());
        });
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.label(small(duration_text(now_secs.round() as i64)).color(DIM));
            ui.label(small("to").color(DIM));
            ui.label(
                small(duration_text(best_secs.round() as i64))
                    .color(BRIGHT)
                    .strong(),
            );
            ui.label(
                small(format!("· saves {}", duration_text(saves.round() as i64))).color(GREEN),
            );
        });
        let on = module.plans.use_remap;
        if ui
            .add(egui::Button::selectable(on, small("Show with remap")))
            .on_hover_text("Use the remap for the times, the table and the implant sets")
            .clicked()
        {
            toggle = true;
        }
    });
    if toggle {
        module.plans.use_remap = !module.plans.use_remap;
    }
}

fn frame_box(ui: &mut egui::Ui, edge: Color32, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(Color32::from_rgb(0x1e, 0x22, 0x1e))
        .stroke(Stroke::new(1.0, edge))
        .corner_radius(CornerRadius::same(4))
        .inner_margin(Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 3.0;
            add(ui);
        });
}

// ---------------------------------------------------------------------------
// Popups.

fn popups(module: &mut Skills, ui: &mut egui::Ui, data: &StaticData, now: i64) {
    let Some(popup) = module.plans.popup.take() else {
        return;
    };
    let ctx = ui.ctx().clone();
    let id = egui::Id::new("skills-plans-popup");
    // Each arm puts the popup back unless it's done.
    let keep = match popup {
        Popup::Saved { search, selected } => saved_window(module, &ctx, id, search, selected, now),
        Popup::Rename { target, text } => rename_window(module, &ctx, id, target, text, now),
        Popup::Delete { index } => delete_window(module, &ctx, id, index),
        Popup::Import { text, name } => import_window(module, &ctx, id, data, text, name, now),
        Popup::Unsaved { then } => unsaved_window(module, &ctx, id, then, now),
    };
    if let Some(popup) = keep
        && module.plans.popup.is_none()
    {
        module.plans.popup = Some(popup);
    }
}

fn modal(ctx: &egui::Context, id: egui::Id, title: &str, add: impl FnOnce(&mut egui::Ui)) -> bool {
    let response = egui::Modal::new(id).show(ctx, |ui| {
        ui.set_width(CARD_MAX_WIDTH - 40.0);
        ui.label(RichText::new(title).size(14.0).color(BRIGHT).strong());
        ui.add_space(4.0);
        add(ui);
    });
    response.should_close()
}

fn saved_window(
    module: &mut Skills,
    ctx: &egui::Context,
    id: egui::Id,
    mut search: String,
    mut selected: Option<usize>,
    now: i64,
) -> Option<Popup> {
    #[derive(PartialEq)]
    enum Action {
        None,
        Close,
        Open(usize),
        Rename(usize),
        Duplicate(usize),
        Delete(usize),
    }
    let mut action = Action::None;
    let state = &module.plans;
    let closed = modal(ctx, id, "Saved plans", |ui| {
        ui.label(small("Plans are shared by all characters.").color(DIM));
        ui.add(
            egui::TextEdit::singleline(&mut search)
                .hint_text("Search plans")
                .desired_width(f32::INFINITY),
        );
        let needle = search.trim().to_lowercase();
        egui::ScrollArea::vertical()
            .max_height(340.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                if state.file.plans.is_empty() {
                    ui.label(small("No saved plans yet.").color(FAINT).italics());
                }
                for (index, plan) in state.file.plans.iter().enumerate() {
                    if !needle.is_empty() && !plan.name.to_lowercase().contains(&needle) {
                        continue;
                    }
                    let picked = selected == Some(index);
                    let (rect, response) = ui
                        .allocate_exact_size(Vec2::new(ui.available_width(), 34.0), Sense::click());
                    let painter = ui.painter_at(rect);
                    if picked {
                        painter.rect_filled(rect, 3.0, GROUP_PICKED);
                    } else if response.hovered() {
                        painter.rect_filled(rect, 3.0, CURRENT_FILL);
                    }
                    let open_mark = if state.editor.index == Some(index) {
                        " (open)"
                    } else {
                        ""
                    };
                    let title = one_line(
                        &painter,
                        &format!("{}{open_mark}", plan.name),
                        12.5,
                        BRIGHT,
                        rect.width() - 16.0,
                    );
                    painter.galley(
                        egui::pos2(rect.left() + 8.0, rect.top() + 3.0),
                        title,
                        BRIGHT,
                    );
                    let detail = format!(
                        "{} skill levels · changed {}",
                        plan.entries.len(),
                        when_text(plan.changed, now)
                    );
                    painter.text(
                        egui::pos2(rect.left() + 8.0, rect.top() + 19.0),
                        egui::Align2::LEFT_TOP,
                        detail,
                        FontId::proportional(11.0),
                        FAINT,
                    );
                    if response.clicked() {
                        selected = Some(index);
                    }
                    if response.double_clicked() {
                        action = Action::Open(index);
                    }
                }
            });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let some = selected.filter(|i| *i < state.file.plans.len());
            if ui
                .add_enabled(some.is_some(), egui::Button::new("Rename"))
                .clicked()
            {
                action = some.map_or(Action::None, Action::Rename);
            }
            if ui
                .add_enabled(some.is_some(), egui::Button::new("Duplicate"))
                .clicked()
            {
                action = some.map_or(Action::None, Action::Duplicate);
            }
            if ui
                .add_enabled(
                    some.is_some(),
                    egui::Button::new(RichText::new("Delete").color(RED)),
                )
                .clicked()
            {
                action = some.map_or(Action::None, Action::Delete);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add_enabled(some.is_some(), egui::Button::new("Open").fill(GROUP_PICKED))
                    .clicked()
                {
                    action = some.map_or(Action::None, Action::Open);
                }
                if ui.button("Close").clicked() {
                    action = Action::Close;
                }
            });
        });
    });
    let keep = Popup::Saved { search, selected };
    match action {
        Action::None if closed => None,
        Action::None => Some(keep),
        Action::Close => None,
        Action::Open(index) => {
            switch(module, Pending::Open(index));
            None
        }
        Action::Rename(index) => {
            let text = module.plans.file.plans[index].name.clone();
            module.plans.popup = Some(Popup::Rename {
                target: RenameTarget::Saved(index),
                text,
            });
            None
        }
        Action::Duplicate(index) => {
            let state = &mut module.plans;
            let mut copy = state.file.plans[index].clone();
            copy.name = free_name(&state.file, &format!("{} (copy)", copy.name));
            copy.changed = now;
            state.file.plans.push(copy);
            write_file(module);
            Some(keep)
        }
        Action::Delete(index) => {
            module.plans.popup = Some(Popup::Delete { index });
            None
        }
    }
}

fn rename_window(
    module: &mut Skills,
    ctx: &egui::Context,
    id: egui::Id,
    target: RenameTarget,
    mut text: String,
    now: i64,
) -> Option<Popup> {
    let mut done: Option<bool> = None;
    let closed = modal(ctx, id, "Rename plan", |ui| {
        let edit = ui.add(egui::TextEdit::singleline(&mut text).desired_width(f32::INFINITY));
        if !edit.has_focus() && !edit.lost_focus() {
            edit.request_focus();
        }
        if edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
            done = Some(true);
        }
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add(egui::Button::new("Rename").fill(GROUP_PICKED))
                    .clicked()
                {
                    done = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    done = Some(false);
                }
            });
        });
    });
    let name = text.trim().to_string();
    match done {
        Some(true) if !name.is_empty() => {
            match target {
                RenameTarget::Open => edit(&mut module.plans, |plan| plan.name = name),
                RenameTarget::Saved(index) => {
                    let state = &mut module.plans;
                    if let Some(plan) = state.file.plans.get_mut(index) {
                        plan.name = name.clone();
                        plan.changed = now;
                    }
                    // The open plan keeps its unsaved edits, with the new name.
                    if state.editor.index == Some(index) {
                        state.editor.plan.name = name;
                    }
                    write_file(module);
                }
            }
            None
        }
        Some(_) => None,
        None if closed => None,
        None => Some(Popup::Rename { target, text }),
    }
}

fn delete_window(
    module: &mut Skills,
    ctx: &egui::Context,
    id: egui::Id,
    index: usize,
) -> Option<Popup> {
    let name = module.plans.file.plans.get(index).map(|p| p.name.clone())?;
    let mut done: Option<bool> = None;
    let closed = modal(ctx, id, "Delete plan", |ui| {
        ui.label(small(format!("Delete \"{name}\"? This can't be undone.")));
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add(egui::Button::new(RichText::new("Delete").color(RED)))
                    .clicked()
                {
                    done = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    done = Some(false);
                }
            });
        });
    });
    match done {
        Some(true) => {
            delete_saved(module, index);
            None
        }
        Some(false) => None,
        None if closed => None,
        None => Some(Popup::Delete { index }),
    }
}

fn import_window(
    module: &mut Skills,
    ctx: &egui::Context,
    id: egui::Id,
    data: &StaticData,
    mut text: String,
    mut name: String,
    now: i64,
) -> Option<Popup> {
    let mut done: Option<bool> = None;
    let imported = plans::parse_plan(&text, data);
    let closed = modal(ctx, id, "Import plan", |ui| {
        ui.label(
            small(
                "Paste a plan copied from EVE, or any list with one skill per line \
                 (\"Skill name 4\" or \"Skill name IV\").",
            )
            .color(DIM),
        );
        if crate::modules::mining::clipboard::is_supported()
            && ui.button("Paste clipboard").clicked()
        {
            match crate::modules::mining::clipboard::read_text_once(MAX_IMPORT_BYTES) {
                Some(clip) => text = clip,
                None => {
                    note(
                        &mut module.plans,
                        now,
                        "Couldn't read text from the clipboard.",
                    );
                }
            }
        }
        egui::ScrollArea::vertical()
            .max_height(220.0)
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut text)
                        .font(egui::TextStyle::Monospace)
                        .desired_rows(8)
                        .desired_width(f32::INFINITY)
                        .hint_text("Mining Barge 5\nExhumers IV\n…"),
                );
            });
        let found = imported.skills.len();
        let mut status = format!("{found} skill level(s) found. Missing prerequisites are added.");
        if !imported.skipped.is_empty() {
            status.push_str(&format!(
                " {} line(s) skipped: {}",
                imported.skipped.len(),
                imported
                    .skipped
                    .iter()
                    .take(5)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        ui.label(small(status).color(if imported.skipped.is_empty() {
            DIM
        } else {
            YELLOW
        }));
        ui.horizontal(|ui| {
            ui.label(small("Name").color(DIM));
            ui.add(egui::TextEdit::singleline(&mut name).desired_width(f32::INFINITY));
        });
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add_enabled(found > 0, egui::Button::new("Import").fill(GROUP_PICKED))
                    .clicked()
                {
                    done = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    done = Some(false);
                }
            });
        });
    });
    match done {
        Some(true) => {
            let mut plan = Plan {
                name: String::new(),
                changed: now,
                entries: Vec::new(),
            };
            for (skill, level) in &imported.skills {
                plans::add_with_prereqs(&mut plan, data, *skill, *level, &|_| 0);
            }
            let state = &mut module.plans;
            let wanted = if name.trim().is_empty() {
                "Imported plan"
            } else {
                name.trim()
            };
            plan.name = free_name(&state.file, wanted);
            state.file.plans.push(plan);
            let index = state.file.plans.len() - 1;
            write_file(module);
            switch(module, Pending::Open(index));
            None
        }
        Some(false) => None,
        None if closed => None,
        None => Some(Popup::Import { text, name }),
    }
}

fn unsaved_window(
    module: &mut Skills,
    ctx: &egui::Context,
    id: egui::Id,
    then: Pending,
    now: i64,
) -> Option<Popup> {
    #[derive(PartialEq)]
    enum Choice {
        Save,
        Discard,
        Cancel,
    }
    let mut choice = None;
    let name = module.plans.editor.plan.name.clone();
    let read_only = module.plans.read_only;
    let closed = modal(ctx, id, "Unsaved changes", |ui| {
        ui.label(small(format!("\"{name}\" has changes that aren't saved.")));
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add_enabled(!read_only, egui::Button::new("Save").fill(GROUP_PICKED))
                    .clicked()
                {
                    choice = Some(Choice::Save);
                }
                if ui.button("Discard").clicked() {
                    choice = Some(Choice::Discard);
                }
                if ui.button("Cancel").clicked() {
                    choice = Some(Choice::Cancel);
                }
            });
        });
    });
    match choice {
        Some(Choice::Save) => {
            save_open_plan(module, now);
            do_switch(module, then);
            None
        }
        Some(Choice::Discard) => {
            do_switch(module, then);
            None
        }
        Some(Choice::Cancel) => None,
        None if closed => None,
        None => Some(Popup::Unsaved { then }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_names_count_up() {
        let mut file = PlansFile::default();
        assert_eq!(free_name(&file, "Miner"), "Miner");
        for name in ["Miner", "miner (2)"] {
            file.plans.push(Plan {
                name: name.into(),
                changed: 0,
                entries: Vec::new(),
            });
        }
        assert_eq!(free_name(&file, "Miner"), "Miner (3)");
    }
}
