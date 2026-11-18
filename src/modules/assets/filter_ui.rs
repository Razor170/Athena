//! The window that makes or edits a saved filter (the gear on a chip, or
//! "+ New filter"), as in the mockup.

use eframe::egui::{self, Color32, RichText, Stroke, Vec2};

use super::Assets;
use super::config::Filter;
use super::ui::{ACCENT, ACCENT_FILL, DIM, FAINT, YELLOW, make_room, small};

/// The window is this wide (Athena's 480 px rule).
const WIDTH: f32 = 480.0;
/// At most this many search hits are listed.
const MAX_HITS: usize = 14;
const PILL_FILL: Color32 = Color32::from_rgb(0x1b, 0x25, 0x33);
const PILL_EDGE: Color32 = Color32::from_rgb(0x3c, 0x5a, 0x80);
const PILL_TEXT: Color32 = Color32::from_rgb(0x7f, 0xb4, 0xff);

/// The open editor.
pub struct Editor {
    /// The filter being edited, or `None` for a new one.
    index: Option<usize>,
    filter: Filter,
    /// What is typed in "Find a category or group".
    find: String,
    /// The minimum value in millions of ISK, as typed.
    min_millions: f64,
    /// "Delete" was clicked once; the next click deletes.
    confirm_delete: bool,
}

impl Editor {
    pub fn new_filter() -> Editor {
        Editor {
            index: None,
            filter: Filter::default(),
            find: String::new(),
            min_millions: 0.0,
            confirm_delete: false,
        }
    }

    pub fn edit(index: usize, filter: &Filter) -> Editor {
        Editor {
            index: Some(index),
            filter: filter.clone(),
            find: String::new(),
            min_millions: filter.min_isk / 1e6,
            confirm_delete: false,
        }
    }
}

/// What the window asks for, done after drawing.
enum Outcome {
    Save,
    Delete,
    Close,
}

/// Draws the editor while it is open.
pub fn show(module: &mut Assets, ctx: &egui::Context) {
    let Some(mut editor) = module.editor.take() else {
        return;
    };
    let title = if editor.index.is_some() {
        "Edit filter"
    } else {
        "New filter"
    };
    let mut open = true;
    let mut outcome = None;
    egui::Window::new(title)
        .id(egui::Id::new("assets_filter_editor"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_width(WIDTH)
        .max_width(WIDTH)
        .show(ctx, |ui| {
            ui.set_width(WIDTH);
            outcome = body(module, &mut editor, ui);
        });
    if !open {
        outcome = Some(Outcome::Close);
    }
    match outcome {
        None => module.editor = Some(editor),
        Some(Outcome::Close) => {}
        Some(Outcome::Delete) => {
            if let Some(index) = editor.index {
                module.settings.delete_filter(index);
                module.settings_changed();
            }
        }
        Some(Outcome::Save) => {
            let mut filter = editor.filter;
            filter.name = filter.name.trim().to_string();
            if filter.name.is_empty() {
                filter.name = format!("Filter {}", module.settings.filters.len() + 1);
            }
            filter.min_isk = (editor.min_millions.max(0.0) * 1e6).round();
            match editor.index {
                Some(index) if index < module.settings.filters.len() => {
                    module.settings.filters[index] = filter;
                }
                _ => {
                    module.settings.filters.push(filter);
                    // A new filter is turned on right away.
                    let index = module.settings.filters.len() - 1;
                    module.settings.active.push(index);
                }
            }
            module.settings_changed();
        }
    }
}

fn body(module: &Assets, editor: &mut Editor, ui: &mut egui::Ui) -> Option<Outcome> {
    let mut outcome = None;
    ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
    ui.label(small("Name (shown on the chip)").color(DIM));
    ui.add(
        egui::TextEdit::singleline(&mut editor.filter.name)
            .hint_text("e.g. Caps")
            .desired_width(WIDTH),
    );

    ui.add_space(4.0);
    ui.label(small("Item is in one of these categories or groups (none = any item)").color(DIM));
    // The picked ones, as pills; a click removes one.
    let mut remove_category = None;
    let mut remove_group = None;
    wrapped(ui, |ui| {
        for &id in &editor.filter.categories {
            let name = module.catalog.category_name(id).unwrap_or("?");
            if pill(ui, &format!("{name} (category) ×")).clicked() {
                remove_category = Some(id);
            }
        }
        for &id in &editor.filter.groups {
            let name = module.catalog.group_name(id).unwrap_or("?");
            if pill(ui, &format!("{name} ×")).clicked() {
                remove_group = Some(id);
            }
        }
        if editor.filter.categories.is_empty() && editor.filter.groups.is_empty() {
            ui.label(small("Any item").italics().color(FAINT));
        }
    });
    if let Some(id) = remove_category {
        editor.filter.categories.retain(|c| *c != id);
    }
    if let Some(id) = remove_group {
        editor.filter.groups.retain(|g| *g != id);
    }
    ui.add(
        egui::TextEdit::singleline(&mut editor.find)
            .hint_text("Find a category or group, e.g. Ship, Carrier, Mineral")
            .desired_width(WIDTH),
    );
    let find = editor.find.trim().to_lowercase();
    if !find.is_empty() {
        let mut hits = 0;
        wrapped(ui, |ui| {
            for (id, name) in &module.catalog.categories {
                if hits >= MAX_HITS || !name.to_lowercase().contains(&find) {
                    continue;
                }
                hits += 1;
                let picked = editor.filter.categories.contains(id);
                if ui
                    .add_enabled(
                        !picked,
                        egui::Button::new(small(format!("+ {name} (category)")))
                            .wrap_mode(egui::TextWrapMode::Extend),
                    )
                    .clicked()
                {
                    editor.filter.categories.push(*id);
                }
            }
            // Names that start with the text first: "dread" lists
            // Dreadnought before Lancer Dreadnought.
            let mut groups: Vec<_> = module
                .catalog
                .groups
                .iter()
                .filter(|g| g.name.to_lowercase().contains(&find))
                .collect();
            groups.sort_by_key(|g| !g.name.to_lowercase().starts_with(&find));
            for group in groups {
                if hits >= MAX_HITS {
                    break;
                }
                hits += 1;
                let picked = editor.filter.groups.contains(&group.group_id);
                let category = module
                    .catalog
                    .category_name(group.category_id)
                    .unwrap_or("");
                if ui
                    .add_enabled(
                        !picked,
                        egui::Button::new(small(format!("+ {}", group.name)))
                            .wrap_mode(egui::TextWrapMode::Extend),
                    )
                    .on_hover_text(format!("Group in {category}"))
                    .clicked()
                {
                    editor.filter.groups.push(group.group_id);
                }
            }
            if hits == 0 {
                ui.label(small("Nothing found.").color(FAINT));
            }
        });
        if module.catalog.groups.is_empty() {
            ui.colored_label(YELLOW, small("The groups need static.sqlite."));
        }
    }

    ui.add_space(4.0);
    ui.label(small("Name contains (type or given name, any case)").color(DIM));
    ui.add(
        egui::TextEdit::singleline(&mut editor.filter.text)
            .hint_text("e.g. II, Compressed")
            .desired_width(WIDTH),
    );

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(small("Worth at least").color(DIM));
        ui.add(
            egui::DragValue::new(&mut editor.min_millions)
                .range(0.0..=1_000_000.0)
                .speed(10.0),
        );
        ui.label(small("million ISK (a ship counts with what is in it)").color(DIM));
    });

    ui.add_space(4.0);
    ui.label(small("Where").color(DIM));
    ui.horizontal(|ui| {
        ui.checkbox(&mut editor.filter.high_sec, small("High sec"));
        ui.checkbox(&mut editor.filter.low_sec, small("Low sec"));
        ui.checkbox(&mut editor.filter.null_sec, small("Null sec"));
        ui.checkbox(&mut editor.filter.wormhole, small("Wormhole"));
    });

    ui.add_space(6.0);
    ui.separator();
    ui.horizontal(|ui| {
        let save = egui::Button::new(RichText::new("Save").size(12.0).color(Color32::WHITE))
            .fill(ACCENT_FILL);
        if crate::widgets::add_outlined(ui, true, save, Stroke::new(1.0, ACCENT)).clicked() {
            outcome = Some(Outcome::Save);
        }
        if ui.button(small("Cancel")).clicked() {
            outcome = Some(Outcome::Close);
        }
        if editor.index.is_some() {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let text = if editor.confirm_delete {
                    "Click again to delete"
                } else {
                    "Delete"
                };
                if ui
                    .button(small(text).color(Color32::from_rgb(0xe5, 0x53, 0x4b)))
                    .clicked()
                {
                    if editor.confirm_delete {
                        outcome = Some(Outcome::Delete);
                    } else {
                        editor.confirm_delete = true;
                    }
                }
            });
        }
    });
    outcome
}

/// A row of buttons that wraps at the window's width. (A plain
/// `horizontal_wrapped` let one-line buttons push the window wider.)
fn wrapped(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    let layout = egui::Layout::left_to_right(egui::Align::Min).with_main_wrap(true);
    ui.allocate_ui_with_layout(Vec2::new(WIDTH, 0.0), layout, add);
}

fn pill(ui: &mut egui::Ui, text: &str) -> egui::Response {
    make_room(ui, text, 0.0);
    let button = egui::Button::new(small(text).color(PILL_TEXT))
        .fill(PILL_FILL)
        .corner_radius(10.0)
        // One line, never squeezed into a narrow column.
        .wrap_mode(egui::TextWrapMode::Extend);
    crate::widgets::add_outlined(ui, true, button, Stroke::new(1.0, PILL_EDGE))
        .on_hover_text("Remove")
}
