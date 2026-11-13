//! Small widgets shared by several modules.

use eframe::egui::{Button, Response, Stroke, Ui};

/// Adds a button with its own border colour that keeps the same size when
/// the mouse is over it or it is pressed.
///
/// Why not just `Button::stroke`? egui sizes a button's padding from the
/// border width of the theme's *current* state (no border when idle, 1 px
/// when hovered), then draws our border on top. With our own 1 px border the
/// button is 1 px bigger on each side while idle than while hovered, so it
/// jumps and pushes its neighbours around. Here every state of the theme gets
/// our border for this one button, so egui's padding maths always matches it.
pub fn add_outlined(ui: &mut Ui, enabled: bool, button: Button, stroke: Stroke) -> Response {
    // `scope` gives a child `Ui` with its own copy of the style, so the
    // change below only affects this button.
    ui.scope(|ui| {
        let widgets = &mut ui.visuals_mut().widgets;
        for state in [
            &mut widgets.noninteractive,
            &mut widgets.inactive,
            &mut widgets.hovered,
            &mut widgets.active,
            &mut widgets.open,
        ] {
            state.bg_stroke = stroke;
        }
        ui.add_enabled(enabled, button)
    })
    .inner
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui;
    use eframe::egui::{Context, Event, RawInput, Rect, pos2, vec2};

    /// Draws one button for a few frames, with the mouse at `mouse`, and
    /// returns where it ended up.
    fn button_rect(mouse: Option<egui::Pos2>, add: &dyn Fn(&mut Ui) -> Response) -> Rect {
        let ctx = Context::default();
        let mut rect = Rect::NOTHING;
        // egui picks the hover look from the previous frame's response, so
        // run a few frames to let it settle.
        for _ in 0..4 {
            let mut input = RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 200.0))),
                ..Default::default()
            };
            if let Some(pos) = mouse {
                input.events.push(Event::PointerMoved(pos));
            }
            let mut output = ctx.run_ui(input, |ui| {
                rect = add(ui).rect;
            });
            // No renderer here to take the font textures, so drop them.
            output.textures_delta.clear();
        }
        rect
    }

    fn outlined(ui: &mut Ui) -> Response {
        add_outlined(
            ui,
            true,
            Button::new("Mining"),
            Stroke::new(1.0, egui::Color32::RED),
        )
    }

    fn plain_stroke(ui: &mut Ui) -> Response {
        ui.add(Button::new("Mining").stroke(Stroke::new(1.0, egui::Color32::RED)))
    }

    #[test]
    fn outlined_button_keeps_its_size_on_hover() {
        let idle = button_rect(None, &outlined);
        let hovered = button_rect(Some(idle.center()), &outlined);
        assert_eq!(idle, hovered);
    }

    #[test]
    fn plain_stroke_button_changes_size_on_hover() {
        // Shows the egui behaviour `add_outlined` works around. If this
        // starts failing after an egui update, the workaround may be gone.
        let idle = button_rect(None, &plain_stroke);
        let hovered = button_rect(Some(idle.center()), &plain_stroke);
        assert_ne!(idle, hovered);
    }
}
