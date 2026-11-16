//! The two saved fits windows: "Save fit" (asks for a name) and "Saved
//! fits" (search, ship type filter, sort, Load, Rename, Delete). Follows
//! the approved mockup (<https://claude.ai/artifact/8J4t2W7hz7feP5ePEUjxML>).

use eframe::egui::{self, Color32, RichText, vec2};

use super::library::{SortBy, changed_text};
use super::{Fittings, Side};
use crate::auth::now_unix;

const TEXT: Color32 = Color32::from_rgb(0xdc, 0xdc, 0xdc);
const DIM: Color32 = Color32::from_rgb(0x9a, 0x9a, 0x9a);
const FAINT: Color32 = Color32::from_rgb(0x6a, 0x6a, 0x6a);
const YELLOW: Color32 = Color32::from_rgb(0xe0, 0xb0, 0x40);
const RED: Color32 = Color32::from_rgb(0xe5, 0x53, 0x4b);
/// The list is at most this wide (Athena's 480 px rule is per column; the
/// list has four, so it gets a bit more).
const LIST_WIDTH: f32 = 620.0;
const LIST_HEIGHT: f32 = 380.0;
/// Fixed column widths; the name takes the rest.
const HULL_W: f32 = 110.0;
const CHANGED_W: f32 = 120.0;
const ACTIONS_W: f32 = 150.0;

fn small(text: impl Into<String>) -> RichText {
    RichText::new(text).size(12.0)
}

fn column_title(side: Side) -> &'static str {
    match side {
        Side::Is => "Is",
        Side::Should => "Should",
    }
}

/// Draws whichever of the two windows is open.
pub fn show(module: &mut Fittings, ctx: &egui::Context) {
    save_window(module, ctx);
    list_window(module, ctx);
}

/// What a click in the "Save fit" window asks for.
enum SaveClick {
    None,
    Save,
    Cancel,
}

fn save_window(module: &mut Fittings, ctx: &egui::Context) {
    let Some(side) = module.save_dialog.as_ref().map(|d| d.side) else {
        return;
    };
    let hull = module.fit(side).hull.clone();
    let mut click = SaveClick::None;
    let mut open = true;
    egui::Window::new(format!("Save fit from {}", column_title(side)))
        .id(egui::Id::new("fittings_save_fit"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .default_width(380.0)
        .show(ctx, |ui| {
            let library = &module.library;
            let Some(dialog) = module.save_dialog.as_mut() else {
                return;
            };
            ui.label(small("Name").color(DIM));
            let response = ui.add(
                egui::TextEdit::singleline(&mut dialog.name)
                    .desired_width(f32::INFINITY)
                    .hint_text("Name of the fit"),
            );
            if dialog.focus {
                response.request_focus();
                dialog.focus = false;
            }
            let entered = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                click = SaveClick::Cancel;
            }
            ui.label(small(format!("Ship type: {hull}")).color(DIM));
            let name = dialog.name.trim();
            let existing = library.find(name, &hull).map(|i| &library.fits[i]);
            if name.is_empty() {
                ui.label(small("Enter a name.").color(FAINT));
            } else if let Some(fit) = existing {
                ui.label(
                    small(format!(
                        "Replaces the saved fit \"{}\" (changed {}).",
                        fit.name,
                        changed_text(fit.changed, now_unix())
                    ))
                    .color(YELLOW),
                );
            }
            ui.add_space(4.0);
            // A one-row strip: `with_layout` alone would take all the height
            // left and make the window tall.
            let strip = vec2(ui.available_width(), 24.0);
            let layout = egui::Layout::right_to_left(egui::Align::Center);
            ui.allocate_ui_with_layout(strip, layout, |ui| {
                let label = if existing.is_some() {
                    "Replace"
                } else {
                    "Save"
                };
                if ui
                    .add_enabled(!name.is_empty(), egui::Button::new(label))
                    .clicked()
                    || (entered && !name.is_empty())
                {
                    click = SaveClick::Save;
                }
                if ui.button("Cancel").clicked() {
                    click = SaveClick::Cancel;
                }
            });
        });
    if !open {
        click = SaveClick::Cancel;
    }
    match click {
        SaveClick::None => {}
        SaveClick::Cancel => module.save_dialog = None,
        SaveClick::Save => {
            if let Some(dialog) = module.save_dialog.take() {
                module.save_fit(dialog.side, &dialog.name);
            }
        }
    }
}

/// What a click in the "Saved fits" list asks for, done after drawing.
enum ListClick {
    None,
    Load(usize),
    Rename(usize, String),
    Delete(usize),
}

fn list_window(module: &mut Fittings, ctx: &egui::Context) {
    if !module.library_window.open {
        return;
    }
    let side = module.library_window.target.unwrap_or(Side::Is);
    let mut open = true;
    let mut click = ListClick::None;
    egui::Window::new(format!("Saved fits › load into {}", column_title(side)))
        .id(egui::Id::new("fittings_saved_fits"))
        .open(&mut open)
        .collapsible(false)
        // A fixed width: with a resizable one, the name column (which
        // takes what is left) and the window kept growing each other.
        .resizable(false)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .min_width(LIST_WIDTH)
        .max_width(LIST_WIDTH)
        .show(ctx, |ui| click = list_body(module, ui));
    if !open {
        module.library_window.open = false;
    }
    match click {
        ListClick::None => {}
        ListClick::Load(index) => module.load_saved(index),
        ListClick::Rename(index, name) => module.rename_saved(index, &name),
        ListClick::Delete(index) => module.delete_saved(index),
    }
}

fn list_body(module: &mut Fittings, ui: &mut egui::Ui) -> ListClick {
    let mut click = ListClick::None;
    // Ship classes ("Carrier", "Exhumer") for the type filter, from the
    // game data when it is there.
    let class_of = |hull: &str| -> String {
        module
            .static_data
            .as_ref()
            .and_then(|d| d.universe.ship_by_name(hull).map(|s| s.group_name.clone()))
            .unwrap_or_else(|| "Other".to_string())
    };
    let mut hulls: Vec<(String, String)> = module
        .library
        .hulls()
        .into_iter()
        .map(|h| (class_of(&h), h))
        .collect();
    hulls.sort();

    let window = &mut module.library_window;
    // A filter for a ship type that no longer has fits shows everything.
    if window
        .hull
        .as_ref()
        .is_some_and(|h| !hulls.iter().any(|(_, x)| x == h))
    {
        window.hull = None;
    }
    let listed = module
        .library
        .listed(&window.search, window.hull.as_deref(), window.sort);

    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut window.search)
                .hint_text("Search name")
                .desired_width(200.0),
        );
        let selected = window.hull.clone().unwrap_or("All ship types".to_string());
        egui::ComboBox::from_id_salt("fittings_saved_hull")
            .selected_text(selected)
            .height(400.0)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut window.hull, None, "All ship types");
                let mut last_class = "";
                for (class, hull) in &hulls {
                    if class != last_class {
                        ui.label(small(class.to_uppercase()).size(10.5).color(FAINT));
                        last_class = class;
                    }
                    ui.selectable_value(&mut window.hull, Some(hull.clone()), hull);
                }
            });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                small(format!(
                    "{} of {} fits",
                    listed.len(),
                    module.library.fits.len()
                ))
                .color(DIM),
            );
        });
    });
    ui.separator();

    if module.library.fits.is_empty() {
        ui.label(
            small("No saved fits yet. Use Save… on a column to keep a fit here.")
                .italics()
                .color(FAINT),
        );
        return click;
    }

    // Row gaps: three between four cells, plus room for the scroll bar.
    let name_w = LIST_WIDTH - HULL_W - CHANGED_W - ACTIONS_W - 3.0 * 6.0 - 12.0;
    // Header: click a title to sort by it.
    ui.horizontal(|ui| {
        header(ui, &mut window.sort, SortBy::Name, "NAME", name_w);
        header(ui, &mut window.sort, SortBy::Hull, "SHIP TYPE", HULL_W);
        header(
            ui,
            &mut window.sort,
            SortBy::Changed,
            "LAST CHANGED",
            CHANGED_W,
        );
    });

    let now = now_unix();
    egui::ScrollArea::vertical()
        .id_salt("fittings_saved_list")
        .max_height(LIST_HEIGHT)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            if listed.is_empty() {
                ui.label(small("No saved fits match.").italics().color(FAINT));
            }
            for index in listed {
                let fit = &module.library.fits[index];
                ui.horizontal(|ui| {
                    // Name, or the rename field.
                    cell(ui, name_w, |ui| match &mut window.renaming {
                        Some((i, name)) if *i == index => {
                            let response = ui
                                .add(egui::TextEdit::singleline(name).desired_width(name_w - 8.0));
                            // The field is new this frame when nothing has
                            // focus yet: put the cursor in it.
                            if ui.memory(|m| m.focused().is_none()) {
                                response.request_focus();
                            }
                            if response.lost_focus()
                                && ui.input(|i| i.key_pressed(egui::Key::Enter))
                            {
                                click = ListClick::Rename(index, name.clone());
                            }
                        }
                        _ => {
                            ui.add(
                                egui::Label::new(RichText::new(&fit.name).size(12.5).color(TEXT))
                                    .truncate(),
                            );
                        }
                    });
                    cell(ui, HULL_W, |ui| {
                        ui.add(egui::Label::new(small(&fit.hull).color(DIM)).truncate());
                    });
                    cell(ui, CHANGED_W, |ui| {
                        ui.label(small(changed_text(fit.changed, now)).color(DIM));
                    });
                    cell(ui, ACTIONS_W, |ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            actions(ui, window, index, &fit.name, &mut click);
                        });
                    });
                });
            }
        });
    if let Some(problem) = &window.problem {
        ui.label(small(problem).color(YELLOW));
    }
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        window.renaming = None;
        window.confirm_delete = None;
        window.problem = None;
    }
    click
}

/// Load, Rename (Save/Cancel while renaming) and Delete (asks once).
/// Right to left: Load is at the right edge.
fn actions(
    ui: &mut egui::Ui,
    window: &mut super::LibraryWindow,
    index: usize,
    fit_name: &str,
    click: &mut ListClick,
) {
    if let Some((i, name)) = &window.renaming
        && *i == index
    {
        if ui.button(small("Save")).clicked() {
            *click = ListClick::Rename(index, name.clone());
        }
        if ui.button(small("Cancel")).clicked() {
            window.renaming = None;
            window.problem = None;
        }
        return;
    }
    if window.confirm_delete == Some(index) {
        if ui.button(small("Delete?").color(RED)).clicked() {
            *click = ListClick::Delete(index);
        }
        if ui.button(small("Keep")).clicked() {
            window.confirm_delete = None;
        }
        return;
    }
    if ui.button(small("Load")).clicked() {
        *click = ListClick::Load(index);
    }
    if ui
        .add(egui::Label::new(small("Delete").color(FAINT)).sense(egui::Sense::click()))
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
    {
        window.confirm_delete = Some(index);
        window.renaming = None;
    }
    if ui
        .add(egui::Label::new(small("Rename").color(FAINT)).sense(egui::Sense::click()))
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
    {
        window.renaming = Some((index, fit_name.to_string()));
        window.confirm_delete = None;
        window.problem = None;
    }
}

/// A fixed-width cell in a list row.
fn cell(ui: &mut egui::Ui, width: f32, add_contents: impl FnOnce(&mut egui::Ui)) {
    ui.allocate_ui_with_layout(
        vec2(width, 20.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_width(width);
            add_contents(ui);
        },
    );
}

/// A column title; the sorted one is brighter and has a small triangle
/// (down: newest first, up: A to Z).
fn header(ui: &mut egui::Ui, sort: &mut SortBy, by: SortBy, title: &str, width: f32) {
    let on = *sort == by;
    cell(ui, width, |ui| {
        let text = small(title).size(10.5).color(if on { TEXT } else { FAINT });
        let response = ui
            .add(egui::Label::new(text).sense(egui::Sense::click()))
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if on {
            let center = response.rect.right_center() + vec2(8.0, 0.0);
            sort_triangle(ui, center, by == SortBy::Changed);
        }
        if response.clicked() {
            *sort = by;
        }
    });
}

/// Painted, because the built-in fonts have no arrows (as in Contracts).
fn sort_triangle(ui: &egui::Ui, center: egui::Pos2, down: bool) {
    let (w, h) = (4.0, 3.0);
    let points = if down {
        vec![
            center + vec2(-w, -h),
            center + vec2(w, -h),
            center + vec2(0.0, h),
        ]
    } else {
        vec![
            center + vec2(-w, h),
            center + vec2(w, h),
            center + vec2(0.0, -h),
        ]
    };
    ui.painter().add(egui::Shape::convex_polygon(
        points,
        TEXT,
        egui::Stroke::NONE,
    ));
}
