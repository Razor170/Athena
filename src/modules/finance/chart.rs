//! The candle chart on the Overview tab, from the mockup Kevin approved
//! (<https://claude.ai/artifact/7ELmchPGonjWxQ5FdzZocv>).
//!
//! Each candle is one time slot (5 minutes up to a week, picked so a
//! candle stays at least `MIN_CANDLE_PX` wide). Its body runs from the
//! value when the slot starts (open) to the value when it ends (close),
//! its wick from the lowest to the highest value inside the slot. The
//! value is either the combined wallet balance of the picked characters
//! or the running net since the start of the time range ([`ChartMode`]).
//! Bars under the candles show each slot's income and expenses.
//!
//! The work is split so that drawing stays cheap:
//! - [`Series`] and the flows are built once per data, filter or range
//!   change ([`ChartData`], cached by the Overview tab).
//! - The candles are rebuilt only when the zoom or the chart width
//!   changes ([`ChartData::candles_for`]).
//! - Drawing a frame is then a few hundred rectangles.

use std::collections::{BTreeSet, HashMap};

use eframe::egui::{
    self, Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Vec2,
};

use super::categories::Category;
use super::config::ChartMode;
use super::data::{JournalEntry, full_isk, short_isk};
use crate::localtime;

const DAY: i64 = 86_400;

/// Candles are at least this many points wide; the candle size is the
/// smallest step that keeps them so.
const MIN_CANDLE_PX: f32 = 7.0;

/// Zooming in stops at this many seconds on screen.
const MIN_SPAN: i64 = 2 * 3600;

pub const CHART_HEIGHT: f32 = 320.0;
/// Room on the right for the value labels.
const AXIS_WIDTH: f32 = 62.0;

/// One candle size the chart can use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    pub secs: i64,
    pub label: &'static str,
}

pub const STEPS: [Step; 11] = [
    Step {
        secs: 300,
        label: "5 min",
    },
    Step {
        secs: 900,
        label: "15 min",
    },
    Step {
        secs: 1800,
        label: "30 min",
    },
    Step {
        secs: 3600,
        label: "1 h",
    },
    Step {
        secs: 2 * 3600,
        label: "2 h",
    },
    Step {
        secs: 3 * 3600,
        label: "3 h",
    },
    Step {
        secs: 6 * 3600,
        label: "6 h",
    },
    Step {
        secs: 12 * 3600,
        label: "12 h",
    },
    Step {
        secs: DAY,
        label: "1 day",
    },
    Step {
        secs: 2 * DAY,
        label: "2 days",
    },
    Step {
        secs: 7 * DAY,
        label: "1 week",
    },
];

/// The smallest step that keeps candles `MIN_CANDLE_PX` wide when `span`
/// seconds fill `width` points. Very long spans get weeks anyway.
pub fn pick_step(span: i64, width: f32) -> Step {
    for step in STEPS {
        let candles = span as f32 / step.secs as f32;
        if width / candles >= MIN_CANDLE_PX {
            return step;
        }
    }
    STEPS[STEPS.len() - 1]
}

/// The start of the slot that `t` falls in. Slots follow local time:
/// hours count from local midnight, days start at local midnight, and
/// weeks on Monday.
pub fn slot_start(t: i64, step: Step) -> i64 {
    let day_start = localtime::local_day_start(t);
    if step.secs < DAY {
        return day_start + (t - day_start) / step.secs * step.secs;
    }
    let days_in_step = step.secs / DAY;
    let local_day = localtime::to_local(t).div_euclid(DAY);
    let back = if days_in_step == 7 {
        // 1970-01-01 was a Thursday, so day 0 is 3 days after a Monday.
        (local_day + 3).rem_euclid(7)
    } else {
        local_day.rem_euclid(days_in_step)
    };
    // Midday of the wanted day, then its midnight: safe across summer
    // time changes, when a day has 23 or 25 hours.
    localtime::local_day_start(day_start - back * DAY + DAY / 2)
}

/// The start of the slot after the one starting at `start`.
pub fn next_slot(start: i64, step: Step) -> i64 {
    if step.secs < DAY {
        let next_midnight = localtime::local_day_start(start + DAY + DAY / 2);
        // A short day (summer time) ends its last slot early.
        return (start + step.secs).min(next_midnight);
    }
    localtime::local_day_start(start + step.secs + DAY / 2)
}

/// A value that changes over time: `start` until the first point, then
/// each point's value from its time on. Points are oldest first.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Series {
    pub start: f64,
    pub points: Vec<(i64, f64)>,
}

impl Series {
    /// The combined balance of the `picked` wallets over all saved time.
    /// `journal` is newest first, as the module keeps it. ESI gives the
    /// balance after each entry, so a gap in the saved history (Athena
    /// not run for over 30 days) shows as a jump, not a wrong level.
    /// Wallets without saved entries count with their current balance.
    pub fn balance(
        journal: &[JournalEntry],
        picked: &BTreeSet<u64>,
        current: &HashMap<u64, (f64, i64)>,
    ) -> Series {
        // Each wallet's balance before its oldest saved entry.
        let mut before: HashMap<u64, f64> = HashMap::new();
        let mut sum_of_amounts: HashMap<u64, f64> = HashMap::new();
        for e in journal.iter().filter(|e| picked.contains(&e.character_id)) {
            *sum_of_amounts.entry(e.character_id).or_default() += e.amount;
            // Newest first, so the last one written is the oldest entry.
            if let Some(balance) = e.balance {
                before.insert(e.character_id, balance - e.amount);
            }
        }
        let mut wallets: HashMap<u64, f64> = HashMap::new();
        for &id in picked {
            let now = current.get(&id).map(|(b, _)| *b).unwrap_or(0.0);
            let start = match (before.get(&id), sum_of_amounts.get(&id)) {
                (Some(b), _) => *b,
                // No balance in any entry: work back from today's.
                (None, Some(sum)) => now - sum,
                (None, None) => now,
            };
            wallets.insert(id, start);
        }
        let mut total: f64 = wallets.values().sum();
        let start = total;
        let mut points = Vec::new();
        for e in journal
            .iter()
            .rev()
            .filter(|e| picked.contains(&e.character_id))
        {
            // `entry` gives a mutable reference to the wallet's value.
            let wallet = wallets.entry(e.character_id).or_default();
            let new = e.balance.unwrap_or(*wallet + e.amount);
            total += new - *wallet;
            *wallet = new;
            points.push((e.when, total));
        }
        Series { start, points }
    }

    /// The running sum of `flows` (oldest first), from 0.
    pub fn net(flows: &[(i64, f64)]) -> Series {
        let mut total = 0.0;
        let points = flows
            .iter()
            .map(|&(when, amount)| {
                total += amount;
                (when, total)
            })
            .collect();
        Series { start: 0.0, points }
    }

    /// The value just before `t`.
    fn before(&self, t: i64) -> f64 {
        let i = self.points.partition_point(|p| p.0 < t);
        if i == 0 {
            self.start
        } else {
            self.points[i - 1].1
        }
    }
}

/// One time slot of the chart.
#[derive(Debug, Clone, PartialEq)]
pub struct Candle {
    /// The slot, cut to the visible time.
    pub from: i64,
    pub to: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub income: f64,
    /// Zero or less.
    pub expenses: f64,
    /// Entries that went into income and expenses.
    pub entries: usize,
}

/// The candles of `series` between `from` and `to`, with the income and
/// expenses from `flows` (oldest first).
pub fn candles(
    series: &Series,
    flows: &[(i64, f64)],
    from: i64,
    to: i64,
    step: Step,
) -> Vec<Candle> {
    let mut out = Vec::new();
    let mut value = series.before(from);
    let mut point = series.points.partition_point(|p| p.0 < from);
    let mut flow = flows.partition_point(|f| f.0 < from);
    let mut start = slot_start(from, step);
    while start < to {
        let end = next_slot(start, step).min(to);
        let mut candle = Candle {
            from: start.max(from),
            to: end,
            open: value,
            high: value,
            low: value,
            close: value,
            income: 0.0,
            expenses: 0.0,
            entries: 0,
        };
        while let Some(&(when, v)) = series.points.get(point)
            && when < end
        {
            value = v;
            candle.high = candle.high.max(v);
            candle.low = candle.low.min(v);
            point += 1;
        }
        candle.close = value;
        while let Some(&(when, amount)) = flows.get(flow)
            && when < end
        {
            if amount >= 0.0 {
                candle.income += amount;
            } else {
                candle.expenses += amount;
            }
            candle.entries += 1;
            flow += 1;
        }
        out.push(candle);
        // Guard against a step that doesn't move (it always should).
        start = if end > start { end } else { start + step.secs };
    }
    out
}

/// What the chart needs, built once per data, filter or range change.
pub struct ChartData {
    /// Everything that went into it; a different key means rebuild.
    pub key: String,
    pub series: Series,
    /// Income and expenses, oldest first, without transfers between
    /// picked characters, only the clicked category.
    pub flows: Vec<(i64, f64)>,
    /// The whole time range, as Unix seconds.
    pub from: i64,
    pub to: i64,
    /// The candles last built, and what for.
    candles_key: Option<(i64, i64, i64)>,
    candles: Vec<Candle>,
}

impl ChartData {
    /// `range_from` may be `i64::MIN` ("All saved"): the chart then starts
    /// at the oldest entry. `entries` are the Overview's entries in the
    /// range (no transfers between picked characters), newest first.
    #[allow(clippy::too_many_arguments)]
    pub fn build<'a>(
        key: String,
        mode: ChartMode,
        drill: Option<Category>,
        entries: impl Iterator<Item = &'a JournalEntry>,
        journal: &[JournalEntry],
        picked: &BTreeSet<u64>,
        balances: &HashMap<u64, (f64, i64)>,
        range_from: i64,
        to: i64,
    ) -> ChartData {
        let mut flows: Vec<(i64, f64)> = entries
            .filter(|e| drill.is_none_or(|cat| e.category() == cat))
            .map(|e| (e.when, e.amount))
            .collect();
        flows.reverse();
        let series = match mode {
            ChartMode::Balance => Series::balance(journal, picked, balances),
            ChartMode::Net => Series::net(&flows),
        };
        let from = if range_from == i64::MIN {
            let oldest = journal
                .iter()
                .rev()
                .find(|e| picked.contains(&e.character_id))
                .map(|e| e.when);
            oldest.unwrap_or(to - DAY).min(to - MIN_SPAN)
        } else {
            range_from
        };
        ChartData {
            key,
            series,
            flows,
            from,
            to,
            candles_key: None,
            candles: Vec::new(),
        }
    }

    /// The candles for the visible part, rebuilt only when it changed.
    pub fn candles_for(&mut self, from: i64, to: i64, step: Step) -> &[Candle] {
        let key = (from, to, step.secs);
        if self.candles_key != Some(key) {
            self.candles = candles(&self.series, &self.flows, from, to, step);
            self.candles_key = Some(key);
        }
        &self.candles
    }
}

/// Zoom of the chart: the visible part of the range, or `None` for all.
pub type Zoom = Option<(i64, i64)>;

/// What the chart line above the plot shows.
pub struct Header {
    pub step: Step,
    pub candles: usize,
    pub zoomed: bool,
}

/// Draws the chart and handles zoom (wheel), move (drag) and reset
/// (double-click). Returns what the header line shows.
pub fn draw(ui: &mut egui::Ui, data: &mut ChartData, zoom: &mut Zoom, colors: &Colors) -> Header {
    let width = ui.available_width().max(200.0);
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(width, CHART_HEIGHT), Sense::click_and_drag());
    let plot_left = rect.left() + 4.0;
    let plot_right = rect.right() - AXIS_WIDTH;
    let plot_width = (plot_right - plot_left).max(1.0);

    // The visible time, kept inside the range.
    let (mut from, mut to) = zoom.unwrap_or((data.from, data.to));
    clamp_view(&mut from, &mut to, data.from, data.to);

    if response.double_clicked() {
        (from, to) = (data.from, data.to);
    } else if response.dragged() {
        let shift = (f64::from(-response.drag_delta().x / plot_width) * (to - from) as f64) as i64;
        let (mut a, mut b) = (from + shift, to + shift);
        clamp_view(&mut a, &mut b, data.from, data.to);
        (from, to) = (a, b);
    }
    if let Some(pointer) = response.hover_pos() {
        // Take the wheel for the chart, so the page around it doesn't
        // scroll at the same time (the ScrollArea reads it after us).
        let scroll = ui.input_mut(|i| {
            let delta = i.smooth_scroll_delta.y + i.smooth_scroll_delta.x;
            i.smooth_scroll_delta = Vec2::ZERO;
            delta
        });
        if scroll != 0.0 {
            let f = f64::from(((pointer.x - plot_left) / plot_width).clamp(0.0, 1.0));
            let factor = f64::from(-scroll / 300.0).exp();
            (from, to) = zoom_view(from, to, f, factor, data.from, data.to);
        }
    }
    let zoomed = from > data.from || to < data.to;
    *zoom = zoomed.then_some((from, to));

    let step = pick_step(to - from, plot_width);
    let candles = data.candles_for(from, to, step);
    paint(ui, rect, &response, candles, from, to, step, colors);
    Header {
        step,
        candles: candles.len(),
        zoomed,
    }
}

/// Zooms `from..to` by `factor` (below 1: in) around the point `f` of the
/// way across, kept inside `min..max`. In f64 and relative to `from`: an
/// f32 holds today's Unix seconds only to about two minutes, which made
/// zoomed views jump.
fn zoom_view(from: i64, to: i64, f: f64, factor: f64, min: i64, max: i64) -> (i64, i64) {
    let span = (to - from) as f64;
    let new_span = (span * factor).clamp(MIN_SPAN as f64, (max - min) as f64);
    let mut a = from + (f * (span - new_span)).round() as i64;
    let mut b = a + new_span.round() as i64;
    clamp_view(&mut a, &mut b, min, max);
    (a, b)
}

/// Keeps `from..to` inside `min..max`, moving it rather than shrinking it.
fn clamp_view(from: &mut i64, to: &mut i64, min: i64, max: i64) {
    let span = (*to - *from).clamp(MIN_SPAN.min(max - min), max - min);
    if *from < min {
        *from = min;
    }
    *to = *from + span;
    if *to > max {
        *to = max;
        *from = max - span;
    }
}

/// The colours the chart uses, from the panel's palette.
pub struct Colors {
    pub up: Color32,
    pub down: Color32,
    pub text: Color32,
    pub dim: Color32,
    pub grid: Color32,
    pub axis: Color32,
}

#[allow(clippy::too_many_arguments)]
fn paint(
    ui: &egui::Ui,
    rect: Rect,
    response: &egui::Response,
    candles: &[Candle],
    from: i64,
    to: i64,
    step: Step,
    colors: &Colors,
) {
    let painter = ui.painter_at(rect);
    let left = rect.left() + 4.0;
    let right = rect.right() - AXIS_WIDTH;
    let top = rect.top() + 8.0;
    let volume_height = (CHART_HEIGHT * 0.2).round();
    let price_bottom = rect.bottom() - volume_height - 30.0;
    let volume_top = price_bottom + 8.0;
    let volume_bottom = rect.bottom() - 20.0;
    let volume_mid = (volume_top + volume_bottom) / 2.0;
    let font = FontId::proportional(11.0);

    let span = (to - from).max(1) as f32;
    let x_of = |t: i64| left + (t - from) as f32 / span * (right - left);

    let mut low = f64::INFINITY;
    let mut high = f64::NEG_INFINITY;
    let mut volume_max: f64 = 1.0;
    for c in candles {
        low = low.min(c.low);
        high = high.max(c.high);
        volume_max = volume_max.max(c.income).max(-c.expenses);
    }
    if !low.is_finite() {
        (low, high) = (0.0, 1.0);
    }
    if high - low < 1.0 {
        (low, high) = (low - 1.0, high + 1.0);
    }
    let pad = (high - low) * 0.08;
    let (low, high) = (low - pad, high + pad);
    let y_of = |v: f64| top + ((high - v) / (high - low)) as f32 * (price_bottom - top);

    // Value grid and labels.
    let grid_step = nice_step(high - low, 5.0);
    let mut v = (low / grid_step).ceil() * grid_step;
    while v <= high {
        let y = y_of(v);
        let is_zero = v.abs() < grid_step / 2.0;
        painter.hline(
            left..=right,
            y,
            Stroke::new(1.0, if is_zero { colors.axis } else { colors.grid }),
        );
        let label = if is_zero {
            "0".to_string()
        } else {
            axis_isk(v, grid_step)
        };
        painter.text(
            Pos2::new(right + 6.0, y),
            Align2::LEFT_CENTER,
            label,
            font.clone(),
            colors.dim,
        );
        v += grid_step;
    }

    // Time labels, about every 90 points.
    if !candles.is_empty() {
        let candle_width = (right - left) / candles.len() as f32;
        let every = ((90.0 / candle_width).ceil() as usize).max(1);
        let mut last_day = None;
        for c in candles.iter().step_by(every) {
            let x = x_of(c.from);
            if x < left + 20.0 || x > right - 20.0 {
                continue;
            }
            painter.vline(x, top..=volume_bottom, Stroke::new(1.0, colors.grid));
            let day = localtime::to_local(c.from).div_euclid(DAY);
            let label = if step.secs >= DAY {
                day_month(c.from)
            } else if last_day != Some(day) {
                format!("{} {}", day_month(c.from), hour_minute(c.from))
            } else {
                hour_minute(c.from)
            };
            last_day = Some(day);
            painter.text(
                Pos2::new(x, rect.bottom() - 3.0),
                Align2::CENTER_BOTTOM,
                label,
                font.clone(),
                colors.dim,
            );
        }
    }

    // Income/expense bars: zero line and scale.
    painter.hline(left..=right, volume_mid, Stroke::new(1.0, colors.axis));
    for (y, label) in [
        (volume_top + 4.0, format!("+{}", short_isk(volume_max))),
        (volume_bottom - 4.0, format!("-{}", short_isk(volume_max))),
    ] {
        painter.text(
            Pos2::new(right + 6.0, y),
            Align2::LEFT_CENTER,
            label,
            font.clone(),
            colors.dim,
        );
    }

    let hovered = response.hover_pos().and_then(|pointer| {
        if pointer.x > right {
            return None;
        }
        candles
            .iter()
            .position(|c| pointer.x >= x_of(c.from) && pointer.x < x_of(c.to))
    });
    if let Some(i) = hovered {
        let c = &candles[i];
        painter.rect_filled(
            Rect::from_x_y_ranges(x_of(c.from)..=x_of(c.to), top..=volume_bottom),
            0.0,
            Color32::from_white_alpha(12),
        );
    }

    let half_bar = (volume_bottom - volume_top) / 2.0;
    for c in candles {
        let x0 = x_of(c.from);
        let x1 = x_of(c.to);
        let w = ((x1 - x0) * 0.7).max(1.0);
        let cx = (x0 + x1) / 2.0;
        let color = if c.close >= c.open {
            colors.up
        } else {
            colors.down
        };
        if c.high > c.low || c.entries > 0 {
            painter.vline(cx, y_of(c.high)..=y_of(c.low), Stroke::new(1.0, color));
            let (y_open, y_close) = (y_of(c.open), y_of(c.close));
            let body = Rect::from_x_y_ranges(
                (cx - w / 2.0)..=(cx + w / 2.0),
                y_open.min(y_close)..=(y_open.max(y_close).max(y_open.min(y_close) + 1.0)),
            );
            painter.rect_filled(body, 0.0, color);
        } else {
            // Nothing happened: a flat dash at the value.
            painter.hline(
                (cx - w / 2.0)..=(cx + w / 2.0),
                y_of(c.close),
                Stroke::new(1.0, colors.axis),
            );
        }
        if c.income > 0.0 {
            let h = (c.income / volume_max) as f32 * half_bar;
            painter.rect_filled(
                Rect::from_x_y_ranges(
                    (cx - w / 2.0)..=(cx + w / 2.0),
                    (volume_mid - h)..=volume_mid,
                ),
                0.0,
                colors.up.gamma_multiply(0.55),
            );
        }
        if c.expenses < 0.0 {
            let h = (-c.expenses / volume_max) as f32 * half_bar;
            painter.rect_filled(
                Rect::from_x_y_ranges(
                    (cx - w / 2.0)..=(cx + w / 2.0),
                    volume_mid..=(volume_mid + h),
                ),
                0.0,
                colors.down.gamma_multiply(0.55),
            );
        }
    }
    painter.rect_stroke(rect, 0.0, Stroke::new(1.0, colors.grid), StrokeKind::Inside);

    if let Some(i) = hovered {
        let c = candles[i].clone();
        response.clone().on_hover_ui_at_pointer(|ui| {
            ui.label(RichText::new(slot_text(&c, step)).color(colors.text));
            ui.label(format!(
                "Open {}   Close {}",
                full_isk(c.open, false),
                full_isk(c.close, false)
            ));
            ui.label(
                RichText::new(format!(
                    "High {}   Low {}",
                    full_isk(c.high, false),
                    full_isk(c.low, false)
                ))
                .color(colors.dim),
            );
            ui.label(
                RichText::new(format!("Income {}", full_isk(c.income, true))).color(colors.up),
            );
            ui.label(
                RichText::new(format!("Expenses {}", full_isk(c.expenses, true)))
                    .color(colors.down),
            );
            let net = c.income + c.expenses;
            ui.label(
                RichText::new(format!(
                    "Net {} · {} entries",
                    full_isk(net, true),
                    c.entries
                ))
                .color(if net >= 0.0 { colors.up } else { colors.down }),
            );
        });
    }
}

/// A round grid step so about `lines` lines fit into `range`.
fn nice_step(range: f64, lines: f64) -> f64 {
    let raw = (range / lines).max(f64::MIN_POSITIVE);
    let power = 10f64.powf(raw.log10().floor());
    for m in [1.0, 2.0, 2.5, 5.0, 10.0] {
        if m * power >= raw {
            return m * power;
        }
    }
    10.0 * power
}

/// A value label with just enough decimals that neighbouring grid lines
/// never read the same: "150 b", "155.2 b", "-373 m".
pub fn axis_isk(value: f64, grid_step: f64) -> String {
    let (unit_size, unit) = if value.abs() >= 1e9 {
        (1e9, " b")
    } else if value.abs() >= 1e6 {
        (1e6, " m")
    } else {
        (1e3, " k")
    };
    let decimals = (-(grid_step / unit_size).log10() - 1e-9)
        .ceil()
        .clamp(0.0, 3.0) as usize;
    format!("{:.*}{unit}", decimals, value / unit_size)
}

/// "12 Nov" in local time.
fn day_month(t: i64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let (_, month, day) = localtime::civil_from_days(localtime::to_local(t).div_euclid(DAY));
    format!("{day} {}", MONTHS[(month - 1) as usize])
}

/// "14:00" in local time.
fn hour_minute(t: i64) -> String {
    let secs = localtime::to_local(t).rem_euclid(DAY);
    format!("{:02}:{:02}", secs / 3600, secs % 3600 / 60)
}

/// The tooltip's first line: "12 Nov 14:00 – 15:00", "12 Nov", "9 – 15 Nov".
fn slot_text(c: &Candle, step: Step) -> String {
    if step.secs < DAY {
        format!(
            "{} {} – {}",
            day_month(c.from),
            hour_minute(c.from),
            hour_minute(c.to)
        )
    } else if step.secs == DAY {
        day_month(c.from)
    } else {
        format!("{} – {}", day_month(c.from), day_month(c.to - 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::finance::data::tests::entry;

    // Tests run on Linux, where local time is UTC.
    const MONDAY: i64 = 1_794_182_400; // 2026-11-09 00:00 UTC, a Monday

    #[test]
    fn steps_follow_width() {
        // 90 days on 900 points: 1 day candles are 10 points wide.
        assert_eq!(pick_step(90 * DAY, 900.0).label, "1 day");
        // 30 days: 6 h candles are 7.5 points wide.
        assert_eq!(pick_step(30 * DAY, 900.0).label, "6 h");
        assert_eq!(pick_step(7 * DAY, 900.0).label, "2 h");
        assert_eq!(pick_step(15 * 3600, 900.0).label, "15 min");
        assert_eq!(pick_step(2 * 3600, 900.0).label, "5 min");
        assert_eq!(pick_step(3000 * DAY, 900.0).label, "1 week");
    }

    #[test]
    fn slots_line_up_with_local_time() {
        let t = MONDAY + 2 * DAY + 14 * 3600 + 1234; // Wednesday 14:20:34
        let hour = Step {
            secs: 3600,
            label: "",
        };
        assert_eq!(slot_start(t, hour), MONDAY + 2 * DAY + 14 * 3600);
        assert_eq!(
            next_slot(slot_start(t, hour), hour),
            MONDAY + 2 * DAY + 15 * 3600
        );
        assert_eq!(slot_start(t, STEPS[6]), MONDAY + 2 * DAY + 12 * 3600);
        assert_eq!(slot_start(t, STEPS[8]), MONDAY + 2 * DAY);
        assert_eq!(slot_start(t, STEPS[10]), MONDAY);
        assert_eq!(next_slot(MONDAY, STEPS[10]), MONDAY + 7 * DAY);
        // Two-day slots start on even days since 1970.
        let two = slot_start(t, STEPS[9]);
        assert_eq!(two.div_euclid(DAY) % 2, 0);
        assert!(two <= t && t < next_slot(two, STEPS[9]));
    }

    fn with_balance(character: u64, id: i64, when: i64, amount: f64, balance: f64) -> JournalEntry {
        let mut e = entry(id, "bounty_prizes", amount);
        e.character_id = character;
        e.when = when;
        e.balance = Some(balance);
        e
    }

    #[test]
    fn balance_adds_up_the_picked_wallets() {
        // Newest first, like the module keeps them.
        let journal = vec![
            with_balance(2, 4, 400, -50.0, 950.0),
            with_balance(1, 3, 300, 30.0, 130.0),
            with_balance(3, 9, 250, 1e6, 2e6), // not picked
            with_balance(1, 2, 200, 100.0, 100.0),
        ];
        let picked: BTreeSet<u64> = [1, 2].into_iter().collect();
        let current = HashMap::from([(1, (130.0, 0)), (2, (950.0, 0))]);
        let series = Series::balance(&journal, &picked, &current);
        // Before anything: wallet 1 had 0, wallet 2 had 1000.
        assert_eq!(series.start, 1000.0);
        assert_eq!(
            series.points,
            vec![(200, 1100.0), (300, 1130.0), (400, 1080.0)]
        );
        assert_eq!(series.before(300), 1100.0);
        assert_eq!(series.before(301), 1130.0);
    }

    #[test]
    fn candles_have_open_high_low_close() {
        let h = 3600;
        let flows = vec![
            (MONDAY + 10, 100.0),
            (MONDAY + 20, -300.0),
            (MONDAY + 30, 50.0),
            (MONDAY + 2 * h + 5, 10.0),
        ];
        let series = Series::net(&flows);
        let hour = STEPS[3];
        let out = candles(&series, &flows, MONDAY, MONDAY + 3 * h, hour);
        assert_eq!(out.len(), 3);
        let first = &out[0];
        assert_eq!(
            (first.open, first.high, first.low, first.close),
            (0.0, 100.0, -200.0, -150.0)
        );
        assert_eq!(
            (first.income, first.expenses, first.entries),
            (150.0, -300.0, 3)
        );
        // Nothing in the second hour: flat at the last value.
        assert_eq!(
            (out[1].open, out[1].close, out[1].entries),
            (-150.0, -150.0, 0)
        );
        assert_eq!((out[2].open, out[2].close), (-150.0, -140.0));
    }

    #[test]
    fn zoomed_candles_start_from_the_value_before() {
        let flows = vec![(MONDAY + 10, 100.0), (MONDAY + DAY + 10, 5.0)];
        let series = Series::net(&flows);
        let out = candles(&series, &flows, MONDAY + DAY, MONDAY + 2 * DAY, STEPS[8]);
        assert_eq!(out.len(), 1);
        assert_eq!(
            (out[0].open, out[0].close, out[0].entries),
            (100.0, 105.0, 1)
        );
    }

    #[test]
    fn view_stays_inside_the_range() {
        let (mut a, mut b) = (-50, 100);
        clamp_view(&mut a, &mut b, 0, DAY);
        assert_eq!((a, b), (0, MIN_SPAN));
        let (mut a, mut b) = (DAY - 100, DAY + 100);
        clamp_view(&mut a, &mut b, 0, DAY);
        assert_eq!((a, b), (DAY - MIN_SPAN, DAY));
    }

    #[test]
    fn zoom_keeps_the_pointer_time_exact() {
        // A 3 h view today, zoomed in by 0.8 around its middle: the time
        // under the pointer stays put to the second.
        let (from, to) = (MONDAY + 3 * DAY, MONDAY + 3 * DAY + 3 * 3600);
        let (a, b) = zoom_view(from, to, 0.5, 0.8, MONDAY, MONDAY + 30 * DAY);
        assert_eq!(b - a, 8640);
        assert_eq!((a + b) / 2, (from + to) / 2);
        // Never closer than MIN_SPAN, never past the range.
        let (a, b) = zoom_view(from, to, 0.0, 0.01, MONDAY, MONDAY + 30 * DAY);
        assert_eq!((a, b - a), (from, MIN_SPAN));
        let (a, b) = zoom_view(from, to, 0.5, 1000.0, MONDAY, MONDAY + 30 * DAY);
        assert_eq!((a, b), (MONDAY, MONDAY + 30 * DAY));
    }

    #[test]
    fn axis_labels_differ_between_lines() {
        assert_eq!(axis_isk(150e9, 10e9), "150 b");
        assert_eq!(axis_isk(155.2e9, 0.2e9), "155.2 b");
        assert_eq!(axis_isk(-373e6, 100e6), "-373 m");
        assert_eq!(nice_step(97.0, 5.0), 20.0);
    }
}
