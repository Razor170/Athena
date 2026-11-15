//! Draws the CIC panel: the submodule tabs, the Logi cards (Live) and the
//! Logi settings. Follows the approved mockup
//! (<https://claude.ai/artifact/HWPaV4x4duudbJo91Vtzvr>).
//!
//! A card column is painted by hand into one fixed-size rectangle: the
//! outgoing arrows above the card, the card, and the incoming arrows below.
//! Hover areas are added on top for the tooltips.

use std::time::Duration;

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontId, Galley, Layout, Pos2, Rect, RichText,
    Sense, Shape, Stroke, StrokeKind, Vec2, pos2, text::LayoutJob, text::TextWrapping, vec2,
};

use super::config::{CHAIN_SECS, CicSettings, ECM_SECS, KEEP_ENTRIES, KEEP_SECS, Ship};
use super::icons::{Icon, Icons};
use super::logi::{Arrow, DMG_SECS, Entry, EntryKind, Lane, PilotState, entry_opacity};
use super::{Cic, Submodule, View};
use crate::static_data::RemoteKind;

const SHIELD: Color32 = Color32::from_rgb(0x7f, 0xb4, 0xff);
const ARMOR: Color32 = Color32::from_rgb(0xf0, 0x95, 0x4f);
const CAP: Color32 = Color32::from_rgb(0x5c, 0xbf, 0x6a);
const DMG: Color32 = Color32::from_rgb(0xe5, 0x53, 0x4b);
const DMG_DARK: Color32 = Color32::from_rgb(0x7a, 0x24, 0x21);
const ECM: Color32 = Color32::from_rgb(0xe8, 0xc5, 0x47);
const BRIGHT: Color32 = Color32::from_rgb(0xf0, 0xf0, 0xf0);
const TEXT: Color32 = Color32::from_rgb(0xdc, 0xdc, 0xdc);
const DIM: Color32 = Color32::from_rgb(0x9a, 0x9a, 0x9a);
const FAINT: Color32 = Color32::from_rgb(0x6a, 0x6a, 0x6a);
const YELLOW: Color32 = Color32::from_rgb(0xe0, 0xb0, 0x40);
const CARD_FILL: Color32 = Color32::from_rgb(0x1f, 0x1f, 0x1f);
const BORDER: Color32 = Color32::from_rgb(0x35, 0x35, 0x35);
const BOX_FILL: Color32 = Color32::from_rgb(0x1f, 0x1f, 0x1f);
const ROW_FILL: Color32 = Color32::from_rgb(0x2a, 0x2a, 0x2a);

/// Width of one pilot's column.
const COL_W: f32 = 352.0;
/// Height of the arrows above and below a card.
const ZONE_H: f32 = 172.0;
const CARD_H: f32 = 74.0;
/// Space between the arrows and the card.
const ZONE_GAP: f32 = 6.0;
const COL_GAP: f32 = 12.0;
const LANE_GAP: f32 = 6.0;
/// One running-log line.
const LINE_H: f32 = 17.0;
/// The arrow and its icon take this much at the left of a lane.
const ARROW_W: f32 = 16.0;
const CHIP_H: f32 = 20.0;

/// The panel.
pub fn show(module: &mut Cic, ui: &mut egui::Ui, now: f64) {
    if module.icons.is_none() {
        module.icons = Some(Icons::load(ui.ctx()));
    }
    ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
    ui.horizontal(|ui| {
        ui.selectable_value(&mut module.submodule, Submodule::Logi, "Logi");
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            // Right to left: Settings first, so Live ends up on its left.
            ui.selectable_value(&mut module.view, View::Settings, "Settings");
            ui.selectable_value(&mut module.view, View::Live, "Live");
            if crate::overlay::is_supported() {
                ui.separator();
                let mut on = module.settings.overlay.enabled;
                if ui
                    .toggle_value(&mut on, small("Overlay"))
                    .on_hover_text("Show the Logi overlay on top of the screen.")
                    .changed()
                {
                    module.settings.overlay.enabled = on;
                    if !on {
                        module.overlay_move_mode = false;
                    }
                    module.settings_edited();
                }
            }
        });
    });
    ui.separator();
    for problem in &module.problems {
        ui.colored_label(YELLOW, small(problem));
    }
    if let Some(problem) = &module.log_problem {
        ui.colored_label(YELLOW, small(problem));
    }
    match module.view {
        View::Live => live(module, ui, now),
        View::Settings => settings_view(module, ui),
    }
}

fn small(text: impl Into<String>) -> RichText {
    RichText::new(text).size(12.0)
}

// --- Live ------------------------------------------------------------------

fn live(module: &mut Cic, ui: &mut egui::Ui, now: f64) {
    let shown: Vec<(String, Ship)> = module
        .settings
        .shown()
        .map(|c| (c.name.clone(), c.ship))
        .collect();
    if shown.is_empty() {
        ui.label(
            small("No characters ticked yet. Open Settings and tick the characters whose combat logs Logi should read.")
                .color(DIM),
        );
        return;
    }
    let Some(icons) = module.icons.as_ref() else {
        return;
    };
    let empty = PilotState::default();
    let settings = &module.settings;
    let mut next_change: Option<f64> = None;
    egui::ScrollArea::horizontal()
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = COL_GAP;
                for (name, ship) in &shown {
                    let pilot = module.pilots.get(name).unwrap_or(&empty);
                    column(ui, icons, name, *ship, pilot, settings, now);
                    if let Some(at) = pilot.next_change(now, settings) {
                        next_change = Some(next_change.map_or(at, |n| n.min(at)));
                    }
                }
            });
        });
    legend(ui);
    // Fades, countdowns and badges change on their own: ask for one repaint
    // when the next of them is due, not every frame. Nothing pending, no
    // repaint.
    if let Some(at) = next_change {
        let wait = (at - now).max(0.05);
        ui.ctx()
            .request_repaint_after(Duration::from_secs_f64(wait));
    }
}

/// One pilot: outgoing arrows, the card, incoming arrows.
fn column(
    ui: &mut egui::Ui,
    icons: &Icons,
    name: &str,
    ship: Ship,
    pilot: &PilotState,
    settings: &CicSettings,
    now: f64,
) {
    let height = ZONE_H * 2.0 + ZONE_GAP * 2.0 + CARD_H;
    let (rect, _) = ui.allocate_exact_size(vec2(COL_W, height), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let tank = if ship.is_shield() { SHIELD } else { ARMOR };
    let tank_name = if ship.is_shield() { "Shield" } else { "Armor" };
    let tank_icon = if ship.is_shield() {
        Icon::Shield
    } else {
        Icon::Armor
    };
    let chain = ship.has_cap_chain();

    let out_rect = Rect::from_min_size(rect.min, vec2(COL_W, ZONE_H));
    let card_rect = Rect::from_min_size(
        pos2(rect.left(), out_rect.bottom() + ZONE_GAP),
        vec2(COL_W, CARD_H),
    );
    let in_rect = Rect::from_min_size(
        pos2(rect.left(), card_rect.bottom() + ZONE_GAP),
        vec2(COL_W, ZONE_H),
    );

    let lane_ctx = LaneCtx {
        icons,
        pilot,
        settings,
        now,
        id: ui.id().with(name),
    };

    // Above: what we send.
    let mut out = vec![LaneSpec::new(
        Lane::RepOut,
        format!("{tank_name} out"),
        tank,
        tank_icon,
    )];
    if chain {
        out.push(LaneSpec::new(
            Lane::CapOut,
            "Cap out".into(),
            CAP,
            Icon::Cap,
        ));
    }
    for (spec, lane_rect) in out.iter().zip(split(out_rect, &vec![1.0; out.len()])) {
        lane(ui, &lane_ctx, spec, lane_rect, true);
    }

    card(ui, icons, card_rect, name, ship, pilot, settings, now);

    // Below: what we get. Reps under reps, damage in the middle, cap under cap.
    let mut incoming = vec![
        LaneSpec::new(Lane::RepIn, format!("{tank_name} in"), tank, tank_icon),
        LaneSpec::new(Lane::Damage, "Damage".into(), DMG, Icon::Damage),
    ];
    let mut weights = vec![1.0, 0.62];
    if chain {
        incoming.push(LaneSpec::new(Lane::CapIn, "Cap in".into(), CAP, Icon::Cap));
        weights.push(1.0);
    }
    for (spec, lane_rect) in incoming.iter().zip(split(in_rect, &weights)) {
        lane(ui, &lane_ctx, spec, lane_rect, false);
    }
}

/// Splits `rect` side by side by `weights`, with [`LANE_GAP`] between.
fn split(rect: Rect, weights: &[f32]) -> Vec<Rect> {
    let total: f32 = weights.iter().sum();
    let free = rect.width() - LANE_GAP * (weights.len() as f32 - 1.0);
    let mut x = rect.left();
    weights
        .iter()
        .map(|w| {
            let width = free * w / total;
            let r = Rect::from_min_size(pos2(x, rect.top()), vec2(width, rect.height()));
            x += width + LANE_GAP;
            r
        })
        .collect()
}

struct LaneSpec {
    lane: Lane,
    label: String,
    color: Color32,
    icon: Icon,
}

impl LaneSpec {
    fn new(lane: Lane, label: String, color: Color32, icon: Icon) -> LaneSpec {
        LaneSpec {
            lane,
            label,
            color,
            icon,
        }
    }
}

/// What every lane of a column needs.
struct LaneCtx<'a> {
    icons: &'a Icons,
    pilot: &'a PilotState,
    settings: &'a CicSettings,
    now: f64,
    id: egui::Id,
}

/// One arrow with its running log. `out`: above the card (label on top,
/// newest entry at the bottom, next to the card); otherwise below it
/// (newest at the top, label at the bottom).
fn lane(ui: &mut egui::Ui, ctx: &LaneCtx, spec: &LaneSpec, rect: Rect, out: bool) {
    let painter = ui.painter_at(rect.expand(2.0));
    let x = rect.left() + 7.0;
    let dashed = Stroke::new(1.6, spec.color);
    // The arrow points up: away from the card above it, into the card below.
    let (line_top, line_bottom, head_y) = if out {
        (rect.top() + 26.0, rect.bottom(), rect.top() + 22.0)
    } else {
        (rect.top() + 2.0, rect.bottom() - 26.0, rect.top() + 1.0)
    };
    painter.extend(Shape::dashed_line(
        &[pos2(x, line_top), pos2(x, line_bottom)],
        dashed,
        4.0,
        3.0,
    ));
    painter.add(Shape::line(
        vec![
            pos2(x - 5.0, head_y + 10.0),
            pos2(x, head_y),
            pos2(x + 5.0, head_y + 10.0),
        ],
        dashed,
    ));
    let icon_y = if out {
        rect.top()
    } else {
        rect.bottom() - 20.0
    };
    ctx.icons.paint(
        &painter,
        spec.icon,
        Rect::from_min_size(pos2(rect.left() - 3.0, icon_y), vec2(20.0, 20.0)),
        spec.color,
    );

    let text_left = rect.left() + ARROW_W;
    let text_w = rect.right() - text_left;
    let label = elided(
        &painter,
        &spec.label.to_uppercase(),
        FontId::proportional(10.5),
        spec.color,
        text_w,
    );
    let label_y = if out {
        rect.top() + 3.0
    } else {
        rect.bottom() - 3.0 - label.size().y
    };
    painter.galley(pos2(text_left, label_y), label, spec.color);

    let entries = ctx.pilot.entries(spec.lane, ctx.now, ctx.settings);
    let with_name = spec.lane != Lane::Damage;
    if entries.is_empty() {
        let text = elided(
            &painter,
            "nothing yet",
            FontId::proportional(11.0),
            FAINT,
            text_w,
        );
        let y = if out {
            rect.bottom() - 4.0 - LINE_H
        } else {
            rect.top() + 4.0
        };
        painter.galley(pos2(text_left, y + 2.0), text, FAINT);
        return;
    }
    // Room for this many lines between the label and the card.
    let rows = ((ZONE_H - 22.0) / LINE_H).floor() as usize;
    for (i, entry) in entries.iter().take(rows).enumerate() {
        let y = if out {
            rect.bottom() - 4.0 - LINE_H * (i as f32 + 1.0)
        } else {
            rect.top() + 4.0 + LINE_H * i as f32
        };
        let line = Rect::from_min_size(pos2(text_left, y), vec2(text_w, LINE_H));
        entry_line(ui, &painter, ctx, entry, line, with_name);
    }
}

/// One running-log line: the number, then the arrow and name.
fn entry_line(
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    ctx: &LaneCtx,
    entry: &Entry,
    line: Rect,
    with_name: bool,
) {
    let age = ctx.now - entry.at;
    let opacity = entry_opacity(age, ctx.settings.keep_secs);
    let color = match entry.kind {
        EntryKind::Shield => SHIELD,
        EntryKind::Armor => ARMOR,
        EntryKind::Cap => CAP,
        EntryKind::Damage => DMG,
    };
    let (value, value_color) = match entry.amount {
        None => ("miss".to_string(), FAINT),
        Some(0) => ("0".to_string(), FAINT),
        Some(n) => (n.to_string(), color),
    };
    let value_font = FontId::monospace(11.5);
    let value_w = 34.0;
    let center_y = line.center().y;
    painter.text(
        pos2(line.left() + value_w, center_y),
        Align2::RIGHT_CENTER,
        &value,
        value_font,
        value_color.gamma_multiply(opacity),
    );
    let mut x = line.left() + value_w + 6.0;
    if with_name {
        if let Some(arrow) = entry.arrow {
            paint_arrow(
                painter,
                pos2(x + 3.0, center_y),
                arrow,
                FAINT.gamma_multiply(opacity),
            );
            x += 10.0;
        }
        let who = elided(
            painter,
            &entry.who,
            FontId::proportional(11.5),
            DIM,
            (line.right() - x).max(1.0),
        );
        let y = center_y - who.size().y / 2.0;
        painter.galley(pos2(x, y), who, DIM.gamma_multiply(opacity));
    }

    let mut tip = String::new();
    if !entry.module.is_empty() {
        tip.push_str(&entry.module);
        tip.push_str(" · ");
    }
    if !entry.who.is_empty() {
        tip.push_str(&entry.who);
        tip.push_str(" · ");
    }
    if let Some(arrow) = entry.arrow {
        tip.push_str(arrow.words());
        tip.push_str(" · ");
    }
    tip.push_str(&format!("{:.0} s ago", age.max(0.0)));
    ui.interact(
        line,
        ctx.id.with((
            "entry",
            entry.at.to_bits(),
            line.top() as i32,
            line.left() as i32,
        )),
        Sense::hover(),
    )
    .on_hover_text(tip);
}

/// The card: name, ship, and the badges.
#[allow(clippy::too_many_arguments)]
fn card(
    ui: &mut egui::Ui,
    icons: &Icons,
    rect: Rect,
    name: &str,
    ship: Ship,
    pilot: &PilotState,
    settings: &CicSettings,
    now: f64,
) {
    let painter = ui.painter_at(rect.expand(1.0));
    let radius = CornerRadius::same(6);
    painter.rect_filled(rect, radius, CARD_FILL);
    let hit = pilot.hit(now);
    let ecm = pilot.ecm_left(now, settings);
    // Damage beats ECM for the border.
    let border = if hit.is_some() {
        Some(DMG)
    } else if ecm.is_some() {
        Some(ECM)
    } else {
        None
    };
    match border {
        Some(color) => {
            let r = rect.shrink(0.5);
            painter.extend(Shape::dashed_line(
                &[
                    r.left_top(),
                    r.right_top(),
                    r.right_bottom(),
                    r.left_bottom(),
                    r.left_top(),
                ],
                Stroke::new(1.0, color),
                5.0,
                3.0,
            ));
        }
        None => {
            painter.rect_stroke(rect, radius, Stroke::new(1.0, BORDER), StrokeKind::Inside);
        }
    }

    // Top line: name, and the ship on the right.
    let tank = if ship.is_shield() { SHIELD } else { ARMOR };
    let ship_text =
        painter.layout_no_wrap(ship.name().to_string(), FontId::proportional(11.0), tank);
    let ship_rect = Rect::from_min_size(
        pos2(
            rect.right() - 10.0 - ship_text.size().x - 14.0,
            rect.top() + 9.0,
        ),
        ship_text.size() + vec2(14.0, 2.0),
    );
    painter.rect_filled(ship_rect, CornerRadius::same(3), tank.gamma_multiply(0.12));
    painter.rect_stroke(
        ship_rect,
        CornerRadius::same(3),
        Stroke::new(1.0, tank.gamma_multiply(0.5)),
        StrokeKind::Inside,
    );
    painter.galley(ship_rect.min + vec2(7.0, 1.0), ship_text, tank);
    let name_galley = elided(
        &painter,
        name,
        FontId::proportional(14.0),
        BRIGHT,
        ship_rect.left() - rect.left() - 18.0,
    );
    painter.galley(
        pos2(rect.left() + 10.0, rect.top() + 8.0),
        name_galley,
        BRIGHT,
    );

    // Badge line.
    let mut chips = ChipRow {
        ui,
        painter: &painter,
        icons,
        x: rect.left() + 10.0,
        y: rect.top() + 38.0,
        right: rect.right() - 8.0,
        id: egui::Id::new(("cic-card", name)),
        count: 0,
    };
    if let Some((age, amount)) = hit {
        // Blinks every half second and fades out over the 3 s.
        let blink = (now * 2.0).floor() as i64 % 2 == 1;
        let fill = if blink { DMG_DARK } else { DMG };
        let opacity = (1.0 - age / (DMG_SECS + 0.5)) as f32;
        chips.chip(
            Some(Icon::Damage),
            "DMG",
            Color32::WHITE.gamma_multiply(opacity),
            fill.gamma_multiply(opacity),
            fill.gamma_multiply(opacity),
            &format!("Taking damage: {amount} HP"),
        );
    }
    for outgoing in [false, true] {
        let links = pilot.links(outgoing, now, settings);
        if links.is_empty() {
            continue;
        }
        let arrows: Vec<Arrow> = links.iter().take(2).map(|l| l.arrow).collect();
        let who: Vec<String> = links
            .iter()
            .map(|l| format!("{} ({})", l.who, l.arrow.words()))
            .collect();
        let (color, fill, edge, verb) = if outgoing {
            (
                DMG,
                Color32::from_rgb(0x2e, 0x1c, 0x1b),
                Color32::from_rgb(0x6a, 0x2f, 0x2c),
                "Sending cap to ",
            )
        } else {
            (
                CAP,
                Color32::from_rgb(0x1c, 0x2b, 0x1e),
                Color32::from_rgb(0x2f, 0x5a, 0x35),
                "Receiving cap from ",
            )
        };
        chips.chain_chip(
            &arrows.len().to_string(),
            &arrows,
            color,
            fill,
            edge,
            &format!("{verb}{}", who.join(", ")),
        );
    }
    if let Some((left, by)) = ecm {
        chips.chip(
            Some(Icon::Ecm),
            &format!("ECM {:.0}s", left.ceil()),
            Color32::from_rgb(0x1a, 0x1a, 0x1a),
            ECM,
            ECM,
            &format!(
                "Jammed by {by}: lost lock, can lock again in {:.0} s",
                left.ceil()
            ),
        );
    }
    if let Some(from) = pilot.tackle(false, now) {
        chips.chip(
            Some(Icon::Tackle),
            "POINT",
            Color32::from_rgb(0xd9, 0xb8, 0xff),
            Color32::from_rgb(0x2a, 0x20, 0x36),
            Color32::from_rgb(0x6a, 0x4f, 0x8f),
            &format!("Warp disruption attempt from {from}"),
        );
    }
    if let Some(from) = pilot.tackle(true, now) {
        chips.chip(
            Some(Icon::Tackle),
            "SCRAM",
            Color32::WHITE,
            Color32::from_rgb(0x7a, 0x3f, 0xb8),
            Color32::from_rgb(0x9a, 0x62, 0xd6),
            &format!("Warp scramble attempt from {from}"),
        );
    }
    if chips.count == 0 {
        painter.text(
            pos2(rect.left() + 10.0, rect.top() + 38.0 + CHIP_H / 2.0),
            Align2::LEFT_CENTER,
            "no cap chain",
            FontId::proportional(11.0),
            FAINT,
        );
    }

    // ECM countdown bar along the bottom.
    if let Some((left, _)) = ecm {
        let share = (left / f64::from(settings.ecm_secs)).clamp(0.0, 1.0) as f32;
        let bar = Rect::from_min_size(
            pos2(rect.left(), rect.bottom() - 3.0),
            vec2(rect.width() * share, 3.0),
        );
        painter.rect_filled(
            bar,
            CornerRadius {
                sw: 6,
                ..CornerRadius::ZERO
            },
            ECM,
        );
    }
}

/// Lays badges out left to right on a card.
struct ChipRow<'a, 'b> {
    ui: &'b mut egui::Ui,
    painter: &'a egui::Painter,
    icons: &'a Icons,
    x: f32,
    y: f32,
    right: f32,
    id: egui::Id,
    count: usize,
}

impl ChipRow<'_, '_> {
    /// Takes the next spot of `width`, or `None` when the row is full.
    fn place(&mut self, width: f32, tip: &str) -> Option<Rect> {
        if self.x + width > self.right {
            return None;
        }
        let rect = Rect::from_min_size(pos2(self.x, self.y), vec2(width, CHIP_H));
        self.x += width + 8.0;
        self.count += 1;
        self.ui
            .interact(rect, self.id.with(self.count), Sense::hover())
            .on_hover_text(tip);
        Some(rect)
    }

    fn chip(
        &mut self,
        icon: Option<Icon>,
        text: &str,
        color: Color32,
        fill: Color32,
        edge: Color32,
        tip: &str,
    ) {
        let galley = bold_galley(self.painter, text, color);
        let icon_w = if icon.is_some() { 20.0 } else { 0.0 };
        let width = 5.0 + icon_w + galley.size().x + 9.0;
        let Some(rect) = self.place(width, tip) else {
            return;
        };
        let radius = CornerRadius::same(10);
        self.painter.rect_filled(rect, radius, fill);
        self.painter
            .rect_stroke(rect, radius, Stroke::new(1.0, edge), StrokeKind::Inside);
        let mut x = rect.left() + 5.0;
        if let Some(icon) = icon {
            self.icons.paint(
                self.painter,
                icon,
                Rect::from_min_size(pos2(x, rect.top() + 1.0), vec2(18.0, 18.0)),
                color,
            );
            x += icon_w;
        }
        let y = rect.center().y - galley.size().y / 2.0;
        self.painter.galley(pos2(x, y), galley, color);
    }

    /// A cap chain badge: a chain glyph, the link count and the arrows.
    fn chain_chip(
        &mut self,
        text: &str,
        arrows: &[Arrow],
        color: Color32,
        fill: Color32,
        edge: Color32,
        tip: &str,
    ) {
        let galley = bold_galley(self.painter, text, color);
        let arrows_w = ARROW_STEP * arrows.len() as f32;
        let width = 5.0 + 20.0 + galley.size().x + 3.0 + arrows_w + 7.0;
        let Some(rect) = self.place(width, tip) else {
            return;
        };
        let radius = CornerRadius::same(10);
        self.painter.rect_filled(rect, radius, fill);
        self.painter
            .rect_stroke(rect, radius, Stroke::new(1.0, edge), StrokeKind::Inside);
        chain_glyph(
            self.painter,
            pos2(rect.left() + 14.0, rect.center().y),
            color,
        );
        let y = rect.center().y - galley.size().y / 2.0;
        let text_w = galley.size().x;
        self.painter
            .galley(pos2(rect.left() + 25.0, y), galley, color);
        let mut x = rect.left() + 25.0 + text_w + 3.0 + ARROW_STEP / 2.0;
        for arrow in arrows {
            paint_arrow(self.painter, pos2(x, rect.center().y), *arrow, color);
            x += ARROW_STEP;
        }
    }
}

/// Room for one painted arrow on a chain badge.
const ARROW_STEP: f32 = 9.0;

/// Paints ↑, ↓, ↕ or "?" centred on `center`, 10 points tall. Painted
/// because egui's built-in font has no ↑ and ↓.
fn paint_arrow(painter: &egui::Painter, center: Pos2, arrow: Arrow, color: Color32) {
    let stroke = Stroke::new(1.4, color);
    let top = center + vec2(0.0, -5.0);
    let bottom = center + vec2(0.0, 5.0);
    let head = |tip: Pos2, dir: f32| {
        Shape::line(
            vec![tip + vec2(-3.0, 3.5 * dir), tip, tip + vec2(3.0, 3.5 * dir)],
            stroke,
        )
    };
    match arrow {
        Arrow::Other => {
            painter.text(
                center,
                Align2::CENTER_CENTER,
                "?",
                FontId::proportional(11.0),
                color,
            );
        }
        Arrow::Up | Arrow::Down | Arrow::Unknown => {
            painter.line_segment([top, bottom], stroke);
            if arrow != Arrow::Down {
                painter.add(head(top, 1.0));
            }
            if arrow != Arrow::Up {
                painter.add(head(bottom, -1.0));
            }
        }
    }
}

/// Two chain links, drawn as rounded outlines on a slant.
fn chain_glyph(painter: &egui::Painter, center: Pos2, color: Color32) {
    let stroke = Stroke::new(2.0, color);
    for offset in [vec2(-2.6, 2.6), vec2(2.6, -2.6)] {
        let c = center + offset;
        // A link: a short slanted capsule, as a closed line around it.
        let slant = std::f32::consts::FRAC_1_SQRT_2;
        let along = vec2(slant, -slant) * 3.2;
        let across = vec2(slant, slant) * 2.4;
        let points: Vec<Pos2> = (0..16)
            .map(|i| {
                let t = i as f32 / 16.0 * std::f32::consts::TAU;
                c + along * t.cos() * 1.3 + across * t.sin()
            })
            .collect();
        painter.add(Shape::closed_line(points, stroke));
    }
}

fn bold_galley(painter: &egui::Painter, text: &str, color: Color32) -> std::sync::Arc<Galley> {
    let mut job = LayoutJob::simple_singleline(text.to_string(), FontId::proportional(12.0), color);
    if let Some(section) = job.sections.first_mut() {
        // egui's default font has no bold; a tiny extra letter spacing reads
        // a bit stronger on the badges.
        section.format.extra_letter_spacing = 0.3;
    }
    painter.layout_job(job)
}

/// A one-line text cut with "…" to `width`.
fn elided(
    painter: &egui::Painter,
    text: &str,
    font: FontId,
    color: Color32,
    width: f32,
) -> std::sync::Arc<Galley> {
    let mut job = LayoutJob::simple_singleline(text.to_string(), font, color);
    job.wrap = TextWrapping {
        max_width: width.max(1.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    painter.layout_job(job)
}

fn legend(ui: &mut egui::Ui) {
    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 16.0;
        for (color, text) in [
            (SHIELD, "Shield reps"),
            (ARMOR, "Armor reps"),
            (CAP, "Capacitor"),
            (DMG, "Damage"),
        ] {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let (rect, _) = ui.allocate_exact_size(vec2(18.0, 12.0), Sense::hover());
                ui.painter().extend(Shape::dashed_line(
                    &[rect.left_center(), rect.right_center()],
                    Stroke::new(2.0, color),
                    4.0,
                    3.0,
                ));
                ui.label(small(text).color(DIM));
            });
        }
        for (color, text) in [(CAP, "receiving cap"), (DMG, "sending cap")] {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let (rect, _) = ui.allocate_exact_size(vec2(18.0, 14.0), Sense::hover());
                chain_glyph(ui.painter(), rect.center(), color);
                ui.label(small(text).color(DIM));
            });
        }
        for (arrow, text) in [
            (Arrow::Up, "link to the pilot above in the chain,"),
            (Arrow::Down, "to the pilot below,"),
            (
                Arrow::Unknown,
                "unknown (same module both ways; set in Settings)",
            ),
        ] {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let (rect, _) = ui.allocate_exact_size(vec2(8.0, 14.0), Sense::hover());
                paint_arrow(ui.painter(), rect.center(), arrow, DIM);
                ui.label(small(text).color(DIM));
            });
        }
    });
}

// --- Settings --------------------------------------------------------------

fn settings_view(module: &mut Cic, ui: &mut egui::Ui) {
    let mut changed = false;
    let mut chain_changed = false;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.with_layout(
                Layout::left_to_right(Align::Min).with_main_wrap(true),
                |ui| {
                    ui.spacing_mut().item_spacing = vec2(18.0, 18.0);
                    settings_box(ui, 480.0, |ui| {
                        changed |= characters_box(module, ui);
                    });
                    settings_box(ui, 440.0, |ui| {
                        chain_changed |= chain_box(module, ui);
                    });
                    settings_box(ui, 360.0, |ui| {
                        changed |= timing_box(&mut module.settings, ui);
                    });
                    if crate::overlay::is_supported() {
                        settings_box(ui, 360.0, |ui| {
                            changed |= overlay_box(module, ui);
                        });
                    }
                },
            );
        });
    if chain_changed {
        module.chain_modules_edited();
    } else if changed {
        module.settings_edited();
    }
}

fn settings_box(ui: &mut egui::Ui, width: f32, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(BOX_FILL)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(12.0)
        .show(ui, |ui| {
            ui.set_width(width.min(ui.available_width().max(200.0)));
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            ui.vertical(add);
        });
}

fn box_title(ui: &mut egui::Ui, title: &str, text: &str) {
    ui.label(RichText::new(title).color(BRIGHT).strong());
    ui.label(small(text).color(DIM));
    ui.add_space(4.0);
}

/// The character list: tick, ship, drag to reorder. True if changed.
fn characters_box(module: &mut Cic, ui: &mut egui::Ui) -> bool {
    box_title(
        ui,
        "Characters",
        "Tick the characters whose combat logs Logi reads. Drag a row by its handle to change the card order. The ship type decides which arrows a card shows.",
    );
    let settings = &mut module.settings;
    if settings.characters.is_empty() {
        ui.label(small("No characters found yet. They show up here once they have a game log, or are logged in on the Characters tab.").color(DIM));
        return false;
    }
    let mut changed = false;
    // Drag and drop: (dragged name, target name, after the target).
    let mut dropped: Option<(String, String, bool)> = None;
    let pointer_y = ui.ctx().pointer_interact_pos().map(|p| p.y);
    let in_logs = &module.in_logs;
    for character in &mut settings.characters {
        let name = character.name.clone();
        let drag_id = egui::Id::new(("cic_order", &name));
        let being_dragged = ui.ctx().is_being_dragged(drag_id);
        let row = egui::Frame::new()
            .fill(ROW_FILL)
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(CornerRadius::same(5))
            .inner_margin(egui::Margin::symmetric(8, 4))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                if being_dragged {
                    ui.multiply_opacity(0.4);
                }
                ui.horizontal(|ui| {
                    let handle = drag_handle(ui, drag_id);
                    // The payload is the dragged row's character name.
                    handle.dnd_set_drag_payload(name.clone());
                    if ui
                        .checkbox(&mut character.show, "")
                        .on_hover_text(format!("Read {name}'s combat log and show a card"))
                        .changed()
                    {
                        changed = true;
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        egui::ComboBox::from_id_salt(("cic_ship", &name))
                            .width(96.0)
                            .selected_text(character.ship.name())
                            .show_ui(ui, |ui| {
                                for ship in Ship::ALL {
                                    if ui
                                        .selectable_value(&mut character.ship, ship, ship.name())
                                        .changed()
                                    {
                                        changed = true;
                                    }
                                }
                            });
                        // The name fills the rest of the row, from the left.
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            let color = if character.show { BRIGHT } else { FAINT };
                            let label = ui.add(
                                egui::Label::new(RichText::new(&name).color(color))
                                    .truncate()
                                    .selectable(false),
                            );
                            if !in_logs.contains(&name) {
                                label.on_hover_text("No game log from the last 24 hours");
                            }
                        });
                    });
                });
            });

        let rect = row.response.rect;
        let after = pointer_y.is_some_and(|y| y > rect.center().y);
        if let Some(dragged) = row.response.dnd_hover_payload::<String>()
            && *dragged != name
        {
            // Show where the row will land.
            let y = if after {
                rect.bottom() + 3.0
            } else {
                rect.top() - 3.0
            };
            ui.painter()
                .hline(rect.x_range(), y, Stroke::new(2.0, SHIELD));
        }
        if let Some(dragged) = row.response.dnd_release_payload::<String>() {
            dropped = Some(((*dragged).clone(), name.clone(), after));
        }
    }
    if let Some((dragged, target, after)) = dropped {
        settings.move_character(&dragged, &target, after);
        changed = true;
    }
    changed
}

/// The grip at the start of a row: three short lines. Drag it to move the row.
fn drag_handle(ui: &mut egui::Ui, id: egui::Id) -> egui::Response {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(14.0, 18.0), Sense::hover());
    let response = ui
        .interact(rect, id, Sense::drag())
        .on_hover_cursor(egui::CursorIcon::Grab)
        .on_hover_text("Drag to change the order");
    let color = if response.hovered() || response.dragged() {
        BRIGHT
    } else {
        FAINT
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

/// Up and Down transmitter for Basilisk and Guardian. True if changed.
fn chain_box(module: &mut Cic, ui: &mut egui::Ui) -> bool {
    box_title(
        ui,
        "Cap chain modules",
        "Which transmitter sends cap up the chain and which sends it down. The lists come from the SDE (remote capacitor transmitters). If both are the same module, the direction can't be known and the arrow points both ways.",
    );
    let modules: Vec<String> = module
        .static_data
        .as_ref()
        .map(|data| {
            data.remote_modules(RemoteKind::Cap)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if modules.is_empty() {
        ui.label(
            small("The module list needs static.sqlite; type the names instead.").color(YELLOW),
        );
    }
    let mut changed = false;
    for (ship, chain) in [
        (Ship::Basilisk, &mut module.settings.basilisk_chain),
        (Ship::Guardian, &mut module.settings.guardian_chain),
    ] {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(ship.name()).color(BRIGHT).strong());
            if chain.up == chain.down {
                ui.label(small("same module both ways: direction unknown").color(FAINT));
            }
        });
        for (arrow, label, value) in [
            (Arrow::Up, "Up the chain", &mut chain.up),
            (Arrow::Down, "Down the chain", &mut chain.down),
        ] {
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(8.0, 14.0), Sense::hover());
                paint_arrow(ui.painter(), rect.center(), arrow, TEXT);
                ui.label(RichText::new(label).color(TEXT));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if modules.is_empty() {
                        changed |= ui
                            .add(egui::TextEdit::singleline(value).desired_width(270.0))
                            .changed();
                        return;
                    }
                    egui::ComboBox::from_id_salt(("cic_chain", ship.name(), label))
                        .width(270.0)
                        .truncate()
                        .selected_text(value.as_str())
                        .show_ui(ui, |ui| {
                            for name in &modules {
                                if ui.selectable_label(value == name, name.as_str()).clicked()
                                    && value != name
                                {
                                    *value = name.clone();
                                    changed = true;
                                }
                            }
                        });
                });
            });
        }
    }
    changed
}

/// The Logi overlay: on/off, what it shows, and moving it.
fn overlay_box(module: &mut Cic, ui: &mut egui::Ui) -> bool {
    box_title(
        ui,
        "Overlay",
        "One line per shown pilot on top of the screen: cap chain, DMG, ECM and point/scram. Clicks go through it.",
    );
    let overlay = &mut module.settings.overlay;
    let mut changed = false;
    if ui
        .checkbox(&mut overlay.enabled, "Show Logi overlay")
        .changed()
    {
        if !overlay.enabled {
            module.overlay_move_mode = false;
        }
        changed = true;
    }
    changed |= ui
        .checkbox(&mut overlay.show_names, "Show pilot names")
        .changed();
    changed |= ui
        .checkbox(
            &mut overlay.only_busy,
            "Only pilots with something going on",
        )
        .on_hover_text("Hit, jammed, pointed or scrammed, or a cap chain link missing.")
        .changed();
    ui.horizontal(|ui| {
        ui.label(RichText::new("Background opacity").color(TEXT));
        changed |= ui
            .add(
                egui::Slider::new(&mut overlay.background_opacity, 0.0..=100.0)
                    .suffix("%")
                    .fixed_decimals(0),
            )
            .changed();
    });
    let enabled = overlay.enabled;
    ui.horizontal(|ui| {
        let moving = module.overlay_move_mode;
        let label = if moving {
            "Done"
        } else {
            "Move / resize overlay"
        };
        if ui
            .add_enabled(enabled, egui::Button::new(label).selected(moving))
            .clicked()
        {
            module.overlay_move_mode = !moving;
            // Not a setting, but the overlay must change now.
            module.egui_ctx.request_repaint();
        }
        let hint = if module.overlay_move_mode {
            "Drag it to move it, its right edge to make room for names."
        } else {
            "Position and width are saved."
        };
        ui.label(RichText::new(hint).size(11.0).color(FAINT));
    });
    changed
}

/// The running log and timing numbers. True if changed.
fn timing_box(settings: &mut CicSettings, ui: &mut egui::Ui) -> bool {
    box_title(
        ui,
        "Running log",
        "How long numbers and badges stay on screen.",
    );
    let mut changed = false;
    for (label, hint, value, (min, max), unit) in [
        (
            "Entries per arrow",
            "older entries drop off",
            &mut settings.keep_entries,
            KEEP_ENTRIES,
            "",
        ),
        (
            "Hide entries after",
            "they fade out before",
            &mut settings.keep_secs,
            KEEP_SECS,
            " s",
        ),
        (
            "Chain counts as broken after",
            "seconds without a transfer",
            &mut settings.chain_secs,
            CHAIN_SECS,
            " s",
        ),
        (
            "ECM lock-out",
            "countdown after being jammed",
            &mut settings.ecm_secs,
            ECM_SECS,
            " s",
        ),
    ] {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.label(RichText::new(label).color(TEXT));
                ui.label(RichText::new(hint).size(11.0).color(FAINT));
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                changed |= ui
                    .add(
                        egui::DragValue::new(value)
                            .range(min..=max)
                            .suffix(unit)
                            .speed(0.2),
                    )
                    .changed();
            });
        });
        ui.separator();
    }
    changed
}
