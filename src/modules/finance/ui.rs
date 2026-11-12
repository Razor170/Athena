//! Draws the Finance panel: the Wallets, Overview, Transactions and Loyalty
//! Points tabs from the mockup Kevin approved
//! (<https://claude.ai/artifact/8GttDB3K7NQ5oJTrEX6HrF>).
//!
//! Drawing only reads the module's data and records clicks. Lists that
//! take work to build (the filtered transactions, jump distances) are
//! cached in [`ViewState`] and only rebuilt when the data or a filter
//! changes, so scrolling stays cheap.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::f32::consts::TAU;

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontId, Layout, Pos2, Rect, RichText, Sense,
    Stroke, Vec2, epaint::Mesh, text::LayoutJob, text::TextWrapping,
};

use super::Finance;
use super::categories::{Category, type_label};
use super::chart::{self, ChartData, Zoom};
use super::config::ChartMode;
use super::data::{Direction, JournalEntry, Slice, SliceKey, Totals, full_isk, short_isk, slices};
use crate::auth::now_unix;
use crate::localtime;

// Colours from the mockup.
const GREEN: Color32 = Color32::from_rgb(0x7c, 0xc4, 0x7c);
const RED: Color32 = Color32::from_rgb(0xef, 0x6b, 0x6b);
const YELLOW: Color32 = Color32::from_rgb(0xe0, 0xb0, 0x40);
const BRIGHT: Color32 = Color32::from_rgb(0xf0, 0xf0, 0xf0);
const TEXT: Color32 = Color32::from_rgb(0xc8, 0xc8, 0xc8);
const DIM: Color32 = Color32::from_rgb(0x8a, 0x8a, 0x8a);
const CARD_FILL: Color32 = Color32::from_rgb(0x23, 0x23, 0x23);
const CARD_HOVER: Color32 = Color32::from_rgb(0x2b, 0x2b, 0x2b);
const CARD_BORDER: Color32 = Color32::from_rgb(0x3a, 0x3a, 0x3a);
const PICKED_FILL: Color32 = Color32::from_rgb(0x0f, 0x3a, 0x4d);
const PICKED_BORDER: Color32 = Color32::from_rgb(0x2c, 0x7d, 0xa3);
const ROW_LINE: Color32 = Color32::from_rgb(0x2a, 0x2a, 0x2a);

/// Cards are at least this wide; as many columns as fit.
const CARD_MIN_WIDTH: f32 = 220.0;
const GAP: f32 = 6.0;
const WALLET_CARD_HEIGHT: f32 = 44.0;
const LP_CARD_HEIGHT: f32 = 60.0;
/// The ring is at most this big.
const RING_MAX: f32 = 440.0;
const SIDE_WIDTH: f32 = 270.0;
/// Rows of the Transactions list.
const DAY_ROW: f32 = 24.0;
const ENTRY_ROW: f32 = 42.0;
const EXTRA_LINE: f32 = 16.0;
/// The nearest LP store is searched this many jumps out.
const MAX_LP_JUMPS: u32 = 200;

fn small(text: impl Into<String>) -> RichText {
    RichText::new(text).size(11.5)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Wallets,
    Overview,
    Transactions,
    Loyalty,
}

/// The time range pickers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum Range {
    Today,
    Days7,
    #[default]
    Days30,
    Days90,
    ThisMonth,
    LastMonth,
    All,
}

impl Range {
    const ALL: [Range; 7] = [
        Range::Today,
        Range::Days7,
        Range::Days30,
        Range::Days90,
        Range::ThisMonth,
        Range::LastMonth,
        Range::All,
    ];

    fn label(self) -> &'static str {
        match self {
            Range::Today => "Today",
            Range::Days7 => "Last 7 days",
            Range::Days30 => "Last 30 days",
            Range::Days90 => "Last 90 days",
            Range::ThisMonth => "This month",
            Range::LastMonth => "Last month",
            Range::All => "All saved",
        }
    }

    /// From (inclusive) and to (exclusive), Unix seconds, in local time
    /// for days and months.
    fn bounds(self, now: i64) -> (i64, i64) {
        let day = |n: i64| localtime::local_day_start(now) - n * 86_400;
        match self {
            Range::Today => (day(0), i64::MAX),
            Range::Days7 => (now - 7 * 86_400, i64::MAX),
            Range::Days30 => (now - 30 * 86_400, i64::MAX),
            Range::Days90 => (now - 90 * 86_400, i64::MAX),
            Range::ThisMonth => (localtime::local_month_start(now, 0), i64::MAX),
            Range::LastMonth => (
                localtime::local_month_start(now, 1),
                localtime::local_month_start(now, 0),
            ),
            Range::All => (i64::MIN, i64::MAX),
        }
    }
}

/// What the panel shows, and lists cached between frames.
pub struct ViewState {
    tab: Tab,
    overview_range: Range,
    overview_direction: Direction,
    /// The category the ring was clicked into.
    drill: Option<Category>,
    tx_range: Range,
    tx_direction: Direction,
    /// ref_types switched off in the Types filter.
    hidden_types: BTreeSet<String>,
    search: String,
    lp_total: bool,
    lp_search: String,
    tx_list: Option<TxList>,
    /// The Overview chart's data, built once per data or filter change.
    chart: Option<ChartData>,
    /// The part of the time range the chart is zoomed to.
    chart_zoom: Zoom,
    /// Start system -> jumps to every system, for the nearest LP store.
    jumps: HashMap<i64, HashMap<i64, u32>>,
}

impl Default for ViewState {
    fn default() -> Self {
        ViewState {
            tab: Tab::default(),
            overview_range: Range::default(),
            overview_direction: Direction::Income,
            drill: None,
            tx_range: Range::default(),
            tx_direction: Direction::Both,
            hidden_types: BTreeSet::new(),
            search: String::new(),
            lp_total: false,
            lp_search: String::new(),
            tx_list: None,
            chart: None,
            chart_zoom: None,
            jumps: HashMap::new(),
        }
    }
}

/// The Transactions list, built once per filter or data change.
struct TxList {
    /// Everything that went into it; a different key means rebuild.
    key: String,
    rows: Vec<TxRow>,
    /// Where each row starts, from the top of the list.
    tops: Vec<f32>,
    height: f32,
    entries: usize,
    wallets: usize,
}

enum TxRow {
    /// A day header, with the day's start.
    Day(i64),
    /// An index into `Finance::journal`.
    Entry(usize),
}

/// The panel.
pub fn show(module: &mut Finance, ui: &mut egui::Ui) {
    ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
    tab_bar(module, ui);
    for problem in &module.problems {
        ui.colored_label(YELLOW, small(problem));
    }
    if !crate::auth::is_configured() {
        ui.colored_label(YELLOW, "EVE login isn't set up in this build.");
        return;
    }
    if module.characters.is_empty() {
        ui.label("Log in your characters on the Characters tab, and their wallets show up here.");
        return;
    }
    if !module.loaded {
        ui.label(small("Reading saved wallet data…").color(DIM));
    }
    match module.view.tab {
        Tab::Wallets => wallets_tab(module, ui),
        Tab::Overview => overview_tab(module, ui),
        Tab::Transactions => transactions_tab(module, ui),
        Tab::Loyalty => loyalty_tab(module, ui),
    }
}

fn tab_bar(module: &mut Finance, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        let tab = &mut module.view.tab;
        ui.selectable_value(tab, Tab::Wallets, "Wallets");
        ui.selectable_value(tab, Tab::Overview, "Overview");
        ui.selectable_value(tab, Tab::Transactions, "Transactions");
        ui.selectable_value(tab, Tab::Loyalty, "Loyalty Points");
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let can_refresh = module.can_refresh(now_unix());
            if ui
                .add_enabled(can_refresh, egui::Button::new(small("Refresh")))
                .on_hover_text("Fetch everything from ESI now. ESI may still send its cached copy.")
                .on_disabled_hover_text("Just refreshed. Try again in a minute.")
                .clicked()
            {
                module.refresh_now();
            }
            let now = now_unix();
            let balances = module.balances.values().map(|(_, at)| *at).max();
            let mut parts = Vec::new();
            if let Some(at) = balances {
                parts.push(format!("Balances {}", ago(now - at)));
            }
            if let Some(at) = module.journal_checked {
                parts.push(format!("Journal {}", ago(now - at)));
            }
            ui.label(small(parts.join(" · ")).color(DIM));
        });
    });
}

/// "just now", "4 min ago", "2 h ago".
fn ago(seconds: i64) -> String {
    let minutes = seconds.max(0) / 60;
    if minutes < 1 {
        "just now".to_string()
    } else if minutes < 120 {
        format!("{minutes} min ago")
    } else {
        format!("{} h ago", minutes / 60)
    }
}

/// How many cards of at least `CARD_MIN_WIDTH` fit, and how wide each is.
fn card_columns(available: f32) -> (usize, f32) {
    let count = (((available + GAP) / (CARD_MIN_WIDTH + GAP)).floor() as usize).max(1);
    let width = (available - GAP * (count as f32 - 1.0)) / count as f32;
    (count, width.max(60.0))
}

/// One line of text that ends in "…" if it doesn't fit in `width`.
fn one_line(
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

/// A card frame: returns the click response. `picked` cards are blue.
fn card(ui: &mut egui::Ui, size: Vec2, picked: bool, clickable: bool) -> (Rect, egui::Response) {
    let sense = if clickable {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    let (fill, border) = match (picked, clickable && response.hovered()) {
        (true, _) => (PICKED_FILL, PICKED_BORDER),
        (false, true) => (CARD_HOVER, CARD_BORDER),
        (false, false) => (CARD_FILL, CARD_BORDER),
    };
    ui.painter().rect(
        rect,
        CornerRadius::same(3),
        fill,
        Stroke::new(1.0, border),
        egui::StrokeKind::Inside,
    );
    (rect, response)
}

// ---------------------------------------------------------------- Wallets

fn wallets_tab(module: &mut Finance, ui: &mut egui::Ui) {
    let picked = module.picked();
    let mut tags: Vec<String> = module
        .characters
        .iter()
        .flat_map(|c| c.tags.iter().cloned())
        .collect();
    tags.sort();
    tags.dedup();

    let mut clicked_tag = None;
    ui.horizontal_wrapped(|ui| {
        ui.label(
            small(format!(
                "Pick the wallets Overview and Transactions use · {} of {} picked",
                picked.len(),
                module.characters.len()
            ))
            .color(DIM),
        );
        if !tags.is_empty() {
            ui.label(small("Pick tag").color(DIM));
            for tag in &tags {
                if ui
                    .add(egui::Button::new(small(tag)).corner_radius(8))
                    .on_hover_text("Pick exactly the characters with this tag")
                    .clicked()
                {
                    clicked_tag = Some(tag.clone());
                }
            }
        }
    });
    if let Some(tag) = clicked_tag {
        module.pick_tag(&tag);
    }

    let total: f64 = module
        .characters
        .iter()
        .filter_map(|c| module.balances.get(&c.id).map(|(b, _)| *b))
        .sum();
    let all_picked = picked.len() == module.characters.len();

    // The "All characters" card first, then one per character.
    let mut cards: Vec<(Option<u64>, String, String, Option<String>)> = vec![(
        None,
        "All characters".to_string(),
        format!("{} ISK", short_isk(total)),
        None,
    )];
    for c in &module.characters {
        let value = match module.balances.get(&c.id) {
            Some((balance, _)) => format!("{} ISK", short_isk(*balance)),
            None => "…".to_string(),
        };
        let problem = module.character_problems.get(&c.id).cloned();
        cards.push((Some(c.id), c.name.clone(), value, problem));
    }

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let (columns, width) = card_columns(ui.available_width());
            let mut toggled = None;
            for row in cards.chunks(columns) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = GAP;
                    for (id, name, value, problem) in row {
                        let is_picked = match id {
                            Some(id) => picked.contains(id),
                            None => all_picked,
                        };
                        let (rect, response) =
                            card(ui, Vec2::new(width, WALLET_CARD_HEIGHT), is_picked, true);
                        let painter = ui.painter();
                        let inner = rect.shrink2(Vec2::new(10.0, 6.0));
                        let name = one_line(painter, name, 13.5, BRIGHT, inner.width());
                        painter.galley(inner.min, name, BRIGHT);
                        // The balance stays visible; a problem shows as a
                        // short yellow note (full text on hover).
                        let note = problem.as_deref().map(short_problem);
                        let note = note.map(|note| {
                            painter.layout_no_wrap(
                                note.to_string(),
                                FontId::proportional(11.5),
                                YELLOW,
                            )
                        });
                        let note_width = note.as_ref().map_or(0.0, |g| g.size().x + 8.0);
                        let line = one_line(painter, value, 12.0, TEXT, inner.width() - note_width);
                        painter.galley(Pos2::new(inner.min.x, inner.min.y + 18.0), line, TEXT);
                        if let Some(note) = note {
                            let x = inner.max.x - note.size().x;
                            painter.galley(Pos2::new(x, inner.min.y + 18.5), note, YELLOW);
                        }
                        let response = match problem {
                            Some(problem) => response.on_hover_text(problem),
                            None => response,
                        };
                        if response.clicked() {
                            toggled = Some(*id);
                        }
                    }
                });
                ui.add_space(GAP - ui.spacing().item_spacing.y);
            }
            match toggled {
                Some(Some(id)) => module.toggle_pick(id),
                Some(None) => module.toggle_all(),
                None => {}
            }
        });
}

/// A few words for a wallet card; the full text shows on hover.
fn short_problem(problem: &str) -> &'static str {
    if problem.starts_with("Login expired") {
        "Login expired"
    } else {
        "ESI problem"
    }
}

// --------------------------------------------------------------- Overview

/// The journal entries of picked wallets inside `range`, without the
/// transfers between picked characters.
fn picked_entries<'a>(
    module: &'a Finance,
    picked: &'a BTreeSet<u64>,
    range: Range,
) -> impl Iterator<Item = &'a JournalEntry> + 'a {
    let (from, to) = range.bounds(now_unix());
    module
        .journal
        .iter()
        .filter(move |e| picked.contains(&e.character_id) && e.when >= from && e.when < to)
        .filter(move |e| !e.is_between(picked))
}

fn range_picker(ui: &mut egui::Ui, id: &str, range: &mut Range) -> bool {
    let before = *range;
    egui::ComboBox::from_id_salt(id)
        .selected_text(range.label())
        .show_ui(ui, |ui| {
            for option in Range::ALL {
                ui.selectable_value(range, option, option.label());
            }
        });
    *range != before
}

/// A category color made lighter (`f` > 0) or darker (`f` < 0), for the
/// types inside a category.
fn shade(color: Color32, f: f32) -> Color32 {
    let mix = |c: u8| {
        let c = c as f32;
        let out = if f > 0.0 {
            c + (255.0 - c) * f
        } else {
            c * (1.0 + f)
        };
        out.round().clamp(0.0, 255.0) as u8
    };
    Color32::from_rgb(mix(color.r()), mix(color.g()), mix(color.b()))
}

fn slice_color(slice: &Slice, index: usize, inside: Option<Category>) -> Color32 {
    match (&slice.key, inside) {
        (SliceKey::Category(cat), _) => cat.color(),
        (SliceKey::RefType(_), Some(cat)) => {
            const SHADES: [f32; 6] = [0.0, -0.4, 0.3, -0.6, 0.5, -0.2];
            shade(cat.color(), SHADES[index % SHADES.len()])
        }
        (SliceKey::RefType(_), None) => DIM,
    }
}

fn slice_label(key: &SliceKey) -> String {
    match key {
        SliceKey::Category(cat) => cat.label().to_string(),
        SliceKey::RefType(ref_type) => type_label(ref_type),
    }
}

fn overview_tab(module: &mut Finance, ui: &mut egui::Ui) {
    let picked = module.picked();
    let range = module.view.overview_range;
    let direction = module.view.overview_direction;
    let inside = module.view.drill;

    let mut totals = Totals::default();
    let mut wallets = BTreeSet::new();
    for e in picked_entries(module, &picked, range) {
        totals.add(e.amount);
        wallets.insert(e.character_id);
    }
    let parts = slices(picked_entries(module, &picked, range), direction, inside);
    let sum: f64 = parts.iter().map(|s| s.amount).sum();

    let mut new_range = range;
    let mut new_direction = direction;
    let mut new_drill = inside;

    ui.horizontal(|ui| {
        range_picker(ui, "finance-overview-range", &mut new_range);
    });
    ui.label(
        small(format!(
            "Based on {} journal entries from {} wallets · transfers between them left out",
            totals.entries,
            wallets.len()
        ))
        .color(DIM),
    );

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let wide = ui.available_width() >= 360.0 + SIDE_WIDTH;
            let ring_size = if wide {
                (ui.available_width() - SIDE_WIDTH - 16.0).min(RING_MAX)
            } else {
                ui.available_width().min(RING_MAX)
            }
            .max(160.0);
            let draw =
                |ui: &mut egui::Ui| ring(ui, ring_size, &parts, sum, inside, range, direction);
            let side = |ui: &mut egui::Ui,
                        new_direction: &mut Direction,
                        new_drill: &mut Option<Category>| {
                ui.set_width(SIDE_WIDTH);
                side_panel(ui, &totals, &parts, sum, inside, new_direction, new_drill);
            };
            let clicked = if wide {
                ui.horizontal_top(|ui| {
                    let clicked = draw(ui);
                    ui.add_space(16.0);
                    ui.vertical(|ui| side(ui, &mut new_direction, &mut new_drill));
                    clicked
                })
                .inner
            } else {
                let clicked = draw(ui);
                side(ui, &mut new_direction, &mut new_drill);
                clicked
            };
            if let Some(SliceKey::Category(cat)) = clicked {
                new_drill = Some(cat);
            }
            ui.add_space(10.0);
            chart_section(module, ui, &picked, range, inside);
        });

    let view = &mut module.view;
    if new_range != range {
        view.chart_zoom = None;
    }
    view.overview_range = new_range;
    if new_direction != direction {
        view.overview_direction = new_direction;
        view.drill = None;
    } else {
        view.drill = new_drill;
    }
}

/// The candle chart under the ring: the Balance | Net switch, the candle
/// size, and the chart itself.
fn chart_section(
    module: &mut Finance,
    ui: &mut egui::Ui,
    picked: &BTreeSet<u64>,
    range: Range,
    inside: Option<Category>,
) {
    let mode = module.settings.chart;
    // The range end moves with the clock; counting it in whole minutes
    // keeps the cached data valid for a minute instead of one frame.
    let now = (now_unix() / 60 + 1) * 60;
    let (from, to) = range.bounds(now);
    let to = to.min(now);
    let key = format!(
        "{}|{picked:?}|{from}|{to}|{inside:?}|{mode:?}",
        module.data_version
    );
    if module.view.chart.as_ref().is_none_or(|c| c.key != key) {
        let entries = module
            .journal
            .iter()
            .filter(|e| picked.contains(&e.character_id) && e.when >= from && e.when < to)
            .filter(|e| !e.is_between(picked));
        module.view.chart = Some(ChartData::build(
            key,
            mode,
            inside,
            entries,
            &module.journal,
            picked,
            &module.balances,
            from,
            to,
        ));
    }

    let mut new_mode = mode;
    let mut reset = false;
    // The header comes first on screen, but its numbers come from
    // drawing the chart, so it is drawn into space kept for it here.
    let header_rect = ui
        .allocate_exact_size(Vec2::new(ui.available_width(), 22.0), Sense::hover())
        .0;
    let colors = chart::Colors {
        up: GREEN,
        down: RED,
        text: TEXT,
        dim: DIM,
        grid: ROW_LINE,
        axis: CARD_BORDER,
    };
    let view = &mut module.view;
    let Some(data) = view.chart.as_mut() else {
        return;
    };
    let header = chart::draw(ui, data, &mut view.chart_zoom, &colors);

    ui.scope_builder(egui::UiBuilder::new().max_rect(header_rect), |ui| {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut new_mode, ChartMode::Balance, "Balance");
            ui.selectable_value(&mut new_mode, ChartMode::Net, "Net");
            ui.label(small(format!("Candle: {}", header.step.label)).color(TEXT));
            ui.label(small(format!("{} candles", header.candles)).color(DIM));
            if header.zoomed && ui.small_button("Reset zoom").clicked() {
                reset = true;
            }
            let what = match new_mode {
                ChartMode::Balance => "Combined wallet balance of the picked characters",
                ChartMode::Net => "Income minus expenses since the start of the range",
            };
            ui.label(small(what).color(DIM));
            ui.label(small("Wheel: zoom · drag: move · double-click: reset").color(DIM));
        });
    });

    if reset {
        module.view.chart_zoom = None;
    }
    if new_mode != mode {
        module.settings.chart = new_mode;
        module.save_settings();
    }
}

/// Draws the ring and returns the piece that was clicked.
fn ring(
    ui: &mut egui::Ui,
    size: f32,
    parts: &[Slice],
    sum: f64,
    inside: Option<Category>,
    range: Range,
    direction: Direction,
) -> Option<SliceKey> {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let painter = ui.painter_at(rect);
    let center = rect.center();
    // Room for the hovered piece, which grows by 5 points.
    let outer = size / 2.0 - 8.0;
    let inner = outer * 0.82;

    // Which piece the mouse is over: by its angle from the top, clockwise.
    let mut hovered = None;
    if let Some(pos) = response.hover_pos() {
        let d = pos - center;
        let r = d.length();
        if r >= inner - 4.0 && r <= outer + 6.0 && sum > 0.0 {
            let angle = d.x.atan2(-d.y).rem_euclid(TAU);
            let mut start = 0.0;
            for (i, part) in parts.iter().enumerate() {
                let end = start + (part.amount / sum) as f32 * TAU;
                if angle >= start && angle < end {
                    hovered = Some(i);
                }
                start = end;
            }
        }
    }

    if parts.is_empty() || sum <= 0.0 {
        painter.circle_stroke(
            center,
            (outer + inner) / 2.0,
            Stroke::new(outer - inner, CARD_BORDER),
        );
    } else {
        let gap = if parts.len() > 1 { 0.012 } else { 0.0 };
        let mut start = 0.0;
        for (i, part) in parts.iter().enumerate() {
            let end = start + (part.amount / sum) as f32 * TAU;
            let color = slice_color(part, i, inside);
            let grow = if hovered == Some(i) { 5.0 } else { 0.0 };
            arc(
                &painter,
                center,
                inner,
                outer + grow,
                start + gap / 2.0,
                (end - gap / 2.0).max(start + gap / 2.0 + 0.002),
                color,
            );
            start = end;
        }
    }

    let what = match direction {
        Direction::Expenses => "expenses",
        _ => "income",
    };
    painter.text(
        center - Vec2::new(0.0, 12.0),
        Align2::CENTER_CENTER,
        short_isk(sum),
        FontId::proportional(28.0),
        BRIGHT,
    );
    let mut y = 16.0;
    if let Some(cat) = inside {
        painter.text(
            center + Vec2::new(0.0, y),
            Align2::CENTER_CENTER,
            cat.label(),
            FontId::proportional(14.0),
            TEXT,
        );
        y += 20.0;
    }
    painter.text(
        center + Vec2::new(0.0, y),
        Align2::CENTER_CENTER,
        format!("{} · {what}", range.label()),
        FontId::proportional(12.5),
        DIM,
    );

    let hovered = hovered.and_then(|i| parts.get(i));
    if let Some(part) = hovered {
        response.clone().on_hover_text(format!(
            "{}: {} ISK ({:.1}%)",
            slice_label(&part.key),
            full_isk(part.amount, false),
            part.amount / sum * 100.0
        ));
    }
    if response.clicked() && inside.is_none() {
        return hovered.map(|p| p.key.clone());
    }
    None
}

/// One piece of the ring, from angle `a0` to `a1` (radians, clockwise
/// from the top), as a strip of triangles.
fn arc(painter: &egui::Painter, center: Pos2, r0: f32, r1: f32, a0: f32, a1: f32, color: Color32) {
    let steps = (((a1 - a0) / 0.03).ceil() as usize).max(2);
    let mut mesh = Mesh::default();
    for step in 0..=steps {
        let a = a0 + (a1 - a0) * step as f32 / steps as f32;
        let dir = Vec2::new(a.sin(), -a.cos());
        mesh.colored_vertex(center + dir * r0, color);
        mesh.colored_vertex(center + dir * r1, color);
        if step > 0 {
            let i = (step * 2) as u32;
            mesh.add_triangle(i - 2, i - 1, i);
            mesh.add_triangle(i - 1, i + 1, i);
        }
    }
    painter.add(egui::Shape::mesh(mesh));
}

fn side_panel(
    ui: &mut egui::Ui,
    totals: &Totals,
    parts: &[Slice],
    sum: f64,
    inside: Option<Category>,
    direction: &mut Direction,
    drill: &mut Option<Category>,
) {
    ui.radio_value(direction, Direction::Income, "Income");
    ui.label(
        RichText::new(format!("{} ISK", full_isk(totals.income, true)))
            .size(15.0)
            .color(GREEN),
    );
    ui.radio_value(direction, Direction::Expenses, "Expenses");
    ui.label(
        RichText::new(format!("{} ISK", full_isk(totals.expenses, true)))
            .size(15.0)
            .color(RED),
    );
    ui.label("Net");
    let net = totals.net();
    ui.label(
        RichText::new(format!("{} ISK", full_isk(net, true)))
            .size(15.0)
            .color(if net >= 0.0 { GREEN } else { RED }),
    );
    ui.add_space(6.0);
    if inside.is_some() && ui.button("‹ All categories").clicked() {
        *drill = None;
    }
    for (i, part) in parts.iter().enumerate() {
        let row = ui.horizontal(|ui| {
            let (square, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
            ui.painter()
                .rect_filled(square, 0.0, slice_color(part, i, inside));
            ui.label(small(format!("{:5.1}%", part.amount / sum * 100.0)).monospace());
            ui.label(small(slice_label(&part.key)).color(TEXT));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(small(short_isk(part.amount)).color(DIM));
            });
        });
        if inside.is_none()
            && let SliceKey::Category(cat) = part.key
        {
            let response = row
                .response
                .interact(Sense::click())
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if response.clicked() {
                *drill = Some(cat);
            }
        }
    }
    if inside.is_none() && !parts.is_empty() {
        ui.label(small("Click a piece or a row to see its types.").color(DIM));
    }
    if parts.is_empty() {
        ui.label(small("Nothing in this time range.").color(DIM));
    }
}

// ----------------------------------------------------------- Transactions

fn transactions_tab(module: &mut Finance, ui: &mut egui::Ui) {
    // Every ref_type seen, per category, for the Types filter.
    let mut types: BTreeMap<Category, BTreeSet<&str>> = BTreeMap::new();
    for e in &module.journal {
        types
            .entry(e.category())
            .or_default()
            .insert(e.ref_type.as_str());
    }
    let types: BTreeMap<Category, Vec<String>> = types
        .into_iter()
        .map(|(cat, set)| (cat, set.into_iter().map(str::to_string).collect()))
        .collect();

    let view = &mut module.view;
    ui.horizontal(|ui| {
        let hidden = view.hidden_types.len();
        let label = if hidden == 0 {
            "Types: all".to_string()
        } else {
            format!("Types: {hidden} off")
        };
        let button = ui.add(egui::Button::new(label).selected(hidden > 0));
        egui::Popup::from_toggle_button_response(&button)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| types_filter(ui, &types, &mut view.hidden_types));

        egui::ComboBox::from_id_salt("finance-tx-direction")
            .selected_text(match view.tx_direction {
                Direction::Both => "Income and expenses",
                Direction::Income => "Income",
                Direction::Expenses => "Expenses",
            })
            .show_ui(ui, |ui| {
                let d = &mut view.tx_direction;
                ui.selectable_value(d, Direction::Both, "Income and expenses");
                ui.selectable_value(d, Direction::Income, "Income");
                ui.selectable_value(d, Direction::Expenses, "Expenses");
            });
        range_picker(ui, "finance-tx-range", &mut view.tx_range);
        ui.add(
            egui::TextEdit::singleline(&mut view.search)
                .hint_text("Search names, items, places")
                .desired_width(200.0),
        );
    });

    let key = tx_key(module);
    if module.view.tx_list.as_ref().is_none_or(|l| l.key != key) {
        let list = build_tx_list(module, key);
        module.view.tx_list = Some(list);
    }
    let Some(list) = module.view.tx_list.as_ref() else {
        return;
    };
    ui.label(
        small(format!(
            "Showing {} entries from {} wallets",
            list.entries, list.wallets
        ))
        .color(DIM),
    );
    if list.rows.is_empty() {
        ui.label(small("No entries match. Change the filters or pick more wallets.").color(DIM));
        return;
    }

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show_viewport(ui, |ui, viewport| {
            ui.set_height(list.height);
            let origin = ui.max_rect().min;
            let width = ui.available_width();
            // Only the rows in view are drawn.
            // The last row starting at or above the viewport's top.
            let first = list
                .tops
                .partition_point(|top| *top <= viewport.min.y)
                .saturating_sub(1);
            for i in first..list.rows.len() {
                let top = list.tops[i];
                if top > viewport.max.y {
                    break;
                }
                let next = list.tops.get(i + 1).copied().unwrap_or(list.height);
                let rect =
                    Rect::from_min_size(origin + Vec2::new(0.0, top), Vec2::new(width, next - top));
                match list.rows[i] {
                    TxRow::Day(day) => {
                        ui.painter().text(
                            rect.left_bottom() + Vec2::new(2.0, -4.0),
                            Align2::LEFT_BOTTOM,
                            localtime::date_text(day),
                            FontId::proportional(12.0),
                            DIM,
                        );
                    }
                    TxRow::Entry(index) => {
                        if let Some(entry) = module.journal.get(index) {
                            draw_entry(module, ui, rect, entry);
                        }
                    }
                }
            }
        });
}

/// A text naming every input of the Transactions list.
fn tx_key(module: &Finance) -> String {
    let view = &module.view;
    let (from, _) = view.tx_range.bounds(now_unix());
    // Ranges move with time; whole minutes are close enough.
    format!(
        "{}|{:?}|{}|{:?}|{:?}|{}|{:?}",
        module.data_version,
        view.tx_range,
        from / 60,
        view.tx_direction,
        view.hidden_types,
        view.search.trim().to_lowercase(),
        module.picked()
    )
}

fn build_tx_list(module: &Finance, key: String) -> TxList {
    let view = &module.view;
    let picked = module.picked();
    let (from, to) = view.tx_range.bounds(now_unix());
    let search = view.search.trim().to_lowercase();
    let mut rows = Vec::new();
    let mut tops = Vec::new();
    let mut height = 0.0;
    let mut wallets = BTreeSet::new();
    let mut entries = 0;
    let mut last_day = None;
    for (index, e) in module.journal.iter().enumerate() {
        if !picked.contains(&e.character_id)
            || e.when < from
            || e.when >= to
            || !view.tx_direction.matches(e.amount)
            || view.hidden_types.contains(&e.ref_type)
        {
            continue;
        }
        if !search.is_empty() && !search_text(module, e).contains(&search) {
            continue;
        }
        let day = localtime::local_day_start(e.when);
        if last_day != Some(day) {
            rows.push(TxRow::Day(day));
            tops.push(height);
            height += DAY_ROW;
            last_day = Some(day);
        }
        rows.push(TxRow::Entry(index));
        tops.push(height);
        height += entry_height(module, e);
        entries += 1;
        wallets.insert(e.character_id);
    }
    TxList {
        key,
        rows,
        tops,
        height,
        entries,
        wallets: wallets.len(),
    }
}

/// Everything the search box looks through, in lower case.
fn search_text(module: &Finance, e: &JournalEntry) -> String {
    let mut text = format!(
        "{} {} {}",
        type_label(&e.ref_type),
        e.description,
        e.reason.as_deref().unwrap_or("")
    );
    for id in [e.first_party_id, e.second_party_id].into_iter().flatten() {
        text.push(' ');
        text.push_str(&module.name_of(id));
    }
    if let Some(trade) = module.trades.get(&(e.character_id, e.id)) {
        text.push(' ');
        text.push_str(&module.name_of(trade.type_id));
        text.push(' ');
        text.push_str(&module.name_of(trade.location_id));
    }
    text.to_lowercase()
}

fn entry_height(module: &Finance, e: &JournalEntry) -> f32 {
    let has_trade = module.trades.contains_key(&(e.character_id, e.id));
    let lines = has_trade as u8 + e.reason.is_some() as u8;
    ENTRY_ROW + EXTRA_LINE * lines as f32
}

fn draw_entry(module: &Finance, ui: &egui::Ui, rect: Rect, e: &JournalEntry) {
    let painter = ui.painter();
    if ui.rect_contains_pointer(rect) {
        painter.rect_filled(rect, 0.0, Color32::from_rgb(0x20, 0x20, 0x20));
    }
    painter.hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1.0, ROW_LINE),
    );
    let left = rect.left() + 4.0;
    let right = rect.right() - 6.0;
    let mut y = rect.top() + 5.0;

    // Line 1: amount, type, time ... balance.
    let amount_color = if e.amount >= 0.0 { GREEN } else { RED };
    let amount = painter.layout_no_wrap(
        format!("{} ISK", full_isk(e.amount, true)),
        FontId::proportional(13.5),
        amount_color,
    );
    let amount_width = amount.size().x;
    painter.galley(Pos2::new(left, y), amount, amount_color);
    let balance = e
        .balance
        .map(|b| format!("{} ISK", full_isk(b, false)))
        .unwrap_or_default();
    let balance = painter.layout_no_wrap(balance, FontId::proportional(12.0), TEXT);
    let balance_width = balance.size().x;
    painter.galley(Pos2::new(right - balance_width, y + 1.0), balance, TEXT);
    let kind = format!(
        "{}  ·  {}",
        type_label(&e.ref_type),
        localtime::clock_text(e.when)
    );
    let kind = one_line(
        painter,
        &kind,
        13.0,
        BRIGHT,
        right - balance_width - left - amount_width - 24.0,
    );
    painter.galley(Pos2::new(left + amount_width + 10.0, y + 0.5), kind, BRIGHT);
    y += 19.0;

    // Line 2: who paid whom ... character.
    let me = module
        .characters
        .iter()
        .find(|c| c.id == e.character_id)
        .map(|c| c.name.clone())
        .unwrap_or_default();
    let other = [e.first_party_id, e.second_party_id]
        .into_iter()
        .flatten()
        .find(|id| *id != e.character_id as i64)
        .map(|id| module.name_of(id));
    let who = match other {
        Some(other) if e.amount < 0.0 => format!("{me} paid {other}"),
        Some(other) => format!("{other} paid {me}"),
        None => e.description.clone(),
    };
    let name = painter.layout_no_wrap(me.clone(), FontId::proportional(12.0), DIM);
    let name_width = name.size().x;
    painter.galley(Pos2::new(right - name_width, y), name, DIM);
    let who = one_line(painter, &who, 12.0, TEXT, right - name_width - left - 16.0);
    painter.galley(Pos2::new(left, y), who, TEXT);
    y += EXTRA_LINE;

    // Line 3: the market trade, if there is one.
    if let Some(t) = module.trades.get(&(e.character_id, e.id)) {
        let text = format!(
            "{} × {} at {} ISK each  ·  {}",
            full_isk(t.quantity as f64, false),
            module.name_of(t.type_id),
            full_isk(t.unit_price, false),
            module.name_of(t.location_id)
        );
        let line = one_line(painter, &text, 12.0, DIM, right - left);
        painter.galley(Pos2::new(left, y), line, DIM);
        y += EXTRA_LINE;
    }
    if let Some(reason) = &e.reason {
        let line = one_line(painter, &format!("“{reason}”"), 12.0, DIM, right - left);
        painter.galley(Pos2::new(left, y), line, DIM);
    }
}

/// The Types popup: a box per category and one per type.
fn types_filter(
    ui: &mut egui::Ui,
    types: &BTreeMap<Category, Vec<String>>,
    hidden: &mut BTreeSet<String>,
) {
    ui.set_max_height(420.0);
    ui.horizontal(|ui| {
        if ui.small_button("All").clicked() {
            hidden.clear();
        }
        if ui.small_button("None").clicked() {
            *hidden = types.values().flatten().cloned().collect();
        }
    });
    if types.is_empty() {
        ui.label(small("No entries yet.").color(DIM));
    }
    egui::ScrollArea::vertical().show(ui, |ui| {
        for (cat, list) in types {
            let mut all_on = list.iter().all(|t| !hidden.contains(t));
            ui.horizontal(|ui| {
                let (square, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
                ui.painter().rect_filled(square, 0.0, cat.color());
                if ui.checkbox(&mut all_on, cat.label()).changed() {
                    for t in list {
                        if all_on {
                            hidden.remove(t);
                        } else {
                            hidden.insert(t.clone());
                        }
                    }
                }
            });
            ui.indent(cat.label(), |ui| {
                for t in list {
                    let mut on = !hidden.contains(t);
                    if ui.checkbox(&mut on, small(type_label(t))).changed() {
                        if on {
                            hidden.remove(t);
                        } else {
                            hidden.insert(t.clone());
                        }
                    }
                }
            });
        }
    });
}

// ---------------------------------------------------------- Loyalty Points

/// One LP card: corporation, points, and the nearest store (or who holds
/// the points, on the Total view).
struct LpCard {
    corporation: String,
    points: i64,
    detail: String,
    /// "3 jumps", shown next to the points so it never gets cut off.
    distance: Option<String>,
}

fn loyalty_tab(module: &mut Finance, ui: &mut egui::Ui) {
    ui.label(
        small("Loyalty points, and the nearest station with that corporation's LP store.")
            .color(DIM),
    );
    ui.horizontal(|ui| {
        let view = &mut module.view;
        ui.selectable_value(&mut view.lp_total, false, "By character");
        ui.selectable_value(&mut view.lp_total, true, "Total");
        ui.add(
            egui::TextEdit::singleline(&mut view.lp_search)
                .hint_text("Search corporations or characters")
                .desired_width(220.0),
        );
    });
    let search = module.view.lp_search.trim().to_lowercase();
    let matches = |text: &str| search.is_empty() || text.to_lowercase().contains(&search);

    if module.loyalty.values().all(|points| points.is_empty()) {
        ui.label(small("No loyalty points yet (or not fetched yet).").color(DIM));
        return;
    }

    if module.view.lp_total {
        let mut totals: BTreeMap<i64, (i64, usize)> = BTreeMap::new();
        for c in &module.characters {
            for p in module.loyalty.get(&c.id).into_iter().flatten() {
                let slot = totals.entry(p.corporation_id).or_default();
                slot.0 += p.loyalty_points;
                slot.1 += 1;
            }
        }
        let mut cards: Vec<LpCard> = totals
            .into_iter()
            .map(|(corp, (points, holders))| LpCard {
                corporation: corporation_name(module, corp),
                points,
                detail: format!(
                    "held by {holders} character{}",
                    if holders == 1 { "" } else { "s" }
                ),
                distance: None,
            })
            .filter(|card| matches(&card.corporation))
            .collect();
        cards.sort_by_key(|card| std::cmp::Reverse(card.points));
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| lp_cards(ui, &cards));
        return;
    }

    // By character: work out jump distances first (cached per start
    // system), so drawing below only reads.
    let starts: Vec<i64> = module
        .characters
        .iter()
        .filter_map(|c| module.location(c))
        .collect();
    // Only the systems the characters are in now: each entry is a whole
    // BFS result (100+ KB), so old ones must not pile up.
    module.view.jumps.retain(|start, _| starts.contains(start));
    if let Some(data) = module.static_data.clone() {
        for start in starts {
            module
                .view
                .jumps
                .entry(start)
                .or_insert_with(|| data.universe.jumps_within(&[start], MAX_LP_JUMPS));
        }
    }

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for c in &module.characters {
                let Some(points) = module.loyalty.get(&c.id) else {
                    continue;
                };
                let name_matches = matches(&c.name);
                let location = module.location(c);
                let cards: Vec<LpCard> = points
                    .iter()
                    .map(|p| {
                        let (detail, distance) = nearest_store(module, p.corporation_id, location);
                        LpCard {
                            corporation: corporation_name(module, p.corporation_id),
                            points: p.loyalty_points,
                            detail,
                            distance,
                        }
                    })
                    .filter(|card| name_matches || matches(&card.corporation))
                    .collect();
                if cards.is_empty() {
                    continue;
                }
                let total: i64 = cards.iter().map(|card| card.points).sum();
                let place = location
                    .and_then(|id| module.static_data.as_deref()?.universe.system(id))
                    .map(|s| format!("in {} · ", s.name))
                    .unwrap_or_default();
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&c.name).size(15.0).color(BRIGHT));
                    ui.label(
                        small(format!(
                            "{place}{} LP in {} corporations",
                            full_isk(total as f64, false),
                            cards.len()
                        ))
                        .color(DIM),
                    );
                });
                lp_cards(ui, &cards);
            }
        });
}

fn corporation_name(module: &Finance, corporation_id: i64) -> String {
    module.name_of(corporation_id)
}

/// The nearest station with the corporation's LP store, and how far it
/// is: ("Jita IV - Moon 4 - Caldari Navy Assembly Plant", "3 jumps").
fn nearest_store(
    module: &Finance,
    corporation_id: i64,
    location: Option<i64>,
) -> (String, Option<String>) {
    let Some(data) = module.static_data.as_deref() else {
        return ("No static data, so stores are unknown".to_string(), None);
    };
    let stations = data.lp_stations(corporation_id);
    if stations.is_empty() {
        return ("No LP store in known space".to_string(), None);
    }
    let Some(jumps) = location.and_then(|start| module.view.jumps.get(&start)) else {
        return ("Location unknown".to_string(), None);
    };
    let nearest = stations
        .iter()
        .filter_map(|s| jumps.get(&s.system_id).map(|j| (*j, s)))
        .min_by_key(|(j, s)| (*j, s.station_id));
    match nearest {
        Some((j, station)) => {
            let distance = match j {
                0 => "here".to_string(),
                1 => "1 jump".to_string(),
                j => format!("{j} jumps"),
            };
            (station.name.clone(), Some(distance))
        }
        None => ("No store reachable by gates".to_string(), None),
    }
}

fn lp_cards(ui: &mut egui::Ui, cards: &[LpCard]) {
    let (columns, width) = card_columns(ui.available_width());
    for row in cards.chunks(columns) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = GAP;
            for c in row {
                let (rect, response) = card(ui, Vec2::new(width, LP_CARD_HEIGHT), false, false);
                let painter = ui.painter();
                let inner = rect.shrink2(Vec2::new(10.0, 6.0));
                let name = one_line(painter, &c.corporation, 13.5, BRIGHT, inner.width());
                painter.galley(inner.min, name, BRIGHT);
                let points = format!("{} LP", full_isk(c.points as f64, false));
                painter.text(
                    inner.min + Vec2::new(0.0, 18.0),
                    Align2::LEFT_TOP,
                    points,
                    FontId::proportional(13.0),
                    TEXT,
                );
                if let Some(distance) = &c.distance {
                    painter.text(
                        Pos2::new(inner.max.x, inner.min.y + 19.0),
                        Align2::RIGHT_TOP,
                        distance,
                        FontId::proportional(12.0),
                        TEXT,
                    );
                }
                let detail = one_line(painter, &c.detail, 11.5, DIM, inner.width());
                let elided = detail.elided;
                painter.galley(inner.min + Vec2::new(0.0, 36.0), detail, DIM);
                if elided {
                    response.on_hover_text(&c.detail);
                }
            }
        });
        ui.add_space(GAP - ui.spacing().item_spacing.y);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn card_columns_fill_the_width() {
        assert_eq!(card_columns(100.0).0, 1);
        let (count, width) = card_columns(700.0);
        assert_eq!(count, 3);
        assert!((width * 3.0 + GAP * 2.0 - 700.0).abs() < 0.01);
    }

    #[test]
    fn ranges() {
        // Linux tests: local time is UTC.
        let now = 1_794_492_238; // Thu 12 Nov 2026 14:03:58
        assert_eq!(Range::Today.bounds(now).0, 1_794_441_600);
        let (from, to) = Range::LastMonth.bounds(now);
        assert_eq!(localtime::date_text(from), "Thu 1 Oct 2026");
        assert_eq!(localtime::date_text(to), "Sun 1 Nov 2026");
        assert_eq!(Range::Days7.bounds(now).0, now - 7 * 86_400);
    }

    #[test]
    fn shades_stay_in_range() {
        let c = Color32::from_rgb(200, 100, 0);
        assert_eq!(shade(c, 0.0), c);
        assert_eq!(shade(c, -1.0), Color32::from_rgb(0, 0, 0));
        assert_eq!(shade(c, 1.0), Color32::from_rgb(255, 255, 255));
    }

    #[test]
    fn ago_texts() {
        assert_eq!(ago(30), "just now");
        assert_eq!(ago(4 * 60), "4 min ago");
        assert_eq!(ago(3 * 3600), "3 h ago");
    }
}
