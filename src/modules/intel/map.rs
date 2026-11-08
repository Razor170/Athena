//! The region map: a dot per system, its name and security on a small dark
//! label, gate lines, and the own pilots in blue, drawn like the approved
//! mockup (and RIFT).
//!
//! Everything is drawn with egui's painter from positions the xtask laid
//! out (see `xtask/src/map.rs`), so nothing is laid out here. To keep pan
//! and zoom smooth:
//! - the shapes of a region are worked out once when the region is picked
//!   ([`RegionDrawing`]), not every frame;
//! - anything off screen is skipped;
//! - text sizes go in whole points, so zooming doesn't fill egui's font
//!   cache with a new size every frame.
//!
//! egui repaints by itself while the map is dragged or scrolled; the map
//! never asks for a repaint of its own.

use std::collections::HashMap;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Shape, Stroke, Vec2,
};

use crate::universe::{Universe, drawing};

// Colours from the mockup.
const BACKGROUND: Color32 = Color32::from_rgb(0x0f, 0x12, 0x15);
const FRAME: Color32 = Color32::from_rgb(0x2a, 0x2f, 0x36);
const GATE: Color32 = Color32::from_rgb(0x4a, 0x51, 0x5c);
const IDLE_DOT: Color32 = Color32::from_rgb(0x6b, 0x71, 0x7a);
const LABEL_FILL: Color32 = Color32::from_rgb(0x14, 0x18, 0x1d);
const NAME: Color32 = Color32::from_rgb(0xf0, 0xf0, 0xf0);
const OTHER_REGION_NAME: Color32 = Color32::from_rgb(0xa8, 0xa8, 0xa8);
const OTHER_REGION_TEXT: Color32 = Color32::from_rgb(0x9a, 0x9a, 0x9a);
pub const PILOT_BLUE: Color32 = Color32::from_rgb(0x4e, 0xa1, 0xff);
const PILOT_TEXT: Color32 = Color32::from_rgb(0x8f, 0xc3, 0xff);
const LEGEND_TEXT: Color32 = Color32::from_rgb(0xa8, 0xa8, 0xa8);

/// Zoom limits. 1.0 is the size the xtask laid the map out for.
const MIN_ZOOM: f32 = 0.3;
const MAX_ZOOM: f32 = 3.0;
/// Each zoom button press zooms by this factor.
const ZOOM_STEP: f32 = 1.25;
/// Below this zoom the labels would be unreadable; only dots are drawn.
const LABELS_FROM_ZOOM: f32 = 0.55;

/// One system to draw, in map points.
struct Node {
    id: i64,
    pos: Pos2,
    name: String,
    /// "-0.4", or the other region's name for border systems.
    detail: String,
    detail_color: Color32,
    /// A system of a neighbouring region (hollow dot).
    border: bool,
}

/// The shapes of one region, worked out once when it is picked.
struct RegionDrawing {
    region_id: i64,
    nodes: Vec<Node>,
    /// Gate lines in map points; `true` for dashed lines to other regions.
    lines: Vec<(Pos2, Pos2, bool)>,
    /// The area to show for "Fit region".
    bounds: Rect,
}

impl RegionDrawing {
    fn new(universe: &Universe, region_id: i64) -> Option<RegionDrawing> {
        let region = universe.region(region_id)?;
        let mut nodes = Vec::new();
        let mut position: HashMap<i64, Pos2> = HashMap::new();
        for &id in &region.systems {
            let Some(system) = universe.system(id) else {
                continue;
            };
            let pos = Pos2::new(system.map_pos[0], system.map_pos[1]);
            position.insert(id, pos);
            let security = system.display_security();
            nodes.push(Node {
                id,
                pos,
                name: system.name.clone(),
                detail: format!("{security:.1}"),
                detail_color: security_color(security),
                border: false,
            });
        }
        for border in &region.borders {
            let Some(system) = universe.system(border.system_id) else {
                continue;
            };
            let pos = Pos2::new(border.map_pos[0], border.map_pos[1]);
            position.insert(system.id, pos);
            let region_name = universe
                .region(system.region_id)
                .map(|r| r.name.clone())
                .unwrap_or_default();
            nodes.push(Node {
                id: system.id,
                pos,
                name: system.name.clone(),
                detail: region_name,
                detail_color: OTHER_REGION_TEXT,
                border: true,
            });
        }

        // Each gate once: from a system of this region to a neighbour with
        // a higher id, or to any border system.
        let mut lines = Vec::new();
        for &id in &region.systems {
            for &next in universe.neighbours(id) {
                let next_is_border = region.borders.iter().any(|b| b.system_id == next);
                let next_is_ours = !next_is_border && position.contains_key(&next);
                if (next_is_ours && next > id) || next_is_border {
                    lines.push((position[&id], position[&next], next_is_border));
                }
            }
        }

        let [left, top, right, bottom] = region.bounds;
        Some(RegionDrawing {
            region_id,
            nodes,
            lines,
            bounds: Rect::from_min_max(Pos2::new(left, top), Pos2::new(right, bottom)),
        })
    }
}

/// EVE's security colours, except null sec, which uses the mockup's red
/// (EVE's purple is too dark on the map's background).
pub fn security_color(security: f64) -> Color32 {
    let tenths = (security * 10.0).round() as i32;
    match tenths {
        10.. => Color32::from_rgb(0x2c, 0x75, 0xe1),
        9 => Color32::from_rgb(0x39, 0x9a, 0xeb),
        8 => Color32::from_rgb(0x4e, 0xce, 0xf8),
        7 => Color32::from_rgb(0x60, 0xdb, 0xa3),
        6 => Color32::from_rgb(0x71, 0xe7, 0x54),
        5 => Color32::from_rgb(0xf5, 0xff, 0x83),
        4 => Color32::from_rgb(0xdc, 0x6d, 0x07),
        3 => Color32::from_rgb(0xce, 0x44, 0x0f),
        2 => Color32::from_rgb(0xbb, 0x11, 0x16),
        1 => Color32::from_rgb(0xa5, 0x3a, 0x3a),
        _ => Color32::from_rgb(0xe0, 0x64, 0x5f),
    }
}

/// What the map shows besides the region itself.
pub struct MapContent<'a> {
    /// Own pilots by the system they are in.
    pub pilots: &'a HashMap<i64, Vec<String>>,
}

/// The map's camera and cached shapes. Lives in the module.
pub struct MapView {
    drawing: Option<RegionDrawing>,
    /// The map point at the centre of the view.
    center: Pos2,
    zoom: f32,
    /// Fit the region on the next frame (after the region changed, when the
    /// size of the view is known).
    fit_pending: bool,
    /// The view is still as "Fit region" left it (not panned or zoomed
    /// since), so it fits again when the panel changes size, e.g. while
    /// the window is still opening.
    fitted: bool,
    /// The size of the view last frame.
    last_size: Vec2,
}

impl Default for MapView {
    fn default() -> Self {
        MapView {
            drawing: None,
            center: Pos2::ZERO,
            zoom: 1.0,
            fit_pending: true,
            fitted: false,
            last_size: Vec2::ZERO,
        }
    }
}

impl MapView {
    pub fn region_id(&self) -> Option<i64> {
        self.drawing.as_ref().map(|d| d.region_id)
    }

    /// Shows another region, fitted to the view.
    pub fn show_region(&mut self, universe: &Universe, region_id: i64) {
        if self.region_id() == Some(region_id) {
            return;
        }
        self.drawing = RegionDrawing::new(universe, region_id);
        self.fit_pending = true;
    }

    /// Draws the map into all of `ui`'s available space.
    pub fn show(&mut self, ui: &mut egui::Ui, content: &MapContent) {
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect(
            rect,
            CornerRadius::same(6),
            BACKGROUND,
            Stroke::new(1.0, FRAME),
            egui::StrokeKind::Inside,
        );
        let Some(drawing) = &self.drawing else {
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                "Pick a region",
                FontId::proportional(13.0),
                LEGEND_TEXT,
            );
            return;
        };
        let bounds = drawing.bounds;

        let resized = rect.size() != self.last_size;
        self.last_size = rect.size();
        if self.fit_pending || (self.fitted && resized) {
            self.fit(bounds, rect);
            self.fit_pending = false;
        }

        // Pan by dragging.
        if response.dragged() {
            self.center -= response.drag_delta() / self.zoom;
            self.fitted = false;
        }
        // Zoom with the wheel (or a pinch), around the mouse pointer.
        if let Some(pointer) = response.hover_pos() {
            let (scroll, pinch) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
            let factor = pinch * (scroll / 400.0).exp();
            if factor != 1.0 {
                self.zoom_around(factor, pointer, rect);
                self.fitted = false;
            }
        }

        self.paint(&painter, rect, content);
        self.controls(ui, rect, bounds);
        legend(&painter, rect);
    }

    fn fit(&mut self, bounds: Rect, view: Rect) {
        // A little room around the edges.
        let margin = 24.0;
        let zoom_x = (view.width() - 2.0 * margin).max(50.0) / bounds.width().max(1.0);
        let zoom_y = (view.height() - 2.0 * margin).max(50.0) / bounds.height().max(1.0);
        self.zoom = zoom_x.min(zoom_y).clamp(MIN_ZOOM, MAX_ZOOM);
        self.center = bounds.center();
        self.fitted = true;
    }

    /// Zooms by `factor`, keeping the map point under `anchor` in place.
    fn zoom_around(&mut self, factor: f32, anchor: Pos2, view: Rect) {
        let before = self.to_map(anchor, view);
        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        let after = self.to_map(anchor, view);
        self.center += before - after;
    }

    fn to_screen(&self, map: Pos2, view: Rect) -> Pos2 {
        view.center() + (map - self.center) * self.zoom
    }

    fn to_map(&self, screen: Pos2, view: Rect) -> Pos2 {
        self.center + (screen - view.center()) / self.zoom
    }

    fn paint(&self, painter: &egui::Painter, view: Rect, content: &MapContent) {
        let Some(drawing) = &self.drawing else {
            return;
        };
        let zoom = self.zoom;
        // Whole points only (see the file comment).
        let name_size = (11.0 * zoom).round().clamp(8.0, 22.0);
        let small_size = (10.0 * zoom).round().clamp(7.0, 20.0);
        let line_width = (1.5 * zoom).clamp(1.0, 3.0);
        let dot_radius = (drawing::DOT_RADIUS + 1.0) * zoom.max(0.6);
        // Anything this far outside the view can't reach into it.
        // Text never shrinks below 8 pt, so labels are relatively wider
        // when zoomed out.
        let label_reach = 160.0 * zoom.max(1.0);
        let visible = view.expand(label_reach);

        // Gate lines first, so dots and labels cover them.
        for &(a, b, dashed) in &drawing.lines {
            let a = self.to_screen(a, view);
            let b = self.to_screen(b, view);
            if !view.intersects(Rect::from_two_pos(a, b)) {
                continue;
            }
            let stroke = Stroke::new(line_width, GATE);
            if dashed {
                painter.extend(Shape::dashed_line(&[a, b], stroke, 5.0 * zoom, 4.0 * zoom));
            } else {
                painter.line_segment([a, b], stroke);
            }
        }

        let show_labels = zoom >= LABELS_FROM_ZOOM;
        for node in &drawing.nodes {
            let center = self.to_screen(node.pos, view);
            if !visible.contains(center) {
                continue;
            }
            let pilots = content.pilots.get(&node.id);
            if node.border {
                painter.circle_stroke(center, dot_radius - 1.0, Stroke::new(2.0, IDLE_DOT));
            } else if pilots.is_some() {
                // A soft ring, then the blue dot.
                painter.circle_filled(
                    center,
                    dot_radius + 3.0 * zoom.max(0.6),
                    PILOT_BLUE.gamma_multiply(0.27),
                );
                painter.circle_filled(center, dot_radius, PILOT_BLUE);
            } else {
                painter.circle_filled(center, dot_radius, IDLE_DOT);
            }
            if !show_labels {
                continue;
            }

            // The name on its dark chip, then the security (or region).
            let label_pos =
                center + Vec2::new(drawing::LABEL_OFFSET[0], drawing::LABEL_OFFSET[1]) * zoom;
            let name_color = if node.border { OTHER_REGION_NAME } else { NAME };
            let galley =
                painter.layout_no_wrap(node.name.clone(), FontId::monospace(name_size), name_color);
            let chip = Rect::from_min_size(label_pos, galley.size()).expand2(Vec2::new(2.0, 0.0));
            painter.rect_filled(chip, CornerRadius::same(2), LABEL_FILL);
            let name_width = galley.size().x;
            let line_height = galley.size().y;
            painter.galley(label_pos, galley, name_color);
            painter.text(
                label_pos + Vec2::new(name_width + 5.0, line_height / 2.0),
                Align2::LEFT_CENTER,
                &node.detail,
                FontId::proportional(small_size),
                node.detail_color,
            );
            if let Some(pilots) = pilots {
                painter.text(
                    label_pos + Vec2::new(0.0, line_height + 1.0),
                    Align2::LEFT_TOP,
                    pilots.join(", "),
                    FontId::proportional(small_size),
                    PILOT_TEXT,
                );
            }
        }
    }

    /// The zoom buttons and "Fit region", bottom right.
    fn controls(&mut self, ui: &mut egui::Ui, view: Rect, bounds: Rect) {
        let size = Vec2::new(150.0, 28.0);
        let area = Rect::from_min_size(view.right_bottom() - size - Vec2::splat(12.0), size);
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(area)
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        if child.button("Fit region").clicked() {
            self.fit(bounds, view);
        }
        if child.button(" + ").on_hover_text("Zoom in").clicked() {
            self.zoom_around(ZOOM_STEP, view.center(), view);
            self.fitted = false;
        }
        if child.button(" − ").on_hover_text("Zoom out").clicked() {
            self.zoom_around(1.0 / ZOOM_STEP, view.center(), view);
            self.fitted = false;
        }
    }
}

/// The colour key, bottom left. Report colours join it with the intel list.
fn legend(painter: &egui::Painter, view: Rect) {
    let font = FontId::proportional(11.0);
    let mut x = view.left() + 14.0;
    let y = view.bottom() - 20.0;
    let entries: [(&str, bool); 2] = [("My pilot", true), ("Other region", false)];
    for (text, filled) in entries {
        let dot = Pos2::new(x + 5.0, y);
        if filled {
            painter.circle_filled(dot, 5.0, PILOT_BLUE);
        } else {
            painter.circle_stroke(dot, 4.0, Stroke::new(2.0, IDLE_DOT));
        }
        let rect = painter.text(
            Pos2::new(x + 14.0, y),
            Align2::LEFT_CENTER,
            text,
            font.clone(),
            LEGEND_TEXT,
        );
        x = rect.right() + 12.0;
    }
    painter.text(
        Pos2::new(x, y),
        Align2::LEFT_CENTER,
        "· drag to pan, wheel to zoom",
        font,
        OTHER_REGION_TEXT,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn universe() -> Universe {
        crate::static_data::tests::sample().universe
    }

    #[test]
    fn region_drawing_has_systems_borders_and_gates() {
        let drawing = RegionDrawing::new(&universe(), 1).unwrap();
        // A1..A4 plus B1 from region Beta.
        assert_eq!(drawing.nodes.len(), 5);
        let b1 = drawing.nodes.iter().find(|n| n.id == 201).unwrap();
        assert!(b1.border);
        assert_eq!(b1.detail, "Beta");
        // A1-A2, A2-A3, A3-A4 solid; A2-B1 dashed. Each gate once.
        assert_eq!(drawing.lines.iter().filter(|l| !l.2).count(), 3);
        assert_eq!(drawing.lines.iter().filter(|l| l.2).count(), 1);
        assert!(RegionDrawing::new(&universe(), 999).is_none());
    }

    #[test]
    fn zoom_keeps_the_point_under_the_pointer() {
        let mut view = MapView::default();
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
        let pointer = Pos2::new(100.0, 450.0);
        let before = view.to_map(pointer, screen);
        view.zoom_around(1.5, pointer, screen);
        let after = view.to_map(pointer, screen);
        assert!((before - after).length() < 1e-3);
        assert_eq!(view.zoom, 1.5);
        // Clamped.
        view.zoom_around(100.0, pointer, screen);
        assert_eq!(view.zoom, MAX_ZOOM);
    }

    #[test]
    fn fit_shows_the_whole_region() {
        let mut view = MapView::default();
        let bounds = Rect::from_min_max(Pos2::new(-10.0, -10.0), Pos2::new(910.0, 790.0));
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 800.0));
        view.fit(bounds, screen);
        let top_left = view.to_screen(bounds.min, screen);
        let bottom_right = view.to_screen(bounds.max, screen);
        assert!(screen.contains(top_left) && screen.contains(bottom_right));
    }

    #[test]
    fn security_colours() {
        assert_eq!(security_color(1.0), Color32::from_rgb(0x2c, 0x75, 0xe1));
        assert_eq!(security_color(0.5), Color32::from_rgb(0xf5, 0xff, 0x83));
        assert_eq!(security_color(-0.4), security_color(0.0));
    }
}
