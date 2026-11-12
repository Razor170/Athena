//! Draws the Characters panel, following the mockup Kevin approved
//! (<https://claude.ai/artifact/CV7E765hJwHYpyTYHNNn9s>): a toolbar, the
//! "Unassigned" box for new logins, then one card per account with three
//! slots. Characters are dragged between them by their handle.
//!
//! Drawing never changes the module directly: clicks are collected as
//! [`Action`]s and applied after the frame is drawn. That keeps the borrow
//! checker happy (the list is read while drawing and changed afterwards).

use eframe::egui::{self, Align, Color32, CornerRadius, Layout, RichText, Sense, Stroke, Vec2};

use super::Characters;
use super::store::{Character, Place, SLOTS_PER_ACCOUNT};
use super::worker::TokenStatus;
use crate::auth::scopes::ALL_SCOPES;

const BRIGHT: Color32 = Color32::from_rgb(0xf0, 0xf0, 0xf0);
const DIM: Color32 = Color32::from_rgb(0x9a, 0x9a, 0x9a);
const CARD_FILL: Color32 = Color32::from_rgb(0x23, 0x23, 0x23);
const ROW_FILL: Color32 = Color32::from_rgb(0x2a, 0x2a, 0x2a);
const BORDER: Color32 = Color32::from_rgb(0x35, 0x35, 0x35);
const UNASSIGNED_FILL: Color32 = Color32::from_rgb(0x24, 0x24, 0x1c);
const UNASSIGNED_BORDER: Color32 = Color32::from_rgb(0x6a, 0x6a, 0x3a);
const UNASSIGNED_TITLE: Color32 = Color32::from_rgb(0xe8, 0xe2, 0xa8);
const GREEN: Color32 = Color32::from_rgb(0x5c, 0xb8, 0x5c);
const GREEN_TEXT: Color32 = Color32::from_rgb(0x9f, 0xd4, 0x9f);
const YELLOW: Color32 = Color32::from_rgb(0xe0, 0xb0, 0x40);
const YELLOW_TEXT: Color32 = Color32::from_rgb(0xf0, 0xd0, 0x90);
const RED: Color32 = Color32::from_rgb(0xd9, 0x53, 0x4f);
const RED_TEXT: Color32 = Color32::from_rgb(0xf0, 0xa0, 0x9c);
const BLUE: Color32 = Color32::from_rgb(0x7f, 0xb4, 0xff);

/// Account cards are at most this wide (like the Mining cards).
const MAX_CARD_WIDTH: f32 = 470.0;
const MIN_CARD_WIDTH: f32 = 300.0;
const GAP: f32 = 12.0;
/// Room a character row keeps on its right for the status, "Log in
/// again" and the menu button.
const RIGHT_PART_WIDTH: f32 = 220.0;

fn small(text: impl Into<String>) -> RichText {
    RichText::new(text).size(11.0)
}

/// Something the user did this frame.
enum Action {
    StartLogin,
    CancelLogin,
    OpenLoginPageAgain,
    AddAccount,
    SetColumns(u8),
    SetFilter(Option<String>),
    Move {
        id: u64,
        to: Place,
        index: usize,
    },
    Remove(u64),
    DeleteAccount(usize),
    ShiftAccount(usize, isize),
    StartRename(usize),
    Rename(usize, String),
    StopRename,
    /// The account name being typed.
    RenameText(String),
    /// The tag being typed in the "Tags…" window.
    TagText(String),
    OpenTags(u64),
    CloseTags,
    AddTag(u64, String),
    RemoveTag(u64, String),
    DismissNotice,
    DismissProblems,
}

pub fn show(module: &mut Characters, ui: &mut egui::Ui) {
    let mut actions = Vec::new();
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
            draw(module, ui, &mut actions);
        });
    tag_window(module, ui.ctx(), &mut actions);
    for action in actions {
        apply(module, action);
    }
}

fn apply(module: &mut Characters, action: Action) {
    match action {
        Action::StartLogin => module.start_login(),
        Action::CancelLogin => module.cancel_login(),
        Action::OpenLoginPageAgain => {
            if let Err(e) = crate::shell::open_local_url(&crate::auth::callback::start_url()) {
                module.problems.push(e);
            }
        }
        Action::AddAccount => {
            let index = module.list.add_account();
            let name = module.list.accounts[index].name.clone();
            module.renaming = Some((index, name));
            module.save();
        }
        Action::SetColumns(n) => {
            module.list.columns = n;
            module.save();
        }
        Action::SetFilter(tag) => module.tag_filter = tag,
        Action::Move { id, to, index } => module.move_character(id, to, index),
        Action::Remove(id) => module.remove_character(id),
        Action::DeleteAccount(index) => {
            module.list.delete_account(index);
            module.renaming = None;
            module.save();
        }
        Action::ShiftAccount(index, by) => {
            module.list.shift_account(index, by);
            module.renaming = None;
            module.save();
        }
        Action::StartRename(index) => {
            if let Some(account) = module.list.accounts.get(index) {
                module.renaming = Some((index, account.name.clone()));
            }
        }
        Action::Rename(index, name) => {
            let name = name.trim();
            if let Some(account) = module.list.accounts.get_mut(index)
                && !name.is_empty()
            {
                account.name = name.to_string();
                module.save();
            }
            module.renaming = None;
        }
        Action::StopRename => module.renaming = None,
        Action::RenameText(new_text) => {
            if let Some((_, text)) = &mut module.renaming {
                *text = new_text;
            }
        }
        Action::TagText(new_text) => {
            if let Some((_, text)) = &mut module.tag_editor {
                *text = new_text;
            }
        }
        Action::OpenTags(id) => module.tag_editor = Some((id, String::new())),
        Action::CloseTags => module.tag_editor = None,
        Action::AddTag(id, tag) => {
            if module.list.add_tag(id, &tag) {
                module.save();
            }
            if let Some((_, text)) = &mut module.tag_editor {
                text.clear();
            }
        }
        Action::RemoveTag(id, tag) => {
            if module.list.remove_tag(id, &tag) {
                module.save();
            }
            // A filter on a tag nobody has any more would show nothing.
            if let Some(filter) = &module.tag_filter
                && !module.list.all_tags().contains(filter)
            {
                module.tag_filter = None;
            }
        }
        Action::DismissNotice => module.notice = None,
        Action::DismissProblems => module.problems.clear(),
    }
}

fn draw(module: &Characters, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    if !crate::auth::is_configured() {
        ui.colored_label(YELLOW_TEXT, "No ESI Client ID is set, so logins are off.");
    }
    if !module.problems.is_empty() {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                for problem in &module.problems {
                    ui.colored_label(YELLOW_TEXT, small(problem));
                }
            });
            if ui.small_button("✕").on_hover_text("Hide").clicked() {
                actions.push(Action::DismissProblems);
            }
        });
    }
    toolbar(module, ui, actions);
    login_strip(module, ui, actions);
    ui.separator();

    let filter = module.tag_filter.as_deref();
    let shown = |c: &Character| filter.is_none_or(|tag| c.tags.iter().any(|t| t == tag));

    // Unassigned: shown while it has characters, and while one is dragged
    // (so a character can be dragged out of an account back into it).
    let unassigned: Vec<&Character> = module
        .list
        .unassigned()
        .into_iter()
        .filter(|c| shown(c))
        .collect();
    let dragging = egui::DragAndDrop::has_payload_of_type::<u64>(ui.ctx());
    let width = ui.available_width().min(MAX_CARD_WIDTH);
    if !unassigned.is_empty() || dragging {
        unassigned_box(module, ui, &unassigned, width, actions);
    }
    if module.list.characters.is_empty() {
        ui.label(
            RichText::new("No characters yet. Click \"+ Add character\" to log one in.").color(DIM),
        );
    }

    // The account cards, row by row.
    let (columns, card_width) = fit_columns(module.list.columns, ui.available_width());
    let visible: Vec<usize> = (0..module.list.accounts.len())
        .filter(|i| {
            filter.is_none()
                || module.list.accounts[*i]
                    .characters
                    .iter()
                    .filter_map(|id| module.list.character(*id))
                    .any(&shown)
        })
        .collect();
    for row in visible.chunks(columns) {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = GAP;
            for index in row {
                account_card(module, ui, *index, card_width, &shown, actions);
            }
        });
        ui.add_space(GAP - 6.0);
    }
}

/// How many cards fit next to each other, and how wide each one is.
fn fit_columns(setting: u8, available: f32) -> (usize, f32) {
    let mut count = setting.clamp(1, 4) as usize;
    loop {
        let n = count as f32;
        let width = ((available - (n - 1.0) * GAP) / n).min(MAX_CARD_WIDTH);
        if count == 1 || width >= MIN_CARD_WIDTH {
            return (count, width.max(0.0));
        }
        count -= 1;
    }
}

fn toolbar(module: &Characters, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    ui.horizontal_wrapped(|ui| {
        let can_log_in = module.login.is_none() && crate::auth::is_configured();
        if ui
            .add_enabled(can_log_in, egui::Button::new("+ Add character"))
            .on_hover_text("Log a character in with EVE's login page in your browser. Athena asks for every ESI scope.")
            .clicked()
        {
            actions.push(Action::StartLogin);
        }
        if ui.button("+ New account").clicked() {
            actions.push(Action::AddAccount);
        }
        ui.add_space(8.0);
        let count = module.list.characters.len();
        let problems = module
            .list
            .characters
            .iter()
            .filter(|c| needs_attention(module, c))
            .count();
        let mut summary = format!(
            "{count} character{} · {} account{}",
            if count == 1 { "" } else { "s" },
            module.list.accounts.len(),
            if module.list.accounts.len() == 1 { "" } else { "s" },
        );
        if problems > 0 {
            summary.push_str(&format!(" · {problems} need a new login"));
        }
        ui.label(RichText::new(summary).color(DIM));

        let tags = module.list.all_tags();
        if !tags.is_empty() {
            ui.add_space(8.0);
            ui.label(RichText::new("Show tag").color(DIM));
            if ui
                .selectable_label(module.tag_filter.is_none(), "All")
                .clicked()
            {
                actions.push(Action::SetFilter(None));
            }
            for tag in tags {
                let on = module.tag_filter.as_deref() == Some(tag.as_str());
                if tag_chip(ui, &tag, on, Sense::click()).clicked() {
                    actions.push(Action::SetFilter(if on { None } else { Some(tag) }));
                }
            }
        }

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            for n in (1..=4u8).rev() {
                if ui
                    .selectable_label(module.list.columns == n, n.to_string())
                    .clicked()
                {
                    actions.push(Action::SetColumns(n));
                }
            }
            ui.label(RichText::new("Columns").color(DIM));
        });
    });
}

/// The strip under the toolbar: a login in progress, or how the last one went.
fn login_strip(module: &Characters, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    if module.login.is_some() {
        egui::Frame::new()
            .fill(Color32::from_rgb(0x23, 0x26, 0x2c))
            .stroke(Stroke::new(1.0, Color32::from_rgb(0x3c, 0x4a, 0x5e)))
            .corner_radius(CornerRadius::same(4))
            .inner_margin(egui::Margin::same(8))
            .show(ui, |ui| {
                // No spinner: it would redraw every frame while waiting.
                ui.label(RichText::new("Waiting for the login in your browser…").color(BRIGHT));
                ui.label(
                    small(format!(
                        "Asks for all {} ESI scopes, so new features never need a new login. Stops after 5 minutes.",
                        ALL_SCOPES.len()
                    ))
                    .color(DIM),
                );
                ui.horizontal(|ui| {
                    if ui.small_button("Open the page again").clicked() {
                        actions.push(Action::OpenLoginPageAgain);
                    }
                    if ui.small_button("Cancel").clicked() {
                        actions.push(Action::CancelLogin);
                    }
                });
            });
    } else if let Some(notice) = &module.notice {
        let (fill, stroke, text) = if notice.is_error {
            (
                Color32::from_rgb(0x2b, 0x1f, 0x1f),
                Color32::from_rgb(0x7a, 0x3a, 0x38),
                Color32::from_rgb(0xf0, 0xb0, 0xac),
            )
        } else {
            (
                Color32::from_rgb(0x1f, 0x2a, 0x1f),
                Color32::from_rgb(0x3d, 0x6a, 0x3d),
                Color32::from_rgb(0xbf, 0xe6, 0xbf),
            )
        };
        egui::Frame::new()
            .fill(fill)
            .stroke(Stroke::new(1.0, stroke))
            .corner_radius(CornerRadius::same(4))
            .inner_margin(egui::Margin::same(8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&notice.text).color(text));
                    if notice.is_error && ui.small_button("Try again").clicked() {
                        actions.push(Action::StartLogin);
                    }
                    if ui.small_button("✕").clicked() {
                        actions.push(Action::DismissNotice);
                    }
                });
            });
    }
}

fn unassigned_box(
    module: &Characters,
    ui: &mut egui::Ui,
    characters: &[&Character],
    width: f32,
    actions: &mut Vec<Action>,
) {
    let frame = egui::Frame::new()
        .fill(UNASSIGNED_FILL)
        .stroke(Stroke::new(1.0, UNASSIGNED_BORDER))
        .corner_radius(CornerRadius::same(4))
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.set_width(width - 16.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Unassigned").color(UNASSIGNED_TITLE).strong());
                ui.label(
                    small("new logins land here, drag them onto an account")
                        .color(Color32::from_rgb(0xa8, 0xa2, 0x7a)),
                );
            });
            let mut dropped = false;
            for (index, character) in characters.iter().enumerate() {
                if character_row(module, ui, character, Place::Unassigned, index, actions) {
                    dropped = true;
                }
            }
            dropped
        });
    if !frame.inner
        && let Some(id) = frame.response.dnd_release_payload::<u64>()
    {
        actions.push(Action::Move {
            id: *id,
            to: Place::Unassigned,
            index: usize::MAX,
        });
    }
    drop_highlight(ui, &frame.response);
}

fn account_card(
    module: &Characters,
    ui: &mut egui::Ui,
    index: usize,
    width: f32,
    shown: &dyn Fn(&Character) -> bool,
    actions: &mut Vec<Action>,
) {
    let account = &module.list.accounts[index];
    let filtering = module.tag_filter.is_some();
    let frame = egui::Frame::new()
        .fill(CARD_FILL)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(4))
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.set_width(width - 16.0);
            ui.vertical(|ui| {
                account_header(module, ui, index, actions);
                let mut dropped = false;
                let characters: Vec<&Character> = account
                    .characters
                    .iter()
                    .filter_map(|id| module.list.character(*id))
                    .collect();
                for (slot, character) in characters.iter().enumerate() {
                    if shown(character)
                        && character_row(
                            module,
                            ui,
                            character,
                            Place::Account(index),
                            slot,
                            actions,
                        )
                    {
                        dropped = true;
                    }
                }
                if !filtering {
                    for _ in characters.len()..SLOTS_PER_ACCOUNT {
                        empty_slot(ui);
                    }
                }
                dropped
            })
            .inner
        });
    // Dropped on the card but not on a row: put it at the end.
    if !frame.inner
        && let Some(id) = frame.response.dnd_release_payload::<u64>()
    {
        actions.push(Action::Move {
            id: *id,
            to: Place::Account(index),
            index: usize::MAX,
        });
    }
    drop_highlight(ui, &frame.response);
}

fn account_header(module: &Characters, ui: &mut egui::Ui, index: usize, actions: &mut Vec<Action>) {
    let account = &module.list.accounts[index];
    ui.horizontal(|ui| {
        match &module.renaming {
            Some((renaming, text)) if *renaming == index => {
                let mut text = text.clone();
                let edit = ui.add(egui::TextEdit::singleline(&mut text).desired_width(180.0));
                if edit.changed() {
                    // Keep what's typed; it's applied on Enter or focus loss.
                    actions.push(Action::RenameText(text.clone()));
                }
                if edit.lost_focus() {
                    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        actions.push(Action::StopRename);
                    } else {
                        actions.push(Action::Rename(index, text));
                    }
                } else if !edit.has_focus() {
                    edit.request_focus();
                }
            }
            _ => {
                let name = ui
                    .add(
                        egui::Label::new(RichText::new(&account.name).color(BRIGHT).strong())
                            .sense(Sense::click()),
                    )
                    .on_hover_text("Double-click to rename");
                if name.double_clicked() {
                    actions.push(Action::StartRename(index));
                }
            }
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.menu_button("☰", |ui| {
                if ui.button("Rename").clicked() {
                    actions.push(Action::StartRename(index));
                    ui.close();
                }
                if ui
                    .add_enabled(index > 0, egui::Button::new("Move up"))
                    .clicked()
                {
                    actions.push(Action::ShiftAccount(index, -1));
                    ui.close();
                }
                if ui
                    .add_enabled(
                        index + 1 < module.list.accounts.len(),
                        egui::Button::new("Move down"),
                    )
                    .clicked()
                {
                    actions.push(Action::ShiftAccount(index, 1));
                    ui.close();
                }
                ui.separator();
                if ui
                    .button("Delete account")
                    .on_hover_text("Its characters go back to Unassigned.")
                    .clicked()
                {
                    actions.push(Action::DeleteAccount(index));
                    ui.close();
                }
            });
            ui.label(small(format!("{}/{SLOTS_PER_ACCOUNT}", account.characters.len())).color(DIM));
        });
    });
}

fn empty_slot(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 30.0), Sense::hover());
    let corners = [
        rect.left_top(),
        rect.right_top(),
        rect.right_bottom(),
        rect.left_bottom(),
        rect.left_top(),
    ];
    ui.painter().extend(egui::Shape::dashed_line(
        &corners,
        Stroke::new(1.0, Color32::from_rgb(0x3c, 0x3c, 0x3c)),
        5.0,
        4.0,
    ));
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "empty slot",
        egui::FontId::proportional(11.0),
        Color32::from_rgb(0x7d, 0x7d, 0x7d),
    );
}

/// One character. Returns true if something was dropped on it (the drop
/// then goes before or after this row).
fn character_row(
    module: &Characters,
    ui: &mut egui::Ui,
    character: &Character,
    place: Place,
    index: usize,
    actions: &mut Vec<Action>,
) -> bool {
    let drag_id = egui::Id::new(("character_drag", character.id));
    let being_dragged = ui.ctx().is_being_dragged(drag_id);
    let status = module
        .statuses
        .get(&character.id)
        .cloned()
        .unwrap_or(TokenStatus::Checking);
    let expired = status == TokenStatus::LoginExpired;
    let border = if expired {
        Color32::from_rgb(0x7a, 0x3a, 0x38)
    } else {
        ROW_FILL
    };
    let row = egui::Frame::new()
        .fill(ROW_FILL)
        .stroke(Stroke::new(1.0, border))
        .corner_radius(CornerRadius::same(3))
        .inner_margin(egui::Margin::symmetric(6, 4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if being_dragged {
                // Faded while it's being moved.
                ui.multiply_opacity(0.4);
            }
            ui.horizontal(|ui| {
                let handle = drag_handle(ui, drag_id);
                // The payload is the dragged character's id.
                handle.dnd_set_drag_payload(character.id);
                // Leave room on the right for the status, "Log in again"
                // and the menu; long names are cut off with "…".
                let info_width = (ui.available_width() - RIGHT_PART_WIDTH).max(80.0);
                let info = ui.allocate_ui_with_layout(
                    Vec2::new(info_width, 0.0),
                    Layout::top_down(Align::Min),
                    |ui| {
                        ui.set_width(info_width);
                        ui.spacing_mut().item_spacing.y = 1.0;
                        ui.add(
                            egui::Label::new(RichText::new(&character.name).color(BRIGHT))
                                .truncate(),
                        );
                        if let Some(corp) = character.corporation_label() {
                            ui.add(egui::Label::new(small(corp).color(DIM)).truncate());
                        }
                        if let Some(alliance) = character.alliance_label() {
                            ui.add(egui::Label::new(small(alliance).color(DIM)).truncate());
                        }
                        if !character.tags.is_empty() {
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                for tag in &character.tags {
                                    tag_chip(ui, tag, false, Sense::hover());
                                }
                            });
                        }
                    },
                );
                info.response.on_hover_ui(|ui| hover_details(ui, character));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    character_menu(module, ui, character, place, actions);
                    if (expired || character.missing_scopes() > 0)
                        && ui
                            .add_enabled(
                                module.login.is_none(),
                                egui::Button::new(small("Log in again")),
                            )
                            .on_hover_text("Opens EVE's login page. Pick this character there.")
                            .clicked()
                    {
                        actions.push(Action::StartLogin);
                    }
                    status_label(ui, &status, character);
                });
            });
        });

    let rect = row.response.rect;
    let pointer_y = ui.ctx().pointer_interact_pos().map(|p| p.y);
    let after = pointer_y.is_some_and(|y| y > rect.center().y);
    if let Some(dragged) = row.response.dnd_hover_payload::<u64>()
        && *dragged != character.id
    {
        // Show where the row will land.
        let y = if after {
            rect.bottom() + 2.0
        } else {
            rect.top() - 2.0
        };
        ui.painter()
            .hline(rect.x_range(), y, Stroke::new(2.0, BLUE));
    }
    if let Some(dragged) = row.response.dnd_release_payload::<u64>() {
        if *dragged != character.id {
            actions.push(Action::Move {
                id: *dragged,
                to: place,
                index: drop_index(module, *dragged, place, index, after),
            });
        }
        return true;
    }
    false
}

/// Where a character dropped before or after the row at `index` ends up,
/// counted without the dragged character (which leaves its old spot first).
fn drop_index(module: &Characters, dragged: u64, place: Place, index: usize, after: bool) -> usize {
    let target = if after { index + 1 } else { index };
    // Moving down within the same list: the rows above shift up by one.
    let same_list_position = match place {
        Place::Account(a) => module.list.accounts[a]
            .characters
            .iter()
            .position(|id| *id == dragged),
        Place::Unassigned => module
            .list
            .unassigned()
            .iter()
            .position(|c| c.id == dragged),
    };
    match same_list_position {
        Some(from) if from < target => target - 1,
        _ => target,
    }
}

fn status_label(ui: &mut egui::Ui, status: &TokenStatus, character: &Character) {
    let missing = character.missing_scopes();
    let (dot, color, text, hover) = match status {
        TokenStatus::Checking => (DIM, DIM, "Checking…".to_string(), None),
        TokenStatus::LoginExpired => (
            RED,
            RED_TEXT,
            "Login expired".to_string(),
            Some("EVE refused the saved login (password change, revoked access, character moved). Log in again.".to_string()),
        ),
        TokenStatus::Error(why) => (
            YELLOW,
            YELLOW_TEXT,
            "Check failed".to_string(),
            Some(format!("{why}\nAthena tries again on the next start.")),
        ),
        TokenStatus::Ok if missing > 0 => (
            YELLOW,
            YELLOW_TEXT,
            "New scopes".to_string(),
            Some(format!(
                "{missing} scope{} added since this login. It still works; log in again to add them.",
                if missing == 1 { "" } else { "s" }
            )),
        ),
        TokenStatus::Ok => (GREEN, GREEN_TEXT, "OK".to_string(), None),
    };
    let response = ui.label(small(text).color(color));
    let (rect, _) = ui.allocate_exact_size(Vec2::new(8.0, 8.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, dot);
    if let Some(hover) = hover {
        response.on_hover_text(hover);
    }
}

fn character_menu(
    module: &Characters,
    ui: &mut egui::Ui,
    character: &Character,
    place: Place,
    actions: &mut Vec<Action>,
) {
    ui.menu_button("☰", |ui| {
        if ui.button("Tags…").clicked() {
            actions.push(Action::OpenTags(character.id));
            ui.close();
        }
        ui.menu_button("Move to account", |ui| {
            if place != Place::Unassigned && ui.button("Unassigned").clicked() {
                actions.push(Action::Move {
                    id: character.id,
                    to: Place::Unassigned,
                    index: usize::MAX,
                });
                ui.close();
            }
            for (index, account) in module.list.accounts.iter().enumerate() {
                if place == Place::Account(index) {
                    continue;
                }
                let full = account.characters.len() >= SLOTS_PER_ACCOUNT;
                let label = if full {
                    format!("{} (full)", account.name)
                } else {
                    account.name.clone()
                };
                if ui.add_enabled(!full, egui::Button::new(label)).clicked() {
                    actions.push(Action::Move {
                        id: character.id,
                        to: Place::Account(index),
                        index: usize::MAX,
                    });
                    ui.close();
                }
            }
        });
        if ui
            .add_enabled(module.login.is_none(), egui::Button::new("Log in again"))
            .clicked()
        {
            actions.push(Action::StartLogin);
            ui.close();
        }
        ui.separator();
        ui.menu_button("Remove", |ui| {
            if ui
                .button(format!(
                    "Yes, remove {} and delete its login",
                    character.name
                ))
                .clicked()
            {
                actions.push(Action::Remove(character.id));
                ui.close();
            }
        });
    });
}

fn hover_details(ui: &mut egui::Ui, character: &Character) {
    ui.label(format!(
        "{} · character id {}",
        character.name, character.id
    ));
    if let Some(corp) = character.corporation_label() {
        ui.label(format!("Corp: {corp}"));
    }
    if let Some(alliance) = character.alliance_label() {
        ui.label(format!("Alliance: {alliance}"));
    }
    let granted = ALL_SCOPES.len() - character.missing_scopes();
    ui.label(format!("Scopes: {granted} of {}", ALL_SCOPES.len()));
}

/// The ⠿ handle a row is dragged by.
fn drag_handle(ui: &mut egui::Ui, id: egui::Id) -> egui::Response {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(14.0, 18.0), Sense::hover());
    let response = ui
        .interact(rect, id, Sense::drag())
        .on_hover_cursor(egui::CursorIcon::Grab)
        .on_hover_text("Drag onto an account");
    let color = if response.hovered() || response.dragged() {
        BRIGHT
    } else {
        DIM
    };
    for dy in [-4.0, 0.0, 4.0] {
        let y = rect.center().y + dy;
        ui.painter().hline(
            (rect.left() + 2.0)..=(rect.right() - 2.0),
            y,
            Stroke::new(1.5, color),
        );
    }
    response
}

/// A blue frame around a card while a character is dragged over it.
fn drop_highlight(ui: &egui::Ui, response: &egui::Response) {
    if response.dnd_hover_payload::<u64>().is_some() {
        ui.painter().rect_stroke(
            response.rect,
            CornerRadius::same(4),
            Stroke::new(1.5, BLUE),
            egui::StrokeKind::Inside,
        );
    }
}

/// A rounded tag label in the tag's own colour. `selected` draws it
/// brighter (the active filter).
fn tag_chip(ui: &mut egui::Ui, tag: &str, selected: bool, sense: Sense) -> egui::Response {
    tag_chip_labelled(ui, tag, tag, selected, sense)
}

/// Like [`tag_chip`], with other text on it (such as "Miner ×") but the
/// tag's own colour.
fn tag_chip_labelled(
    ui: &mut egui::Ui,
    tag: &str,
    label: &str,
    selected: bool,
    sense: Sense,
) -> egui::Response {
    let (fill, text) = tag_colours(tag, selected);
    let galley =
        ui.painter()
            .layout_no_wrap(label.to_string(), egui::FontId::proportional(11.0), text);
    let size = galley.size() + Vec2::new(12.0, 2.0);
    let (rect, response) = ui.allocate_exact_size(size, sense);
    ui.painter().rect_filled(rect, CornerRadius::same(8), fill);
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(8),
        Stroke::new(1.0, text),
        egui::StrokeKind::Inside,
    );
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, text);
    if sense.senses_click() {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response
    }
}

/// A tag's colours, always the same for the same name: the name picks a hue.
fn tag_colours(tag: &str, selected: bool) -> (Color32, Color32) {
    let hue = tag_hue(tag);
    let fill = egui::ecolor::Hsva::new(hue, 0.45, if selected { 0.40 } else { 0.20 }, 1.0);
    let text = egui::ecolor::Hsva::new(hue, 0.35, 0.90, 1.0);
    (Color32::from(fill), Color32::from(text))
}

/// 0..1 from the tag's name (case ignored). The name picks one of ten
/// hues spread evenly around the colour wheel, so tags are easy to tell
/// apart. FNV-1a is a simple hash that gives the same answer in every
/// Athena version, unlike Rust's own hasher.
fn tag_hue(tag: &str) -> f32 {
    const HUES: u32 = 10;
    let mut hash: u32 = 0x811c_9dc5;
    for byte in tag.to_lowercase().bytes() {
        hash ^= byte as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    (hash % HUES) as f32 / HUES as f32
}

/// The "Tags…" window for one character.
fn tag_window(module: &Characters, ctx: &egui::Context, actions: &mut Vec<Action>) {
    let Some((id, text)) = &module.tag_editor else {
        return;
    };
    let Some(character) = module.list.character(*id) else {
        actions.push(Action::CloseTags);
        return;
    };
    let mut open = true;
    let mut text = text.clone();
    egui::Window::new(format!("Tags for {}", character.name))
        .id(egui::Id::new("character_tags"))
        .collapsible(false)
        .resizable(false)
        .pivot(egui::Align2::RIGHT_TOP)
        .default_pos(ctx.pointer_latest_pos().unwrap_or_default())
        .open(&mut open)
        .show(ctx, |ui| {
            ui.set_width(300.0);
            if character.tags.is_empty() {
                ui.label(small("No tags yet.").color(DIM));
            }
            ui.horizontal_wrapped(|ui| {
                for tag in &character.tags {
                    if tag_chip_labelled(ui, tag, &format!("{tag} ×"), false, Sense::click())
                        .on_hover_text("Remove this tag")
                        .clicked()
                    {
                        actions.push(Action::RemoveTag(*id, tag.clone()));
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label("Add tag");
                // `return_key(None)`: Enter doesn't end the editing, so
                // several tags can be typed one after another.
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut text)
                        .desired_width(180.0)
                        .return_key(None),
                );
                if edit.changed() {
                    actions.push(Action::TagText(text.clone()));
                }
                if edit.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    actions.push(Action::AddTag(*id, text.clone()));
                } else if ui.memory(|m| m.focused().is_none()) {
                    // Ready to type as soon as the window opens.
                    edit.request_focus();
                }
            });
            // Tags already used on other characters, matching what's typed.
            let typed = text.trim().to_lowercase();
            let known: Vec<String> = module
                .list
                .all_tags()
                .into_iter()
                .filter(|t| !character.tags.contains(t))
                .filter(|t| typed.is_empty() || t.to_lowercase().contains(&typed))
                .collect();
            if !known.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    ui.label(small("Known:").color(DIM));
                    for tag in known {
                        if tag_chip(ui, &tag, false, Sense::click()).clicked() {
                            actions.push(Action::AddTag(*id, tag));
                        }
                    }
                });
            }
            ui.label(
                small("Enter adds it. Each tag gets a fixed colour from its name.").color(DIM),
            );
        });
    if !open {
        actions.push(Action::CloseTags);
    }
}

fn needs_attention(module: &Characters, character: &Character) -> bool {
    module.statuses.get(&character.id) == Some(&TokenStatus::LoginExpired)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_colours_ignore_case_and_stay_put() {
        assert_eq!(tag_hue("Miner"), tag_hue("miner"));
        assert_ne!(tag_hue("Miner"), tag_hue("Hauler"));
    }

    #[test]
    fn columns_shrink_to_fit() {
        assert_eq!(fit_columns(3, 1440.0).0, 3);
        assert_eq!(fit_columns(4, 700.0).0, 2);
        assert_eq!(fit_columns(2, 200.0).0, 1);
        assert!(fit_columns(1, 2000.0).1 <= MAX_CARD_WIDTH);
    }
}
