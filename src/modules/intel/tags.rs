//! System tags: short labels under a system's name on the map, such as
//! "POS" or "M3", each with a colour, an icon and a group.
//!
//! They use RIFT's text format, one tag per line, so a RIFT export can be
//! pasted straight in:
//!
//! ```text
//! 4DTQ-K;POS;#F5FF83;structure_skyhook;false;POS
//! ```
//!
//! The fields are system, label, colour, icon, a flag Athena doesn't use,
//! and group. Fields Athena doesn't use (the flag, unknown icon names, the
//! colour exactly as written, any extra fields) are kept and written back
//! unchanged. Tags are saved in `intel_tags.txt` in the data folder, in the
//! same format. Lines of that file Athena can't read (say, a misspelt
//! system) are kept as they are and written back at the end, so saving
//! never loses anything.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use eframe::egui::{self, Color32, FontId, Pos2, Stroke};

use crate::universe::Universe;

const FILE_NAME: &str = "intel_tags.txt";
/// Colour for a tag whose colour can't be read.
const DEFAULT_COLOR: Color32 = Color32::from_rgb(0xdc, 0xdc, 0xdc);

/// The icons Athena draws itself; anything else gets a plain dot.
pub const ICONS: [(&str, &str); 4] = [
    ("resources_isk", "ISK"),
    ("mining_diamond", "Diamond"),
    ("structure_skyhook", "Structure"),
    ("dot", "Dot"),
];

/// One tag on one system.
#[derive(Debug, Clone, PartialEq)]
pub struct Tag {
    /// The system's id, found from its name when the tag was read.
    pub system_id: i64,
    /// The name as written in the file.
    pub system_name: String,
    pub label: String,
    /// The colour for drawing.
    pub color: Color32,
    /// The colour as written ("#F5FF83", or empty), written back as is.
    pub color_text: String,
    /// RIFT's icon name, such as "structure_skyhook".
    pub icon: String,
    /// RIFT's fifth field ("false"), kept as it was.
    pub flag: String,
    pub group: String,
    /// Any fields after the group, kept as they were.
    pub extra: Vec<String>,
}

impl Tag {
    /// The line for this tag in RIFT's format.
    pub fn to_line(&self) -> String {
        let mut fields = vec![
            self.system_name.clone(),
            self.label.clone(),
            self.color_text.clone(),
            self.icon.clone(),
            self.flag.clone(),
            self.group.clone(),
        ];
        fields.extend(self.extra.iter().cloned());
        fields.join(";")
    }

    /// Same system and label: a second copy of the same tag.
    pub fn same_as(&self, other: &Tag) -> bool {
        self.system_id == other.system_id && self.label.eq_ignore_ascii_case(&other.label)
    }
}

/// What reading a block of tag text found.
#[derive(Debug, Default, PartialEq)]
pub struct ParsedTags {
    pub tags: Vec<Tag>,
    /// Lines that aren't tags or name a system that isn't on the map,
    /// exactly as they were.
    pub unread: Vec<String>,
}

/// Reads tags in RIFT's format, one per line. Empty lines are ignored.
pub fn parse(text: &str, universe: &Universe) -> ParsedTags {
    let mut parsed = ParsedTags::default();
    // A file saved by Notepad can start with a byte order mark.
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        match parse_line(line, universe) {
            Some(tag) => parsed.tags.push(tag),
            None => parsed.unread.push(raw.trim_end().to_string()),
        }
    }
    parsed
}

fn parse_line(line: &str, universe: &Universe) -> Option<Tag> {
    let fields: Vec<&str> = line.split(';').map(str::trim).collect();
    let field = |i: usize| fields.get(i).copied().unwrap_or_default().to_string();
    let system = universe.system_by_name(fields.first()?)?;
    let label = field(1);
    if label.is_empty() {
        return None;
    }
    Some(Tag {
        system_id: system.id,
        system_name: system.name.clone(),
        label,
        color: parse_color(&field(2)).unwrap_or(DEFAULT_COLOR),
        color_text: field(2),
        icon: field(3),
        flag: if fields.len() > 4 {
            field(4)
        } else {
            "false".to_string()
        },
        group: field(5),
        extra: fields.iter().skip(6).map(|f| f.to_string()).collect(),
    })
}

/// "#F5FF83" (or without the #) to a colour.
pub fn parse_color(text: &str) -> Option<Color32> {
    let hex = text.trim().trim_start_matches('#');
    if hex.len() != 6 {
        return None;
    }
    let value = u32::from_str_radix(hex, 16).ok()?;
    Some(Color32::from_rgb(
        (value >> 16) as u8,
        (value >> 8) as u8,
        value as u8,
    ))
}

/// A colour as "#F5FF83".
pub fn color_text(color: Color32) -> String {
    format!("#{:02X}{:02X}{:02X}", color.r(), color.g(), color.b())
}

/// All tags as text, one line each, then the `unread` lines unchanged.
pub fn format(tags: &[Tag], unread: &[String]) -> String {
    let mut text: String = tags.iter().map(|t| t.to_line() + "\n").collect();
    for line in unread {
        text.push_str(line);
        text.push('\n');
    }
    if text.is_empty() {
        text.push('\n');
    }
    text
}

/// Adds `new` tags to `tags`, skipping copies of tags already there.
/// Returns how many were added.
pub fn merge(tags: &mut Vec<Tag>, new: Vec<Tag>) -> usize {
    let mut added = 0;
    for tag in new {
        if !tags.iter().any(|t| t.same_as(&tag)) {
            tags.push(tag);
            added += 1;
        }
    }
    tags.sort_by(|a, b| {
        a.system_name
            .cmp(&b.system_name)
            .then_with(|| a.label.cmp(&b.label))
    });
    added
}

/// The groups in use, sorted, without duplicates. Tags without a group
/// count as "" (shown as "No group").
pub fn groups(tags: &[Tag]) -> Vec<String> {
    let mut groups: Vec<String> = tags.iter().map(|t| t.group.clone()).collect();
    groups.sort();
    groups.dedup();
    groups
}

/// The tags to draw on the map, by system, leaving out hidden groups.
pub fn by_system<'a>(tags: &'a [Tag], hidden_groups: &[String]) -> HashMap<i64, Vec<&'a Tag>> {
    let mut by_system: HashMap<i64, Vec<&Tag>> = HashMap::new();
    for tag in tags {
        if !hidden_groups.contains(&tag.group) {
            by_system.entry(tag.system_id).or_default().push(tag);
        }
    }
    by_system
}

/// The colour last used in a group, as the starting colour for a new tag.
pub fn group_color(tags: &[Tag], group: &str) -> Option<Color32> {
    tags.iter()
        .rev()
        .find(|t| t.group == group)
        .map(|t| t.color)
}

pub fn file_path(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE_NAME)
}

/// Reads the saved tags. Runs once at startup, before the window shows.
/// A missing file is no tags. Any other read error is returned, and then
/// the file must not be overwritten.
pub fn load(data_dir: &Path, universe: &Universe) -> Result<ParsedTags, String> {
    let path = file_path(data_dir);
    match std::fs::read(&path) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => Ok(parse(&text, universe)),
            Err(_) => Err(format!("{} isn't UTF-8 text.", path.display())),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ParsedTags::default()),
        Err(e) => Err(format!("Can't read {}: {e}", path.display())),
    }
}

/// Draws a tag's icon centred on `center`, `size` points tall: "Ƶ" for
/// ISK, a diamond, a triangle for structures, otherwise a dot. The shapes
/// are painted rather than taken from a font, so they look the same
/// whatever fonts are installed.
pub fn paint_icon(painter: &egui::Painter, center: Pos2, size: f32, icon: &str, color: Color32) {
    let r = size * 0.4;
    match icon {
        "resources_isk" => {
            painter.text(
                center,
                egui::Align2::CENTER_CENTER,
                "Ƶ",
                FontId::proportional(size),
                color,
            );
        }
        "mining_diamond" => {
            let points = vec![
                center + egui::vec2(0.0, -r),
                center + egui::vec2(r * 0.8, 0.0),
                center + egui::vec2(0.0, r),
                center + egui::vec2(-r * 0.8, 0.0),
            ];
            painter.add(egui::Shape::convex_polygon(points, color, Stroke::NONE));
        }
        "structure_skyhook" => {
            let points = vec![
                center + egui::vec2(0.0, -r),
                center + egui::vec2(r, r * 0.8),
                center + egui::vec2(-r, r * 0.8),
            ];
            painter.add(egui::Shape::convex_polygon(points, color, Stroke::NONE));
        }
        _ => {
            painter.circle_filled(center, r * 0.7, color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn universe() -> Universe {
        crate::static_data::tests::sample().universe
    }

    #[test]
    fn reads_rift_lines_and_writes_them_back_unchanged() {
        let text = "a1-xyz;POS;#F5FF83;structure_skyhook;false;POS\n\
                    \n\
                    B1;M3;#71e754;resources_isk;true;Ratting;more;fields\n\
                    Nowhere;POS;#FFFFFF;dot;false;POS\n\
                    A2-XYZ;;#FFFFFF;dot;false;POS\n";
        let parsed = parse(text, &universe());
        assert_eq!(
            parsed.unread,
            [
                "Nowhere;POS;#FFFFFF;dot;false;POS",
                "A2-XYZ;;#FFFFFF;dot;false;POS"
            ]
        );
        // Saving writes the unread lines back at the end.
        let saved = format(&parsed.tags, &parsed.unread);
        assert!(
            saved.ends_with("Nowhere;POS;#FFFFFF;dot;false;POS\nA2-XYZ;;#FFFFFF;dot;false;POS\n")
        );
        let [pos, m3] = &parsed.tags[..] else {
            panic!("two tags expected");
        };
        assert_eq!(pos.system_id, 101);
        assert_eq!(pos.color, Color32::from_rgb(0xF5, 0xFF, 0x83));
        // The system name is written the way the map spells it.
        assert_eq!(
            pos.to_line(),
            "A1-XYZ;POS;#F5FF83;structure_skyhook;false;POS"
        );
        // The colour keeps its spelling.
        assert_eq!(
            m3.to_line(),
            "B1;M3;#71e754;resources_isk;true;Ratting;more;fields"
        );
    }

    #[test]
    fn short_lines_get_defaults() {
        let parsed = parse("A1-XYZ;Ice", &universe());
        let tag = &parsed.tags[0];
        assert_eq!(tag.color, DEFAULT_COLOR);
        assert_eq!(tag.to_line(), "A1-XYZ;Ice;;;false;");
        // A byte order mark at the start is ignored.
        assert_eq!(parse("\u{feff}A1-XYZ;Ice", &universe()).tags.len(), 1);
    }

    #[test]
    fn merging_skips_copies_and_sorts() {
        let u = universe();
        let mut tags = parse("B1;M3;#FFFFFF;dot;false;Ratting", &u).tags;
        let added = merge(
            &mut tags,
            parse(
                "b1;m3;#000000;dot;false;X\nA1-XYZ;POS;#FFFFFF;dot;false;POS",
                &u,
            )
            .tags,
        );
        assert_eq!(added, 1);
        assert_eq!(tags[0].system_name, "A1-XYZ");
        assert_eq!(groups(&tags), ["POS", "Ratting"]);
        assert_eq!(group_color(&tags, "POS"), Some(Color32::WHITE));
    }

    #[test]
    fn colours() {
        assert_eq!(
            parse_color("#4ECEF8"),
            Some(Color32::from_rgb(0x4e, 0xce, 0xf8))
        );
        assert_eq!(
            parse_color("4ecef8"),
            Some(Color32::from_rgb(0x4e, 0xce, 0xf8))
        );
        assert_eq!(parse_color("#FFF"), None);
        assert_eq!(color_text(Color32::from_rgb(1, 2, 255)), "#0102FF");
    }
}
