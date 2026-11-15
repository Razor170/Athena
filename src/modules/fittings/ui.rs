//! Draws the Fittings panel: the submodule tabs and the Compare view with
//! its three columns Is | Should | Diff. Follows the approved mockup
//! (<https://claude.ai/artifact/TmDsuRTg9qJndfD1P8HaXx>).

use eframe::egui::{
    self, Color32, CornerRadius, FontId, Galley, Margin, RichText, Sense, Stroke, pos2,
    text::LayoutJob, text::TextWrapping, vec2,
};

use super::eft::{Fit, Section};
use super::{ColumnMode, Fittings, Side, Submodule};

const GREEN: Color32 = Color32::from_rgb(0x5c, 0xbf, 0x6a);
const GREEN_BG: Color32 = Color32::from_rgb(0x1a, 0x2b, 0x1d);
const GREEN_EDGE: Color32 = Color32::from_rgb(0x2f, 0x6b, 0x38);
const RED: Color32 = Color32::from_rgb(0xe5, 0x53, 0x4b);
const RED_BG: Color32 = Color32::from_rgb(0x33, 0x1b, 0x1a);
const RED_EDGE: Color32 = Color32::from_rgb(0x7a, 0x2c, 0x28);
const YELLOW: Color32 = Color32::from_rgb(0xe0, 0xb0, 0x40);
const BRIGHT: Color32 = Color32::from_rgb(0xf0, 0xf0, 0xf0);
const TEXT: Color32 = Color32::from_rgb(0xdc, 0xdc, 0xdc);
const DIM: Color32 = Color32::from_rgb(0x9a, 0x9a, 0x9a);
const FAINT: Color32 = Color32::from_rgb(0x6a, 0x6a, 0x6a);
const CARD_FILL: Color32 = Color32::from_rgb(0x1f, 0x1f, 0x1f);
const BORDER: Color32 = Color32::from_rgb(0x35, 0x35, 0x35);

/// A column is at most this wide (Athena's 480 px rule), and at least
/// `MIN_COL` before the three columns go under each other.
const MAX_COL: f32 = 480.0;
const MIN_COL: f32 = 300.0;
const COL_GAP: f32 = 12.0;
/// Inner margin of a column frame, left and right.
const COL_PAD: f32 = 10.0;
/// One item line.
const ROW_H: f32 = 19.0;
/// The count is right-aligned in this width; the name starts after it.
const QTY_W: f32 = 64.0;

/// What a click in a column asks for. Collected while drawing and done
/// afterwards, because the columns only borrow parts of the module.
#[derive(Default)]
struct Actions {
    paste: Option<Side>,
    text_changed: bool,
}

/// The panel.
pub fn show(module: &mut Fittings, ui: &mut egui::Ui) {
    ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
    ui.horizontal(|ui| {
        ui.selectable_value(&mut module.submodule, Submodule::Compare, "Compare");
    });
    ui.separator();
    match module.submodule {
        Submodule::Compare => compare(module, ui),
    }
}

fn small(text: impl Into<String>) -> RichText {
    RichText::new(text).size(12.0)
}

fn compare(module: &mut Fittings, ui: &mut egui::Ui) {
    summary(module, ui);
    if let Some(message) = &module.message {
        ui.label(small(message).color(DIM));
    }
    let (is_hull, should_hull) = (&module.is_fit.hull, &module.should_fit.hull);
    if !is_hull.is_empty() && !should_hull.is_empty() && is_hull != should_hull {
        ui.label(
            small(format!(
                "Different hulls: Is is a {is_hull}, Should is a {should_hull}."
            ))
            .color(YELLOW),
        );
    }

    let mut actions = Actions::default();
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let available = ui.available_width();
            if available >= 3.0 * MIN_COL + 2.0 * COL_GAP {
                let width = ((available - 2.0 * COL_GAP) / 3.0).min(MAX_COL);
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = COL_GAP;
                    input_column(ui, module, Side::Is, width, &mut actions);
                    input_column(ui, module, Side::Should, width, &mut actions);
                    diff_column(ui, module, width);
                });
            } else {
                // Too narrow for three: one under the other.
                let width = available.min(MAX_COL);
                ui.spacing_mut().item_spacing.y = COL_GAP;
                input_column(ui, module, Side::Is, width, &mut actions);
                input_column(ui, module, Side::Should, width, &mut actions);
                diff_column(ui, module, width);
            }
        });

    if let Some(side) = actions.paste {
        module.paste(side);
    } else if actions.text_changed {
        module.texts_changed();
    }
}

/// Hull, the counts, and the buttons on the right.
fn summary(module: &mut Fittings, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        let hull = [&module.should_fit.hull, &module.is_fit.hull]
            .into_iter()
            .find(|h| !h.is_empty())
            .map_or("No fit yet", |h| h.as_str());
        ui.label(RichText::new(hull).strong().color(BRIGHT));
        let diff = &module.diff;
        chip(
            ui,
            &format!("{} missing", diff.missing),
            GREEN,
            GREEN_BG,
            GREEN_EDGE,
        );
        chip(
            ui,
            &format!("{} surplus", diff.surplus),
            RED,
            RED_BG,
            RED_EDGE,
        );
        chip(
            ui,
            &format!("{} matching", diff.matching),
            DIM,
            CARD_FILL,
            BORDER,
        );
        // Right to left: the first button ends up at the right edge.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .button("Copy missing")
                .on_hover_text("Copies the green items as lines for EVE's Multibuy.")
                .clicked()
            {
                module.copy_missing(ui.ctx());
            }
            if ui
                .button("Swap")
                .on_hover_text("Exchanges Is and Should.")
                .clicked()
            {
                module.swap();
            }
            if ui
                .checkbox(&mut module.saved.show_matching, "Show matching")
                .on_hover_text("Also list the items both fits have, in grey.")
                .changed()
            {
                module.save();
            }
        });
    });
}

fn chip(ui: &mut egui::Ui, text: &str, color: Color32, fill: Color32, edge: Color32) {
    egui::Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0, edge))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(8, 1))
        .show(ui, |ui| {
            ui.label(small(text).color(color));
        });
}

/// A column frame `width` wide around `add_contents`.
fn column_frame(ui: &mut egui::Ui, width: f32, add_contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(CARD_FILL)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(COL_PAD as i8, 6))
        .show(ui, |ui| {
            // The frame adds its margin and border around this width.
            let inner = (width - 2.0 * COL_PAD - 2.0).max(50.0);
            ui.set_min_width(inner);
            ui.set_max_width(inner);
            // The columns sit in a horizontal row, and a frame's inside
            // takes over its parent's direction: stack the lines vertically.
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 1.0);
                add_contents(ui);
            });
        });
}

/// The Is or Should column: header, then the parsed list or the raw text.
fn input_column(
    ui: &mut egui::Ui,
    module: &mut Fittings,
    side: Side,
    width: f32,
    actions: &mut Actions,
) {
    let (title, hint) = match side {
        Side::Is => ("Is", "the fit you have"),
        Side::Should => ("Should", "the fit you want"),
    };
    column_frame(ui, width, |ui| {
        let mode = match side {
            Side::Is => &mut module.is_mode,
            Side::Should => &mut module.should_mode,
        };
        ui.horizontal(|ui| {
            ui.label(RichText::new(title).size(14.0).strong().color(BRIGHT));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Right to left: Text, List, then Paste on their left.
                ui.selectable_value(mode, ColumnMode::Text, small("Text"));
                ui.selectable_value(mode, ColumnMode::List, small("List"));
                if ui
                    .button(small("Paste"))
                    .on_hover_text(
                        "Replaces this fit with the one on the clipboard. In EVE's fitting \
                         window use \"Copy to clipboard\" first.",
                    )
                    .clicked()
                {
                    actions.paste = Some(side);
                }
            });
        });
        let mode = *mode;
        let (fit, other, text) = match side {
            Side::Is => (
                &module.is_fit,
                &module.should_fit,
                &mut module.saved.is_text,
            ),
            Side::Should => (
                &module.should_fit,
                &module.is_fit,
                &mut module.saved.should_text,
            ),
        };
        let subtitle = if fit.hull.is_empty() {
            hint.to_string()
        } else if fit.name.is_empty() {
            fit.hull.clone()
        } else {
            format!("{} · {}", fit.hull, fit.name)
        };
        ui.add(egui::Label::new(small(subtitle).color(FAINT)).truncate());
        ui.add_space(4.0);
        match mode {
            ColumnMode::Text => {
                let edit = egui::TextEdit::multiline(text)
                    .font(egui::TextStyle::Monospace)
                    .desired_width(f32::INFINITY)
                    .desired_rows(28)
                    .hint_text("Paste an EFT fit here (Ctrl+V).");
                if ui.add(edit).changed() {
                    actions.text_changed = true;
                }
            }
            ColumnMode::List => fit_list(ui, fit, other, side),
        }
    });
}

/// The parsed fit, section by section. Is shows in red what it has more
/// of than Should; Should shows in green what Is has less of.
fn fit_list(ui: &mut egui::Ui, fit: &Fit, other: &Fit, side: Side) {
    if fit.is_empty() {
        ui.label(
            small("Paste an EFT fit (EVE fitting window › Copy to clipboard).")
                .italics()
                .color(FAINT),
        );
        return;
    }
    let mut first = true;
    for section in Section::SHOWN {
        let lines = fit.lines(section);
        if lines.is_empty() {
            continue;
        }
        section_title(ui, section, first);
        first = false;
        for line in lines {
            let more_than_other = line.count > other.count(section, &line.name);
            let color = match side {
                Side::Is if more_than_other => RED,
                Side::Should if more_than_other => GREEN,
                _ => TEXT,
            };
            let qty_color = if color == TEXT { DIM } else { color };
            item_row(
                ui,
                &group_digits(line.count),
                &line.name,
                color,
                qty_color,
                None,
            );
        }
    }
}

/// The Diff column: only differences, plus matching items when switched on.
fn diff_column(ui: &mut egui::Ui, module: &Fittings, width: f32) {
    column_frame(ui, width, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Diff").size(14.0).strong().color(BRIGHT));
            ui.label(small("+ missing").color(GREEN));
            ui.label(small("·").color(FAINT));
            ui.label(small("− surplus").color(RED));
        });
        ui.add_space(4.0);
        if module.is_fit.is_empty() && module.should_fit.is_empty() {
            ui.label(small("Paste two fits to compare.").italics().color(FAINT));
            return;
        }
        let show_matching = module.saved.show_matching;
        let mut first = true;
        for section in Section::SHOWN {
            let lines: Vec<_> = module
                .diff
                .lines(section)
                .iter()
                .filter(|l| l.change != 0 || show_matching)
                .collect();
            if lines.is_empty() {
                continue;
            }
            section_title(ui, section, first);
            first = false;
            for line in lines {
                let (qty, color, fill) = match line.change {
                    c if c > 0 => (
                        format!("+{}", group_digits(c.unsigned_abs())),
                        GREEN,
                        Some(GREEN_BG),
                    ),
                    c if c < 0 => (
                        format!("−{}", group_digits(c.unsigned_abs())),
                        RED,
                        Some(RED_BG),
                    ),
                    _ => (group_digits(line.have), FAINT, None),
                };
                item_row(ui, &qty, &line.name, color, color, fill);
            }
        }
        if module.diff.missing == 0 && module.diff.surplus == 0 {
            ui.add_space(4.0);
            ui.label(small("Fits match. Nothing missing, nothing extra.").color(GREEN));
        }
    });
}

fn section_title(ui: &mut egui::Ui, section: Section, first: bool) {
    if !first {
        ui.add_space(8.0);
    }
    ui.label(
        RichText::new(section.title().to_uppercase())
            .size(10.5)
            .color(FAINT),
    );
}

/// One item line: the count right-aligned, then the name (cut with "…"
/// when too long; hover shows all of it).
fn item_row(
    ui: &mut egui::Ui,
    qty: &str,
    name: &str,
    color: Color32,
    qty_color: Color32,
    fill: Option<Color32>,
) {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(width, ROW_H), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let painter = ui.painter();
    if let Some(fill) = fill {
        painter.rect_filled(rect, CornerRadius::same(3), fill);
    }
    let qty_galley = painter.layout_no_wrap(qty.to_string(), FontId::monospace(11.5), qty_color);
    let qty_right = rect.left() + QTY_W;
    let qty_pos = pos2(
        qty_right - qty_galley.size().x,
        rect.center().y - qty_galley.size().y / 2.0,
    );
    painter.galley(qty_pos, qty_galley, qty_color);

    let name_left = qty_right + 8.0;
    let name_galley = elided(
        painter,
        name,
        FontId::proportional(12.5),
        color,
        rect.right() - name_left - 4.0,
    );
    let cut = name_galley.elided;
    let name_pos = pos2(name_left, rect.center().y - name_galley.size().y / 2.0);
    painter.galley(name_pos, name_galley, color);
    if cut {
        response.on_hover_text(name);
    }
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

/// 100000 gives "100,000".
fn group_digits(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits_are_grouped() {
        assert_eq!(group_digits(0), "0");
        assert_eq!(group_digits(999), "999");
        assert_eq!(group_digits(1000), "1,000");
        assert_eq!(group_digits(100_000), "100,000");
        assert_eq!(group_digits(1_234_567), "1,234,567");
    }
}
