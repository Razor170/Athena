//! The Welcome tab: a short guide to Athena and each module.
//!
//! The text lives in `guide.rs`. This file only draws it.

use eframe::egui::{self, Color32, FontId, RichText, text::LayoutJob, text::TextFormat};

use super::{Module, ModuleContext};

mod guide;

/// The guide stays readable on a wide tab by never getting wider than this.
const MAX_WIDTH: f32 = 640.0;
/// Room for the "1." or "•" in front of a list item.
const MARKER_WIDTH: f32 = 18.0;
const DIM: Color32 = Color32::from_rgb(0x9a, 0x9a, 0x9a);
/// Background behind button names such as "[End session]".
const BUTTON_BG: Color32 = Color32::from_rgb(0x3c, 0x3c, 0x3c);

pub struct Welcome;

impl Module for Welcome {
    fn id(&self) -> &'static str {
        "welcome"
    }

    fn title(&self) -> &'static str {
        "Welcome"
    }

    fn ui(&mut self, ui: &mut egui::Ui, ctx: &ModuleContext) {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // The scroll area's own content is as wide as the tab, so
                // narrow a child area instead (`scope` starts it empty).
                ui.scope(|ui| {
                    ui.set_max_width(MAX_WIDTH);
                    draw_guide(ui);
                    ui.separator();
                    ui.label(
                        RichText::new(format!(
                            "EVE log folder: {} · Version {}",
                            ctx.settings.log_folder.display(),
                            crate::version::display_text()
                        ))
                        .color(DIM),
                    );
                });
            });
    }
}

fn draw_guide(ui: &mut egui::Ui) {
    ui.heading("Welcome to Athena");
    rich_paragraph(ui, guide::INTRO);

    ui.add_space(8.0);
    subheading(ui, "Getting started");
    for (index, step) in guide::GETTING_STARTED.iter().enumerate() {
        list_item(ui, &format!("{}.", index + 1), step);
    }

    ui.add_space(8.0);
    subheading(ui, "Tabs and windows");
    for item in guide::TABS_AND_WINDOWS {
        list_item(ui, "•", item);
    }

    ui.add_space(4.0);
    ui.separator();
    for section in guide::SECTIONS {
        // The title in bold and the subtitle dimmed, on the header line.
        let mut header = LayoutJob::default();
        header.append(
            section.title,
            0.0,
            TextFormat::simple(FontId::proportional(14.0), ui.visuals().strong_text_color()),
        );
        if !section.subtitle.is_empty() {
            header.append(
                section.subtitle,
                8.0,
                TextFormat::simple(FontId::proportional(13.0), DIM),
            );
        }
        // `id_salt` keeps each header's open/closed state apart even if two
        // titles were ever the same. Sections start closed.
        egui::CollapsingHeader::new(header)
            .id_salt(section.title)
            .default_open(false)
            .show(ui, |ui| {
                for paragraph in section.paragraphs {
                    rich_paragraph(ui, paragraph);
                    ui.add_space(2.0);
                }
                if !section.note.is_empty() {
                    ui.label(RichText::new(section.note).color(DIM));
                }
            });
    }
}

fn subheading(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(16.0).strong());
}

/// One numbered or bulleted line, with the text wrapping under itself.
fn list_item(ui: &mut egui::Ui, marker: &str, text: &str) {
    let text_width = ui.available_width() - MARKER_WIDTH - ui.spacing().item_spacing.x;
    ui.horizontal_top(|ui| {
        ui.add_sized([MARKER_WIDTH, 0.0], egui::Label::new(marker));
        // Labels in a row don't wrap, so give the text its own top-down
        // area of a fixed width to wrap in.
        ui.allocate_ui_with_layout(
            egui::vec2(text_width, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| rich_paragraph(ui, text),
        );
    });
}

/// Draws text with the guide's markers (see `guide.rs`), wrapped to the
/// available width.
fn rich_paragraph(ui: &mut egui::Ui, text: &str) {
    let normal = ui.visuals().text_color();
    let strong = ui.visuals().strong_text_color();
    let font = FontId::proportional(14.0);

    let mut job = LayoutJob::default();
    job.wrap.max_width = ui.available_width();
    for piece in parse(text) {
        let (shown, format) = match piece {
            Piece::Plain(s) => (s, TextFormat::simple(font.clone(), normal)),
            Piece::Bold(s) => (s, TextFormat::simple(font.clone(), strong)),
            Piece::Button(s) => (
                // Spaces inside the background read as button padding.
                format!(" {s} "),
                TextFormat {
                    background: BUTTON_BG,
                    ..TextFormat::simple(FontId::proportional(13.0), strong)
                },
            ),
            Piece::Swatch(color) => ("■".to_string(), TextFormat::simple(font.clone(), color)),
        };
        job.append(&shown, 0.0, format);
    }
    ui.label(job);
}

/// A run of guide text with one look.
#[derive(Debug, PartialEq)]
enum Piece {
    Plain(String),
    Bold(String),
    Button(String),
    Swatch(Color32),
}

/// Splits guide text at its markers. Text after a marker that is never
/// closed stays plain, so a typo shows up in the tab instead of vanishing.
fn parse(text: &str) -> Vec<Piece> {
    let mut pieces = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        // Find the earliest marker that has a matching end.
        let next = [("**", "**"), ("[", "]"), ("{", "}")]
            .iter()
            .filter_map(|(open, close)| {
                let start = rest.find(open)?;
                let after = start + open.len();
                let len = rest[after..].find(close)?;
                Some((start, after, after + len, close.len(), *open))
            })
            .min_by_key(|found| found.0);

        let Some((start, inner_start, inner_end, close_len, open)) = next else {
            pieces.push(Piece::Plain(rest.to_string()));
            break;
        };
        if start > 0 {
            pieces.push(Piece::Plain(rest[..start].to_string()));
        }
        let inner = &rest[inner_start..inner_end];
        pieces.push(match open {
            "**" => Piece::Bold(inner.to_string()),
            "[" => Piece::Button(inner.to_string()),
            _ => match swatch_color(inner) {
                Some(color) => Piece::Swatch(color),
                None => Piece::Plain(format!("{{{inner}}}")),
            },
        });
        rest = &rest[inner_end + close_len..];
    }
    pieces
}

/// The mining hold bar colours, by name (same values as in `mining/ui.rs`).
fn swatch_color(name: &str) -> Option<Color32> {
    match name {
        "green" => Some(Color32::from_rgb(0x5c, 0xbf, 0x6a)),
        "purple" => Some(Color32::from_rgb(0xa3, 0x7e, 0xf0)),
        "yellow" => Some(Color32::from_rgb(0xe8, 0xc5, 0x47)),
        "red" => Some(Color32::from_rgb(0xe5, 0x53, 0x4b)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(s: &str) -> Piece {
        Piece::Plain(s.to_string())
    }

    #[test]
    fn plain_text_stays_whole() {
        assert_eq!(parse("Just text."), vec![plain("Just text.")]);
    }

    #[test]
    fn markers_are_split_out() {
        assert_eq!(
            parse("**Live** has [Emptied] and {red} full"),
            vec![
                Piece::Bold("Live".into()),
                plain(" has "),
                Piece::Button("Emptied".into()),
                plain(" and "),
                Piece::Swatch(swatch_color("red").unwrap()),
                plain(" full"),
            ]
        );
    }

    #[test]
    fn unclosed_or_unknown_markers_stay_visible() {
        assert_eq!(parse("a [b"), vec![plain("a [b")]);
        assert_eq!(parse("{blue}"), vec![plain("{blue}")]);
    }

    #[test]
    fn every_guide_text_parses_without_stray_markers() {
        let mut texts: Vec<&str> = vec![guide::INTRO];
        texts.extend(guide::GETTING_STARTED);
        texts.extend(guide::TABS_AND_WINDOWS);
        for section in guide::SECTIONS {
            texts.extend(section.paragraphs);
        }
        for text in texts {
            for piece in parse(text) {
                if let Piece::Plain(s) = piece {
                    assert!(
                        !s.contains("**") && !s.contains('{') && !s.contains('['),
                        "stray marker in: {text}"
                    );
                }
            }
        }
    }
}
