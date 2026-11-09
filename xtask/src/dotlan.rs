//! Reads DOTLAN EveMaps' region maps, for the Intel map's layout.
//!
//! Each region map is an SVG from `https://evemaps.dotlan.net/svg/<Region>.svg`
//! (spaces in the name written as `_`). CI downloads them with
//! `xtask/dotlan.ps1` into a `dotlan/` folder; they are © Wollari and are
//! never committed. Every system is drawn as a box like this, on a
//! 1024 × 768 canvas:
//!
//! ```text
//! <use id="sys30004382" x="5" y="310" width="62.5" height="30" xlink:href="#def30004382" />
//! ```
//!
//! We only need the system id and the box's top-left corner, so this finds
//! each `<use id="sys` tag with plain string search instead of parsing the
//! whole SVG (no XML crate needed). Systems of neighbouring regions that a
//! gate leads to have boxes too, at the edge of the map; `map.rs` decides
//! which boxes belong to which region, using the SDE.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, bail};

/// The top-left corner of every system box in one region's map, by system id.
pub type Boxes = BTreeMap<i64, (f64, f64)>;

/// What starts a system box in Dotlan's SVG.
const BOX_START: &str = "<use id=\"sys";

/// Dotlan's file name for a region: "Cloud Ring" becomes "Cloud_Ring.svg".
pub fn file_name(region_name: &str) -> String {
    format!("{}.svg", region_name.replace(' ', "_"))
}

/// Finds every system box in one SVG.
///
/// A box tag that is there but can't be read (no number where one should
/// be) is an error, so a changed format fails the build loudly.
pub fn read_boxes(svg: &str) -> Result<Boxes> {
    let mut boxes = Boxes::new();
    let mut rest = svg;
    while let Some(start) = rest.find(BOX_START) {
        let after_start = &rest[start + BOX_START.len()..];
        // The tag ends at the next '>'. Attribute values in these files
        // never contain one.
        let Some(end) = after_start.find('>') else {
            bail!("a system box tag never ends");
        };
        // From the digits of the id (`30004382" x="5" ...`) to the tag's end.
        let tag = &after_start[..end];
        let id_text = tag.split('"').next().unwrap_or_default();
        let id: i64 = id_text
            .parse()
            .with_context(|| format!("system box id \"sys{id_text}\" is not a number"))?;
        let x = attribute(tag, "x").with_context(|| format!("system box sys{id}"))?;
        let y = attribute(tag, "y").with_context(|| format!("system box sys{id}"))?;
        boxes.insert(id, (x, y));
        rest = &after_start[end..];
    }
    Ok(boxes)
}

/// Reads a number attribute such as ` x="5"` from inside one tag.
fn attribute(tag: &str, name: &str) -> Result<f64> {
    // The leading space makes sure ` x="` doesn't match the end of another
    // attribute's name.
    let pattern = format!(" {name}=\"");
    let Some(start) = tag.find(&pattern) else {
        bail!("no {name} attribute");
    };
    let value_and_rest = &tag[start + pattern.len()..];
    let value = value_and_rest.split('"').next().unwrap_or_default();
    value
        .trim()
        .parse()
        .with_context(|| format!("{name}=\"{value}\" is not a number"))
}

/// Reads the map of every region in `regions` (id → name) from `folder`.
///
/// Returns `None`, with a warning, when the folder doesn't exist (Dotlan was
/// down and nothing was cached), so the whole map falls back to
/// `position2D`. A region whose file is missing is left out, with a warning,
/// and falls back on its own. A file that is there but has no system boxes
/// (Dotlan changed its format, or an error page was saved as `.svg`) fails
/// the build.
pub fn load(
    folder: &Path,
    regions: &BTreeMap<i64, String>,
) -> Result<Option<BTreeMap<i64, Boxes>>> {
    if !folder.is_dir() {
        println!(
            "warning: no Dotlan folder at {}; the Intel map uses CCP's position2D layout",
            folder.display()
        );
        return Ok(None);
    }
    let mut maps = BTreeMap::new();
    for (&region_id, name) in regions {
        let path = folder.join(file_name(name));
        if !path.is_file() {
            println!(
                "warning: {} is missing; {name} uses CCP's position2D layout",
                path.display()
            );
            continue;
        }
        let svg = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let boxes = read_boxes(&svg).with_context(|| format!("reading {}", path.display()))?;
        if boxes.is_empty() {
            bail!(
                "{} has no system boxes; has Dotlan's map format changed? \
                 (Delete the file to fall back to position2D for {name}.)",
                path.display()
            );
        }
        maps.insert(region_id, boxes);
    }
    Ok(Some(maps))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hand-written in the shape of a Dotlan region map (not a real one:
    /// those are © Wollari and never committed). Three systems of this
    /// region and one of a neighbouring region.
    const SAMPLE: &str = r##"<?xml version="1.0"?>
<svg version="1.1" id="svgdoc" width="1024" height="768" viewBox="0 0 1024 768">
<defs>
<symbol id="def1001"><rect x="4" y="3.5" width="50" height="22" /></symbol>
</defs>
<g id="sysuse">
<use id="sys1001" x="5" y="310" width="62.5" height="30" xlink:href="#def1001" />
<use id="sys1002" x="10" y="30" width="62.5" height="30" xlink:href="#def1002" />
<use id="sys1003" x="122.5" y="80.25" width="62.5" height="30" xlink:href="#def1003" />
<use id="sys2001" x="900" y="700" width="62.5" height="30" xlink:href="#def2001" />
</g>
<use id="legend" x="1" y="1" />
</svg>
"##;

    #[test]
    fn reads_every_system_box() {
        let boxes = read_boxes(SAMPLE).unwrap();
        assert_eq!(boxes.len(), 4);
        assert_eq!(boxes[&1001], (5.0, 310.0));
        assert_eq!(boxes[&1002], (10.0, 30.0));
        assert_eq!(boxes[&1003], (122.5, 80.25));
        assert_eq!(boxes[&2001], (900.0, 700.0));
    }

    #[test]
    fn a_page_without_boxes_gives_none() {
        let boxes = read_boxes("<html><body>502 Bad Gateway</body></html>").unwrap();
        assert!(boxes.is_empty());
    }

    #[test]
    fn a_broken_box_is_an_error() {
        assert!(read_boxes(r#"<use id="sysABC" x="1" y="2" />"#).is_err());
        assert!(read_boxes(r#"<use id="sys1001" x="left" y="2" />"#).is_err());
        assert!(read_boxes(r#"<use id="sys1001" y="2" />"#).is_err());
        assert!(read_boxes(r#"<use id="sys1001" x="1" y="2""#).is_err());
    }

    #[test]
    fn width_is_not_read_as_x() {
        // ` x="` needs the space, so `width="` or `xlink:href` never match.
        let boxes = read_boxes(r#"<use id="sys7" width="62.5" y="4" x="3" />"#).unwrap();
        assert_eq!(boxes[&7], (3.0, 4.0));
    }

    #[test]
    fn file_names_use_underscores() {
        assert_eq!(file_name("Branch"), "Branch.svg");
        assert_eq!(file_name("Vale of the Silent"), "Vale_of_the_Silent.svg");
        assert_eq!(file_name("UUA-F4"), "UUA-F4.svg");
    }

    fn temp_folder(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("athena-xtask-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn load_skips_missing_files_and_fails_on_empty_ones() {
        let regions = BTreeMap::from([(1, "Cloud Ring".to_string()), (2, "Branch".to_string())]);

        // No folder at all: just a warning.
        let missing = std::env::temp_dir().join("athena-xtask-no-such-folder");
        assert!(load(&missing, &regions).unwrap().is_none());

        // One file there, one missing: the missing region is left out.
        let dir = temp_folder("load");
        std::fs::write(dir.join("Cloud_Ring.svg"), SAMPLE).unwrap();
        let maps = load(&dir, &regions).unwrap().unwrap();
        assert_eq!(maps.len(), 1);
        assert_eq!(maps[&1].len(), 4);

        // A file with no boxes fails the build.
        std::fs::write(dir.join("Branch.svg"), "<html>error</html>").unwrap();
        assert!(load(&dir, &regions).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
