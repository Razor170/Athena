//! Draws the Contracts panel from the mockup Kevin approved
//! (<https://claude.ai/artifact/Mjj1sMcvENcxp3WLyYiANe>): the contract
//! list on the left with filters and details on click, the changelog on
//! the right with a draggable split (like Intel), and a Settings view.
//!
//! The changelog has three parts: "While Athena was closed" (changes found
//! after a start that weren't seen yet), then one group per local day.
//! Clicking an entry (or "Mark all seen") moves it into its day.

use std::collections::{BTreeMap, HashSet};

use eframe::egui::{
    self, Align, Color32, CornerRadius, FontId, Label, Layout, Margin, Pos2, Rect, RichText, Sense,
    Stroke, Vec2, text::LayoutJob, text::TextWrapping,
};

use super::Contracts;
use super::changes::{AlertKind, ChangeKind, deadline};
use super::config::{StatusFilter, TypeFilter};
use super::data::{Contract, ContractType, Side, Stage, finished_label};
use super::store::LogEntry;
use crate::localtime;
use crate::modules::finance::data::short_isk;
use crate::modules::planetary::status::duration_text;

// Colours from the mockup (the same as Planetary and Finance).
const GREEN: Color32 = Color32::from_rgb(0x7c, 0xc4, 0x7c);
const RED: Color32 = Color32::from_rgb(0xef, 0x6b, 0x6b);
const YELLOW: Color32 = Color32::from_rgb(0xe0, 0xb0, 0x40);
const BLUE: Color32 = Color32::from_rgb(0x7f, 0xb8, 0xd8);
const PURPLE: Color32 = Color32::from_rgb(0xb4, 0x8c, 0xe8);
/// "While closed": the badge, the dot on a row and the group title.
const ORANGE: Color32 = Color32::from_rgb(0xe8, 0x91, 0x3a);
const BRIGHT: Color32 = Color32::from_rgb(0xf0, 0xf0, 0xf0);
const TEXT: Color32 = Color32::from_rgb(0xc8, 0xc8, 0xc8);
const DIM: Color32 = Color32::from_rgb(0x8a, 0x8a, 0x8a);
const TYPE_TEXT: Color32 = Color32::from_rgb(0xa9, 0xc4, 0xd8);
const CARD_FILL: Color32 = Color32::from_rgb(0x23, 0x23, 0x23);
const CARD_BORDER: Color32 = Color32::from_rgb(0x3a, 0x3a, 0x3a);
const ROW_LINE: Color32 = Color32::from_rgb(0x2f, 0x2f, 0x2f);
const ROW_HOVER: Color32 = Color32::from_rgb(0x2a, 0x2a, 0x2a);
const ROW_OPEN: Color32 = Color32::from_rgb(0x26, 0x2c, 0x31);
const OPEN_EDGE: Color32 = Color32::from_rgb(0x2c, 0x7d, 0xa3);
const CLOSED_FILL: Color32 = Color32::from_rgb(0x2b, 0x24, 0x20);
const CLOSED_EDGE: Color32 = Color32::from_rgb(0x6a, 0x4a, 0x2a);
const DETAIL_FILL: Color32 = Color32::from_rgb(0x1e, 0x1e, 0x1e);
const DETAIL_BORDER: Color32 = Color32::from_rgb(0x33, 0x33, 0x33);
const HANDLE: Color32 = Color32::from_rgb(0x55, 0x55, 0x55);
const BADGE_TEXT: Color32 = Color32::from_rgb(0x1b, 0x1b, 0x1b);

const ROW_HEIGHT: f32 = 24.0;
const CELL_GAP: f32 = 8.0;
/// Column widths of the list; the "Contract" column gets the rest.
const DOT_W: f32 = 12.0;
const STATUS_W: f32 = 110.0;
const TYPE_W: f32 = 92.0;
const CONTRACT_MIN_W: f32 = 150.0;
const CHARACTER_W: f32 = 120.0;
const PRICE_W: f32 = 84.0;
const WHERE_W: f32 = 150.0;
const TIME_W: f32 = 120.0;
const MIN_LIST_WIDTH: f32 = 420.0;
const MIN_LOG_WIDTH: f32 = 300.0;
const HANDLE_WIDTH: f32 = 12.0;
/// Items listed in a contract's details.
const MAX_ITEMS: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum View {
    #[default]
    Contracts,
    Settings,
}

fn small(text: impl Into<String>) -> RichText {
    RichText::new(text).size(12.0)
}

/// A label that doesn't take clicks or text selection (so a click on it
/// reaches the row or entry under it) and cuts long text with "…".
fn text(ui: &mut egui::Ui, text: RichText) {
    ui.add(Label::new(text).selectable(false).truncate());
}

/// The panel.
pub fn show(module: &mut Contracts, ui: &mut egui::Ui, now: i64) {
    ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
    view_tabs(module, ui, now);
    for problem in &module.problems {
        ui.colored_label(YELLOW, small(problem));
    }
    if module.view == View::Settings {
        settings_view(module, ui);
        return;
    }
    if !crate::auth::is_configured() {
        ui.colored_label(YELLOW, "EVE login isn't set up in this build.");
        return;
    }
    if module.characters.is_empty() && module.contracts.is_empty() {
        ui.label("Log in your characters on the Characters tab, and their contracts show up here.");
        return;
    }
    for c in &module.characters {
        if let Some(problem) = module.character_problems.get(&c.id) {
            ui.colored_label(YELLOW, small(format!("{}: {problem}", c.name)));
        }
    }
    if !module.loaded {
        ui.label(small("Reading saved contracts…").color(DIM));
    }
    split(module, ui, now);
}

fn view_tabs(module: &mut Contracts, ui: &mut egui::Ui, now: i64) {
    ui.horizontal(|ui| {
        ui.selectable_value(&mut module.view, View::Contracts, "Contracts");
        ui.selectable_value(&mut module.view, View::Settings, "Settings");
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui
                .add_enabled(module.can_refresh(now), egui::Button::new(small("Refresh")))
                .on_hover_text(
                    "Ask ESI for every contract list now. ESI may still send its cached copy.",
                )
                .on_disabled_hover_text("Just refreshed. Try again in a minute.")
                .clicked()
            {
                module.refresh_now();
            }
            let text = match module.checked {
                Some(at) => format!("Asked ESI {} · every 5 min", ago(now - at)),
                None => "Asking ESI…".to_string(),
            };
            ui.label(small(text).color(DIM));
        });
    });
}

/// "just now", "4 min ago", "5 h ago", "2 d ago".
fn ago(seconds: i64) -> String {
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

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// "13 Nov 06:51" in local time, or only "06:51" when it is today.
fn when_text(unix: i64, now: i64) -> String {
    let clock = &localtime::clock_text(unix)[..5];
    if localtime::local_day_start(unix) == localtime::local_day_start(now) {
        return clock.to_string();
    }
    let (_, month, day) = localtime::civil_from_days(localtime::to_local(unix).div_euclid(86_400));
    format!("{day} {} {clock}", MONTHS[(month - 1) as usize])
}

/// The list and the changelog, with a handle between them to drag the
/// split.
fn split(module: &mut Contracts, ui: &mut egui::Ui, now: i64) {
    let area = ui.available_rect_before_wrap();
    let mut log_width = module.settings.log_width.clamp(
        MIN_LOG_WIDTH,
        (area.width() - MIN_LIST_WIDTH - HANDLE_WIDTH).max(MIN_LOG_WIDTH),
    );
    // A very narrow tab: list and changelog share what there is.
    if area.width() < MIN_LIST_WIDTH + MIN_LOG_WIDTH + HANDLE_WIDTH {
        log_width = ((area.width() - HANDLE_WIDTH) / 2.0).max(0.0);
    }
    let list_rect = Rect::from_min_max(
        area.min,
        Pos2::new(area.right() - log_width - HANDLE_WIDTH, area.bottom()),
    );
    let handle_rect = Rect::from_min_max(
        Pos2::new(list_rect.right(), area.top()),
        Pos2::new(list_rect.right() + HANDLE_WIDTH, area.bottom()),
    );
    let log_rect = Rect::from_min_max(Pos2::new(handle_rect.right(), area.top()), area.max);

    let mine = module.mine();
    let mut list_ui = ui.new_child(egui::UiBuilder::new().max_rect(list_rect));
    contract_list(module, &mut list_ui, &mine, now);

    let handle = ui
        .interact(handle_rect, ui.id().with("contracts_split"), Sense::drag())
        .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
    let bar = Rect::from_center_size(handle_rect.center(), Vec2::new(4.0, 48.0));
    ui.painter().rect_filled(bar, CornerRadius::same(2), HANDLE);
    if handle.dragged() {
        module.settings.log_width = log_width - handle.drag_delta().x;
    }
    if handle.drag_stopped() {
        module.save_settings();
    }

    let mut log_ui = ui.new_child(egui::UiBuilder::new().max_rect(log_rect));
    changelog(module, &mut log_ui, &mine, now);
    ui.allocate_rect(area, Sense::hover());
}

/// One contract, ready to draw.
struct Row {
    id: i64,
    stage: Stage,
    status: &'static str,
    status_color: Color32,
    kind: &'static str,
    title: String,
    side: &'static str,
    character: String,
    price: String,
    location: String,
    time: String,
    time_color: Color32,
    /// It has a "while closed" entry not seen yet.
    closed_change: bool,
    /// Sort key: active ones by how soon they need you, finished ones by
    /// when they ended (newest first).
    sort: (u8, i64),
}

fn stage_color(stage: Stage) -> Color32 {
    match stage {
        Stage::Waiting | Stage::HasBids => BLUE,
        Stage::AssignedToYou | Stage::Overdue | Stage::Rejected | Stage::Failed => RED,
        Stage::InProgress => PURPLE,
        Stage::Finished => GREEN,
        Stage::Expired | Stage::Cancelled | Stage::Deleted | Stage::Reversed | Stage::Unknown => {
            DIM
        }
    }
}

fn type_matches(filter: Option<TypeFilter>, kind: ContractType) -> bool {
    match filter {
        None => true,
        Some(TypeFilter::ItemExchange) => kind == ContractType::ItemExchange,
        Some(TypeFilter::Courier) => kind == ContractType::Courier,
        Some(TypeFilter::Auction) => kind == ContractType::Auction,
    }
}

/// Whether one of the contract's characters has the picked tag.
fn tag_matches(module: &Contracts, contract: &Contract) -> bool {
    let Some(tag) = &module.settings.tag else {
        return true;
    };
    module
        .characters
        .iter()
        .filter(|c| contract.characters.contains(&c.id) || involved(contract, c.id))
        .any(|c| c.tags.contains(tag))
}

fn involved(contract: &Contract, id: u64) -> bool {
    let id = id as i64;
    contract.issuer_id == id || contract.acceptor_id == id || contract.assignee_id == id
}

/// Everything the search box looks at, lower case.
fn search_text(module: &Contracts, contract: &Contract, row_title: &str) -> String {
    let mut parts = vec![
        row_title.to_string(),
        contract.title.clone(),
        contract.kind.label().to_string(),
    ];
    for id in [
        contract.issuer_id,
        contract.acceptor_id,
        contract.assignee_id,
        contract.start_location_id,
        contract.end_location_id,
    ] {
        if let Some(name) = module.names.get(&id) {
            parts.push(name.clone());
        }
    }
    for c in &contract.characters {
        parts.push(module.character_name(*c));
    }
    for item in contract.items.iter().flatten() {
        if let Some(name) = module.names.get(&item.type_id) {
            parts.push(name.clone());
        }
    }
    parts.join(" ").to_lowercase()
}

/// The rows that pass the filters, sorted. Also returns the counts for
/// the summary line, over all contracts with the tag and type filter.
fn rows(module: &Contracts, mine: &[u64], now: i64) -> (Vec<Row>, Counts) {
    let soon_secs = i64::from(module.settings.soon_hours) * 3600;
    let closed: HashSet<i64> = module
        .log
        .iter()
        .filter(|e| e.offline && !e.seen)
        .map(|e| e.contract_id)
        .collect();
    let needle = module.search.trim().to_lowercase();
    let mut counts = Counts::default();
    let mut out = Vec::new();
    for contract in module.contracts.values() {
        if !type_matches(module.settings.type_filter, contract.kind)
            || !tag_matches(module, contract)
        {
            continue;
        }
        let stage = contract.stage(mine, now);
        let (side, character) = contract.side(mine);
        let finished_cutoff = match module.settings.finished_days {
            0 => i64::MIN,
            days => now - i64::from(days) * 86_400,
        };
        let recent = stage.is_active() || contract.ended_at(stage) >= finished_cutoff;
        if !recent {
            continue;
        }
        let soon = deadline(contract, stage, side).is_some_and(|d| d - now <= soon_secs);
        if stage.is_active() {
            counts.active += 1;
            match stage {
                Stage::InProgress | Stage::Overdue => counts.in_progress += 1,
                Stage::AssignedToYou => counts.assigned += 1,
                _ => counts.waiting += 1,
            }
            if soon {
                counts.soon += 1;
            }
        }
        let shown = match module.settings.status_filter {
            StatusFilter::Active => stage.is_active(),
            StatusFilter::Finished => !stage.is_active(),
            StatusFilter::All => true,
        };
        if !shown {
            continue;
        }
        let title = module.contract_title(contract);
        if !needle.is_empty() && !search_text(module, contract, &title).contains(&needle) {
            continue;
        }
        let (time, time_color, sort) = time_cell(contract, stage, side, soon, now);
        let location = if contract.start_location_id > 0 {
            module.name(contract.start_location_id)
        } else {
            String::new()
        };
        out.push(Row {
            id: contract.id,
            stage,
            status: stage.label(contract, side),
            status_color: stage_color(stage),
            kind: contract.kind.label(),
            title,
            side: side.label(),
            character: character.map_or_else(String::new, |id| module.character_name(id)),
            price: module.price_text(contract, side),
            location,
            time,
            time_color,
            closed_change: closed.contains(&contract.id),
            sort,
        });
    }
    out.sort_by(|a, b| a.sort.cmp(&b.sort).then(a.id.cmp(&b.id)));
    (out, counts)
}

#[derive(Default)]
struct Counts {
    active: usize,
    waiting: usize,
    in_progress: usize,
    assigned: usize,
    soon: usize,
}

/// The "Time" column, its colour and the row's sort key.
fn time_cell(
    contract: &Contract,
    stage: Stage,
    side: Side,
    soon: bool,
    now: i64,
) -> (String, Color32, (u8, i64)) {
    let soon_color = if soon { YELLOW } else { TEXT };
    match stage {
        Stage::AssignedToYou => (
            format!("expires in {}", duration_text(contract.expires - now)),
            soon_color,
            (0, contract.expires),
        ),
        Stage::Overdue => {
            let due = contract.due().unwrap_or(contract.expires);
            (
                format!("overdue {}", duration_text(now - due)),
                RED,
                (0, due),
            )
        }
        Stage::Waiting | Stage::HasBids => {
            let word = if contract.kind == ContractType::Auction {
                "ends"
            } else {
                "expires"
            };
            (
                format!("{word} in {}", duration_text(contract.expires - now)),
                soon_color,
                (1, contract.expires),
            )
        }
        Stage::InProgress => match contract.due() {
            Some(due) => (
                format!("due in {}", duration_text(due - now)),
                if side == Side::Accepted {
                    soon_color
                } else {
                    TEXT
                },
                (1, due),
            ),
            None => ("in progress".to_string(), TEXT, (1, i64::MAX)),
        },
        _ => {
            let ended = contract.ended_at(stage);
            (when_text(ended, now), DIM, (2, -ended))
        }
    }
}

fn contract_list(module: &mut Contracts, ui: &mut egui::Ui, mine: &[u64], now: i64) {
    ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
    let (rows, counts) = rows(module, mine, now);
    filters(module, ui, counts.active);
    ui.horizontal_wrapped(|ui| {
        let chars = module.characters.len();
        text(
            ui,
            small(format!(
                "{chars} characters · {} active contracts",
                counts.active
            ))
            .color(DIM),
        );
        ui.add_space(10.0);
        for (count, label, color) in [
            (counts.waiting, "waiting", BLUE),
            (counts.in_progress, "in progress", PURPLE),
            (counts.soon, "expire or due soon", YELLOW),
            (counts.assigned, "assigned to you", RED),
        ] {
            if count > 0 {
                dot(ui, color);
                text(ui, small(format!("{count} {label}")).color(DIM));
                ui.add_space(8.0);
            }
        }
    });

    egui::Frame::new()
        .fill(CARD_FILL)
        .stroke(Stroke::new(1.0, CARD_BORDER))
        .corner_radius(CornerRadius::same(4))
        .inner_margin(Margin::same(0))
        .show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            header_row(ui);
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if rows.is_empty() {
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.add_space(8.0);
                            text(ui, small(empty_text(module)).color(DIM));
                        });
                    }
                    for row in &rows {
                        contract_row(module, ui, row, mine, now);
                    }
                });
        });
}

fn empty_text(module: &Contracts) -> &'static str {
    if !module.loaded || module.checked.is_none() {
        "Fetching contracts…"
    } else if module.contracts.is_empty() {
        "No contracts in the last 30 days."
    } else {
        "No contracts match the filters."
    }
}

fn filters(module: &mut Contracts, ui: &mut egui::Ui, active: usize) {
    let mut tags: Vec<String> = module
        .characters
        .iter()
        .flat_map(|c| c.tags.iter().cloned())
        .collect();
    tags.sort();
    tags.dedup();
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        let status = &mut module.settings.status_filter;
        for (value, label) in [
            (StatusFilter::Active, format!("Active ({active})")),
            (StatusFilter::Finished, "Finished".to_string()),
            (StatusFilter::All, "All".to_string()),
        ] {
            changed |= ui.selectable_value(status, value, small(label)).changed();
        }
        ui.add_space(10.0);
        let kind = &mut module.settings.type_filter;
        for (value, label) in [
            (None, "All types"),
            (Some(TypeFilter::ItemExchange), "Item exchange"),
            (Some(TypeFilter::Courier), "Courier"),
            (Some(TypeFilter::Auction), "Auction"),
        ] {
            changed |= ui.selectable_value(kind, value, small(label)).changed();
        }
        if !tags.is_empty() {
            ui.add_space(8.0);
            ui.label(small("Tag").color(DIM));
            for tag in &tags {
                let picked = module.settings.tag.as_ref() == Some(tag);
                if ui
                    .add(egui::Button::selectable(picked, small(tag)).corner_radius(8))
                    .on_hover_text(
                        "Only contracts of characters with this tag (click again for all)",
                    )
                    .clicked()
                {
                    module.settings.tag = if picked { None } else { Some(tag.clone()) };
                    changed = true;
                }
            }
        }
        ui.add_space(8.0);
        ui.add(
            egui::TextEdit::singleline(&mut module.search)
                .hint_text("Search title, item, place, name")
                .desired_width(190.0),
        );
    });
    if changed {
        module.save_settings();
    }
}

/// A small coloured dot, as in the summary line. Painted, because the
/// built-in fonts have no "●".
fn dot(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(8.0, 12.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 3.5, color);
}

/// The x positions of the columns in a row of `rect`.
struct Columns {
    dot: Rect,
    status: Rect,
    kind: Rect,
    contract: Rect,
    character: Rect,
    price: Rect,
    location: Rect,
    time: Rect,
}

fn columns(rect: Rect) -> Columns {
    // In a narrow list, "Where" goes first, then "Type", so the contract
    // keeps room for its title.
    let base = DOT_W + STATUS_W + CHARACTER_W + PRICE_W + TIME_W + CELL_GAP * 7.0;
    let room = |extra: f32| rect.width() - base - extra;
    let (type_w, where_w) = if room(TYPE_W + WHERE_W + CELL_GAP * 2.0) >= CONTRACT_MIN_W {
        (TYPE_W, WHERE_W)
    } else if room(TYPE_W + CELL_GAP) >= CONTRACT_MIN_W {
        (TYPE_W, 0.0)
    } else {
        (0.0, 0.0)
    };
    let gaps = |w: f32| if w > 0.0 { CELL_GAP } else { 0.0 };
    let contract_w = room(type_w + where_w + gaps(type_w) + gaps(where_w)).max(40.0);
    let mut x = rect.left() + CELL_GAP;
    let mut next = |w: f32| {
        let r = Rect::from_min_max(Pos2::new(x, rect.top()), Pos2::new(x + w, rect.bottom()));
        if w > 0.0 {
            x += w + CELL_GAP;
        }
        r
    };
    Columns {
        dot: next(DOT_W),
        status: next(STATUS_W),
        kind: next(type_w),
        contract: next(contract_w),
        character: next(CHARACTER_W),
        price: next(PRICE_W),
        location: next(where_w),
        time: next(TIME_W),
    }
}

/// One line of text in a cell, cut with "…" when too long. Nothing is
/// drawn in a column hidden for lack of room (width 0).
fn paint_cell(ui: &egui::Ui, rect: Rect, text: &str, size: f32, color: Color32, right: bool) {
    if rect.width() < 1.0 {
        return;
    }
    let mut job = LayoutJob::simple_singleline(text.to_string(), FontId::proportional(size), color);
    job.wrap = TextWrapping::truncate_at_width(rect.width().max(1.0));
    let galley = ui.painter().layout_job(job);
    let x = if right {
        rect.right() - galley.size().x
    } else {
        rect.left()
    };
    let pos = Pos2::new(x, rect.center().y - galley.size().y / 2.0);
    // Clipped to the cell, so nothing spills into the changelog.
    ui.painter()
        .with_clip_rect(ui.clip_rect().intersect(rect.expand(1.0)))
        .galley(pos, galley, color);
}

fn header_row(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), Sense::hover());
    let c = columns(rect);
    for (cell, label, right) in [
        (c.status, "Status", false),
        (c.kind, "Type", false),
        (c.contract, "Contract", false),
        (c.character, "Character", false),
        (c.price, "Price / reward", true),
        (c.location, "Where", false),
        (c.time, "Time", true),
    ] {
        paint_cell(ui, cell, label, 12.0, DIM, right);
    }
    ui.painter().hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1.0, CARD_BORDER),
    );
}

fn contract_row(module: &mut Contracts, ui: &mut egui::Ui, row: &Row, mine: &[u64], now: i64) {
    let open = module.open.contains(&row.id);
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_HEIGHT), Sense::click());
    if module.scroll_to == Some(row.id) {
        response.scroll_to_me(Some(Align::Center));
        module.scroll_to = None;
    }
    let fill = if open {
        Some(ROW_OPEN)
    } else if response.hovered() {
        Some(ROW_HOVER)
    } else {
        None
    };
    if let Some(fill) = fill {
        ui.painter().rect_filled(rect, CornerRadius::ZERO, fill);
    }
    let c = columns(rect);
    if row.closed_change {
        ui.painter()
            .circle_filled(c.dot.left_center() + Vec2::new(4.0, 0.0), 3.5, ORANGE);
    }
    paint_cell(ui, c.status, row.status, 13.0, row.status_color, false);
    paint_cell(ui, c.kind, row.kind, 13.0, TYPE_TEXT, false);
    // Title and "you issued" share the Contract column.
    let side_width = if row.side.is_empty() { 0.0 } else { 80.0 };
    let title_rect = Rect::from_min_max(
        c.contract.min,
        Pos2::new(c.contract.right() - side_width, c.contract.bottom()),
    );
    paint_cell(ui, title_rect, &row.title, 13.0, BRIGHT, false);
    if side_width > 0.0 {
        let side_rect = Rect::from_min_max(
            Pos2::new(title_rect.right() + 4.0, c.contract.top()),
            c.contract.max,
        );
        paint_cell(ui, side_rect, row.side, 12.0, DIM, true);
    }
    paint_cell(ui, c.character, &row.character, 13.0, TEXT, false);
    paint_cell(ui, c.price, &row.price, 13.0, BRIGHT, true);
    paint_cell(ui, c.location, &row.location, 13.0, TEXT, false);
    paint_cell(ui, c.time, &row.time, 13.0, row.time_color, true);
    ui.painter().hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1.0, ROW_LINE),
    );
    if response.clicked() {
        if open {
            module.open.remove(&row.id);
        } else {
            module.open.insert(row.id);
        }
    }
    if open && let Some(contract) = module.contracts.get(&row.id).cloned() {
        details(module, ui, &contract, row.stage, mine, now);
    }
}

/// The open details of a contract: the facts on the left, the items on
/// the right.
fn details(
    module: &Contracts,
    ui: &mut egui::Ui,
    contract: &Contract,
    stage: Stage,
    mine: &[u64],
    now: i64,
) {
    let side = contract.side(mine).0;
    let mut facts: Vec<(&str, String, Color32)> = Vec::new();
    let mut fact = |k: &'static str, v: String| facts.push((k, v, TEXT));
    let availability = match contract.availability.as_str() {
        "public" => "Public",
        "personal" => "Private",
        "corporation" => "Corporation",
        "alliance" => "Alliance",
        _ => "",
    };
    fact(
        "Type",
        format!("{} · {availability}", contract.kind.label()),
    );
    if side != Side::Issued {
        fact("From", module.name(contract.issuer_id));
    }
    if contract.assignee_id > 0 && side != Side::AssignedToMe {
        fact("For", module.name(contract.assignee_id));
    }
    let isk = |value: f64| format!("{} ISK", short_isk(value));
    match contract.kind {
        ContractType::Courier => {
            fact(
                "Route",
                format!(
                    "{} » {}",
                    module.name(contract.start_location_id),
                    module.name(contract.end_location_id)
                ),
            );
            fact(
                "Volume",
                format!("{} m³", super::thousands(contract.volume)),
            );
            fact("Reward", isk(contract.reward));
            fact("Collateral", isk(contract.collateral));
        }
        ContractType::Auction => {
            match contract.top_bid() {
                Some(bid) => fact(
                    "Top bid",
                    format!(
                        "{} by {} ({} bids)",
                        isk(bid.amount),
                        module.name(bid.bidder_id),
                        contract.bids.len()
                    ),
                ),
                None => fact("Top bid", "no bids yet".to_string()),
            }
            fact("Starting bid", isk(contract.price));
            if contract.buyout > 0.0 {
                fact("Buyout", isk(contract.buyout));
            }
        }
        _ => {
            if contract.price > 0.0 {
                fact("Price", isk(contract.price));
            }
            if contract.reward > 0.0 {
                fact("Issuer pays", isk(contract.reward));
            }
        }
    }
    if contract.kind != ContractType::Courier && contract.start_location_id > 0 {
        fact("Location", module.name(contract.start_location_id));
    }
    if contract.acceptor_id > 0 {
        let when = contract
            .accepted
            .map(|at| format!(", {}", when_text(at, now)))
            .unwrap_or_default();
        fact(
            "Accepted by",
            format!("{}{when}", module.name(contract.acceptor_id)),
        );
    }
    if let Some(due) = contract.due() {
        fact("Deliver by", when_text(due, now));
    }
    fact("Issued", when_text(contract.issued, now));
    if stage.is_open() {
        fact("Expires", when_text(contract.expires, now));
    } else if !stage.is_active() {
        fact("Ended", when_text(contract.ended_at(stage), now));
    }

    let mut items: Vec<(String, String)> = Vec::new();
    let items_head = match &contract.items {
        None => "Loading items…".to_string(),
        Some(list) if list.is_empty() => {
            if contract.kind == ContractType::Courier {
                "Items: hidden by EVE".to_string()
            } else {
                "No items".to_string()
            }
        }
        Some(list) => {
            for item in list.iter().take(MAX_ITEMS) {
                let name = if item.included {
                    module.name(item.type_id)
                } else {
                    format!("Wanted: {}", module.name(item.type_id))
                };
                items.push((name, super::thousands(item.quantity as f64)));
            }
            if list.len() > MAX_ITEMS {
                items.push((format!("+ {} more", list.len() - MAX_ITEMS), String::new()));
            }
            format!("{} item types", list.len())
        }
    };
    let foot = match (stage, contract.kind) {
        (Stage::AssignedToYou, _) => "Accept it in EVE. Athena only reads contracts.",
        (Stage::Expired, _) if side == Side::Issued => "Delete it in EVE to get the items back.",
        _ => "",
    };

    egui::Frame::new()
        .fill(DETAIL_FILL)
        .stroke(Stroke::new(1.0, DETAIL_BORDER))
        .corner_radius(CornerRadius::same(3))
        .inner_margin(Margin::symmetric(8, 6))
        .outer_margin(Margin {
            left: 22,
            right: 8,
            top: 2,
            bottom: 6,
        })
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::new(8.0, 3.0);
            ui.columns(2, |cols| {
                egui::Grid::new(("contract_facts", contract.id))
                    .num_columns(2)
                    .spacing([8.0, 3.0])
                    .show(&mut cols[0], |ui| {
                        for (k, v, color) in &facts {
                            ui.label(small(*k).color(DIM));
                            ui.add(Label::new(small(v).color(*color)).wrap());
                            ui.end_row();
                        }
                    });
                let ui = &mut cols[1];
                ui.label(small(&items_head).color(DIM));
                for (name, quantity) in &items {
                    ui.horizontal(|ui| {
                        ui.label(small(name));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(small(quantity).color(DIM));
                        });
                    });
                }
                if !foot.is_empty() {
                    ui.add_space(4.0);
                    ui.label(small(foot).color(DIM));
                }
            });
        });
}

/// The changelog on the right.
fn changelog(module: &mut Contracts, ui: &mut egui::Ui, mine: &[u64], now: i64) {
    ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
    let closed_count = module.log.iter().filter(|e| e.offline && !e.seen).count();
    let unseen = module.unseen_count();
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(RichText::new("Changelog").size(14.0).color(BRIGHT));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui
                .add_enabled(unseen > 0, egui::Button::new(small("Mark all seen")))
                .clicked()
            {
                module.mark_all_seen();
            }
            let only = &mut module.settings.log_only_closed;
            changed |= ui
                .selectable_value(only, true, small(format!("While closed ({closed_count})")))
                .changed();
            changed |= ui.selectable_value(only, false, small("All")).changed();
        });
    });
    if changed {
        module.save_settings();
    }
    text(
        ui,
        small("Saved on disk, so it keeps growing past ESI's 30 days. Click an entry to show its contract.")
            .color(DIM),
    );

    // Entries before the cut, split into the "while closed" group and the
    // day groups (local days, newest first).
    let shown: Vec<LogEntry> = module.log.iter().take(module.log_shown).cloned().collect();
    let more = module.log.len() > shown.len();
    let closed: Vec<&LogEntry> = shown.iter().filter(|e| e.offline && !e.seen).collect();
    let mut days: BTreeMap<i64, Vec<&LogEntry>> = BTreeMap::new();
    if !module.settings.log_only_closed {
        for entry in shown.iter().filter(|e| !(e.offline && !e.seen)) {
            days.entry(localtime::local_day_start(entry.at))
                .or_default()
                .push(entry);
        }
    }

    let mut clicked: Option<i64> = None;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::new(6.0, 3.0);
            if module.log.is_empty() {
                text(
                    ui,
                    small("Nothing yet. Changes to your contracts show up here.").color(DIM),
                );
            }
            if !closed.is_empty() {
                let sub = format!(
                    "until Athena started at {}",
                    when_text(module.started_at, now)
                );
                group_header(ui, "While Athena was closed", &sub, ORANGE);
                for entry in &closed {
                    if log_entry(module, ui, entry, mine, now) {
                        clicked = Some(entry.id);
                    }
                }
                ui.add_space(6.0);
            } else if module.settings.log_only_closed {
                text(
                    ui,
                    small("Nothing new from while Athena was closed.").color(DIM),
                );
            }
            let today = localtime::local_day_start(now);
            for (day, entries) in days.iter().rev() {
                let (title, color) = day_title(*day, today);
                let sub = if *day == today {
                    format!("Athena started {}", when_text(module.started_at, now))
                } else {
                    String::new()
                };
                group_header(ui, &title, &sub, color);
                for entry in entries {
                    if log_entry(module, ui, entry, mine, now) {
                        clicked = Some(entry.id);
                    }
                }
                ui.add_space(6.0);
            }
            if more && ui.button(small("Show older")).clicked() {
                module.log_shown += super::LOG_PAGE;
            }
        });
    if let Some(entry_id) = clicked {
        open_entry(module, entry_id, mine, now);
    }
}

/// "Today, 13 Nov", "Yesterday, 12 Nov" or "Wed 11 Nov 2026".
fn day_title(day_start: i64, today: i64) -> (String, Color32) {
    let date = localtime::date_text(day_start);
    // "Fri 13 Nov 2026" -> "13 Nov" for today and yesterday.
    let short: String = date
        .split(' ')
        .skip(1)
        .take(2)
        .collect::<Vec<_>>()
        .join(" ");
    if day_start == today {
        (format!("Today, {short}"), BRIGHT)
    } else if localtime::local_day_start(today - 1) == day_start {
        (format!("Yesterday, {short}"), TEXT)
    } else {
        (date, TEXT)
    }
}

fn group_header(ui: &mut egui::Ui, title: &str, sub: &str, color: Color32) {
    ui.horizontal(|ui| {
        text(ui, small(title).color(color));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            text(ui, small(sub).color(DIM));
        });
    });
    let y = ui.cursor().top();
    ui.painter().hline(
        ui.max_rect().x_range(),
        y - 1.0,
        Stroke::new(1.0, DETAIL_BORDER),
    );
    ui.add_space(2.0);
}

/// What the entry says on its first line, and its colour.
fn entry_label(kind: ChangeKind, contract: Option<&Contract>, side: Side) -> (String, Color32) {
    let (text, color) = match kind {
        ChangeKind::Issued => ("Issued", BLUE),
        ChangeKind::NewForYou => ("New contract for you", RED),
        ChangeKind::YouAccepted => ("You accepted", PURPLE),
        ChangeKind::Accepted => ("Accepted", PURPLE),
        ChangeKind::Finished => (
            contract.map_or("Finished", |c| finished_label(c, side)),
            GREEN,
        ),
        ChangeKind::NewBid => ("New bid", BLUE),
        ChangeKind::Expired => ("Expired", DIM),
        ChangeKind::Cancelled => ("Cancelled", DIM),
        ChangeKind::Rejected => ("Rejected", RED),
        ChangeKind::Failed => ("Failed", RED),
        ChangeKind::Deleted => ("Deleted", DIM),
        ChangeKind::Reversed => ("Reversed", DIM),
        ChangeKind::Overdue => ("Overdue", RED),
        ChangeKind::ExpiresSoon => ("Expires soon", YELLOW),
        ChangeKind::DueSoon => ("Due soon", YELLOW),
    };
    (text.to_string(), color)
}

/// The third line: our character, and what else is worth knowing.
fn entry_detail(
    module: &Contracts,
    entry: &LogEntry,
    contract: &Contract,
    character: Option<u64>,
    now: i64,
) -> String {
    let mut parts = Vec::new();
    if let Some(id) = character {
        parts.push(module.character_name(id));
    }
    let extra = match entry.kind {
        ChangeKind::Issued => format!(
            "{}, {}",
            contract.kind.label(),
            module.price_text(contract, Side::Issued)
        ),
        ChangeKind::NewForYou => format!("from {}", module.name(contract.issuer_id)),
        ChangeKind::YouAccepted => match contract.due() {
            Some(due) => format!("deliver by {}", when_text(due, now)),
            None => format!("from {}", module.name(contract.issuer_id)),
        },
        ChangeKind::Accepted => match contract.due() {
            Some(due) => format!(
                "hauler {}, due {}",
                module.name(contract.acceptor_id),
                when_text(due, now)
            ),
            None => format!("by {}", module.name(contract.acceptor_id)),
        },
        ChangeKind::Finished if contract.acceptor_id > 0 => match contract.kind {
            ContractType::Courier => format!("by {}", module.name(contract.acceptor_id)),
            _ => format!(
                "{} for {}",
                module.name(contract.acceptor_id),
                module.price_text(contract, Side::Issued)
            ),
        },
        ChangeKind::NewBid => format!(
            "{}, {} bids",
            entry.amount.map(short_isk).unwrap_or_default(),
            contract.bids.len()
        ),
        ChangeKind::Expired => "nobody accepted it".to_string(),
        ChangeKind::Overdue => contract
            .due()
            .map(|due| format!("was due {}", when_text(due, now)))
            .unwrap_or_default(),
        ChangeKind::ExpiresSoon => format!("expires {}", when_text(contract.expires, now)),
        ChangeKind::DueSoon => contract
            .due()
            .map(|due| format!("due {}", when_text(due, now)))
            .unwrap_or_default(),
        _ => String::new(),
    };
    if !extra.is_empty() {
        parts.push(extra);
    }
    parts.join(" · ")
}

/// Draws one entry. Returns true when it was clicked.
fn log_entry(
    module: &Contracts,
    ui: &mut egui::Ui,
    entry: &LogEntry,
    mine: &[u64],
    now: i64,
) -> bool {
    let contract = module.contracts.get(&entry.contract_id);
    let (side, character) = contract.map_or((Side::Other, None), |c| c.side(mine));
    let (what, color) = entry_label(entry.kind, contract, side);
    let title = contract.map_or_else(
        || format!("Contract #{}", entry.contract_id),
        |c| module.contract_title(c),
    );
    let detail = contract
        .map(|c| entry_detail(module, entry, c, character, now))
        .unwrap_or_default();
    let closed = entry.offline && !entry.seen;
    let fresh = !entry.offline && !entry.seen;
    let selected = module.open.contains(&entry.contract_id);
    let (fill, edge) = if closed {
        (CLOSED_FILL, CLOSED_EDGE)
    } else if selected {
        (ROW_OPEN, OPEN_EDGE)
    } else {
        (CARD_FILL, ROW_LINE)
    };
    let response = egui::Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0, edge))
        .corner_radius(CornerRadius::same(3))
        .inner_margin(Margin::symmetric(6, 4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = Vec2::new(6.0, 1.0);
            ui.horizontal_top(|ui| {
                ui.add_sized(
                    [38.0, 16.0],
                    Label::new(small(&localtime::clock_text(entry.at)[..5]).color(DIM))
                        .selectable(false),
                );
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        text(ui, RichText::new(&what).color(color));
                        if closed {
                            badge(ui, "while closed", ORANGE);
                        } else if fresh {
                            badge(ui, "new", BLUE);
                        }
                    });
                    text(ui, RichText::new(&title).color(BRIGHT));
                    if !detail.is_empty() {
                        text(ui, small(&detail).color(DIM));
                    }
                });
            });
        })
        .response
        .interact(Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    response.clicked()
}

fn badge(ui: &mut egui::Ui, label: &str, fill: Color32) {
    egui::Frame::new()
        .fill(fill)
        .corner_radius(CornerRadius::same(3))
        .inner_margin(Margin::symmetric(4, 0))
        .show(ui, |ui| {
            text(ui, small(label).color(BADGE_TEXT).strong());
        });
}

/// A click on a changelog entry: it counts as seen, and its contract is
/// opened in the list (filters changed so it shows).
fn open_entry(module: &mut Contracts, entry_id: i64, mine: &[u64], now: i64) {
    module.mark_seen(entry_id);
    let Some(contract_id) = module
        .log
        .iter()
        .find(|e| e.id == entry_id)
        .map(|e| e.contract_id)
    else {
        return;
    };
    let Some(contract) = module.contracts.get(&contract_id) else {
        return;
    };
    let stage = contract.stage(mine, now);
    let old_finished = module.settings.finished_days > 0
        && contract.ended_at(stage) < now - i64::from(module.settings.finished_days) * 86_400;
    module.settings.status_filter = if stage.is_active() {
        StatusFilter::Active
    } else {
        StatusFilter::Finished
    };
    if !stage.is_active() && old_finished {
        // Shown under "Finished" only up to the days set in Settings.
        module.settings.finished_days = 0;
    }
    module.settings.type_filter = None;
    module.settings.tag = None;
    module.search.clear();
    module.open.clear();
    module.open.insert(contract_id);
    module.scroll_to = Some(contract_id);
    module.save_settings();
}

fn settings_view(module: &mut Contracts, ui: &mut egui::Ui) {
    let mut changed = false;
    let mut soon_changed = false;
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.set_max_width(480.0);
        ui.label(RichText::new("Contracts").size(14.0).color(BRIGHT));
        ui.label(
            small("Athena asks ESI every 5 minutes (ESI's own cache time) for each logged-in character.")
                .color(DIM),
        );
        ui.add_space(6.0);
        ui.label(RichText::new("Tell me when").color(BRIGHT));
        egui::Grid::new("contracts_alerts")
            .num_columns(3)
            .spacing([12.0, 4.0])
            .show(ui, |ui| {
                ui.label("");
                ui.label(small("Alert").color(DIM));
                ui.label(small("Sound").color(DIM));
                ui.end_row();
                for kind in AlertKind::ALL {
                    let switch = module.settings.alerts.get_mut(kind);
                    ui.label(kind.label());
                    changed |= ui.checkbox(&mut switch.alert, "").changed();
                    changed |= ui.checkbox(&mut switch.sound, "").changed();
                    ui.end_row();
                }
            });
        ui.label(
            small(
                "Alert = the count on the Contracts button and a taskbar flash. Every change \
                 always lands in the changelog, and changes from while Athena was closed are \
                 always marked.",
            )
            .color(DIM),
        );
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label("\"Expires soon\" and \"due soon\" warning");
            let response =
                ui.add(egui::DragValue::new(&mut module.settings.soon_hours).range(1..=168));
            // Only once the drag ends or the typed number is done, not on
            // every pixel of a drag.
            let editing = response.dragged() || response.has_focus();
            if response.changed() && !editing {
                soon_changed = true;
            }
            if response.drag_stopped() || response.lost_focus() {
                soon_changed = true;
            }
            ui.label("hours before");
        });
        ui.horizontal(|ui| {
            ui.label("Show finished contracts from the last");
            let label = |days: u32| match days {
                0 => "Forever".to_string(),
                days => format!("{days} days"),
            };
            egui::ComboBox::from_id_salt("contracts_finished_days")
                .selected_text(label(module.settings.finished_days))
                .show_ui(ui, |ui| {
                    for days in [7, 30, 90, 0] {
                        changed |= ui
                            .selectable_value(&mut module.settings.finished_days, days, label(days))
                            .changed();
                    }
                });
        });
        ui.label(small("The changelog is kept forever in contracts.sqlite next to Athena.").color(DIM));
    });
    if soon_changed {
        module.soon_hours_changed();
    } else if changed {
        module.save_settings();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn day_titles() {
        // Tests run on Linux, where local time is UTC. 2026-11-13 00:00 UTC.
        let today = 1_794_528_000;
        assert_eq!(day_title(today, today).0, "Today, 13 Nov");
        assert_eq!(day_title(today - 86_400, today).0, "Yesterday, 12 Nov");
        assert_eq!(day_title(today - 2 * 86_400, today).0, "Wed 11 Nov 2026");
        assert_eq!(when_text(today + 3600, today + 7200), "01:00");
        assert_eq!(when_text(today - 3600, today + 7200), "12 Nov 23:00");
    }
}
