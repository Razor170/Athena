//! Saved fits: the fits stored with "Save…" and picked again with
//! "Saved…". Each one keeps its name, ship type, EFT text and when it was
//! last changed. They live in `fittings_saved.toml` in the data folder,
//! as EFT text, so the file can be read and backed up by hand.
//!
//! A fit is known by its name *and* ship type: "PvE" on a Raven and "PvE"
//! on a Golem are two fits. Saving under a name and type that exist
//! replaces that fit.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const FILE_NAME: &str = "fittings_saved.toml";

/// One saved fit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedFit {
    pub name: String,
    /// The ship type, such as "Thanatos" (the EFT header's hull).
    pub hull: String,
    /// The fit as EFT text.
    pub text: String,
    /// When it was saved or last replaced or renamed (Unix seconds).
    pub changed: i64,
}

/// How the "Saved fits" list is sorted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortBy {
    Name,
    Hull,
    /// Newest change first.
    #[default]
    Changed,
}

/// All saved fits, as stored in `fittings_saved.toml`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Library {
    pub fits: Vec<SavedFit>,
    /// True when the file couldn't be read *and* couldn't be backed up:
    /// then nothing may be written, or the old fits would be lost.
    #[serde(skip)]
    pub read_only: bool,
}

impl Library {
    pub fn file_path(data_dir: &Path) -> PathBuf {
        data_dir.join(FILE_NAME)
    }

    /// Reads the saved fits. A missing file is normal (nothing saved yet).
    /// Any other problem (unreadable or broken file) gives an empty list
    /// and a message, and the file is first copied aside to
    /// `fittings_saved.broken-<unix>.toml`, so the next save can't wipe
    /// fits made by hand. If even that copy fails, the list stays
    /// read-only until Athena restarts.
    pub fn load(data_dir: &Path, now: i64) -> (Library, Option<String>) {
        let path = Library::file_path(data_dir);
        let problem = match std::fs::read_to_string(&path) {
            Ok(text) => match toml::from_str::<Library>(&text) {
                Ok(library) => return (library, None),
                Err(err) => err.to_string(),
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return (Library::default(), None);
            }
            Err(err) => err.to_string(),
        };
        let backup = data_dir.join(format!("fittings_saved.broken-{now}.toml"));
        match std::fs::copy(&path, &backup) {
            Ok(_) => (
                Library::default(),
                Some(format!(
                    "Couldn't read {FILE_NAME} ({problem}). It was copied to {} and \
                     the saved fits start empty.",
                    backup.display()
                )),
            ),
            Err(copy_err) => (
                Library {
                    fits: Vec::new(),
                    read_only: true,
                },
                Some(format!(
                    "Couldn't read {FILE_NAME} ({problem}) or copy it aside ({copy_err}). \
                     Saving fits is off until Athena restarts, so the file isn't lost."
                )),
            ),
        }
    }

    /// The position of the fit with this name and ship type. Names are
    /// compared without case, so "PvE" and "pve" are the same fit.
    pub fn find(&self, name: &str, hull: &str) -> Option<usize> {
        let name = name.trim().to_lowercase();
        self.fits
            .iter()
            .position(|f| f.name.to_lowercase() == name && f.hull == hull)
    }

    /// Saves a fit, replacing one with the same name and ship type.
    /// Returns its position.
    pub fn save(&mut self, name: &str, hull: &str, text: &str, now: i64) -> usize {
        let fit = SavedFit {
            name: name.trim().to_string(),
            hull: hull.to_string(),
            text: text.to_string(),
            changed: now,
        };
        match self.find(name, hull) {
            Some(index) => {
                self.fits[index] = fit;
                index
            }
            None => {
                self.fits.push(fit);
                self.fits.len() - 1
            }
        }
    }

    /// Renames a fit. Refused (with the reason) when the name is empty or
    /// another fit of the same ship type already has it.
    pub fn rename(&mut self, index: usize, new_name: &str, now: i64) -> Result<(), String> {
        let new_name = new_name.trim();
        if new_name.is_empty() {
            return Err("Enter a name.".to_string());
        }
        let Some(hull) = self.fits.get(index).map(|f| f.hull.clone()) else {
            return Err("That fit is gone.".to_string());
        };
        if self
            .find(new_name, &hull)
            .is_some_and(|other| other != index)
        {
            return Err(format!(
                "There already is a {hull} fit called \"{new_name}\"."
            ));
        }
        let fit = &mut self.fits[index];
        fit.name = new_name.to_string();
        fit.changed = now;
        Ok(())
    }

    pub fn delete(&mut self, index: usize) {
        if index < self.fits.len() {
            self.fits.remove(index);
        }
    }

    /// The positions of the fits that match the search (part of the name,
    /// any case) and the ship type filter, in the chosen order.
    pub fn listed(&self, search: &str, hull: Option<&str>, sort: SortBy) -> Vec<usize> {
        let search = search.trim().to_lowercase();
        let mut out: Vec<usize> = (0..self.fits.len())
            .filter(|&i| {
                let fit = &self.fits[i];
                hull.is_none_or(|h| fit.hull == h)
                    && (search.is_empty() || fit.name.to_lowercase().contains(&search))
            })
            .collect();
        let by_name =
            |a: &SavedFit, b: &SavedFit| a.name.to_lowercase().cmp(&b.name.to_lowercase());
        out.sort_by(|&a, &b| {
            let (a, b) = (&self.fits[a], &self.fits[b]);
            match sort {
                SortBy::Name => by_name(a, b).then_with(|| a.hull.cmp(&b.hull)),
                SortBy::Hull => a.hull.cmp(&b.hull).then_with(|| by_name(a, b)),
                SortBy::Changed => b.changed.cmp(&a.changed).then_with(|| by_name(a, b)),
            }
        });
        out
    }

    /// The ship types that have saved fits, without repeats, sorted.
    pub fn hulls(&self) -> Vec<String> {
        let mut hulls: Vec<String> = self.fits.iter().map(|f| f.hull.clone()).collect();
        hulls.sort();
        hulls.dedup();
        hulls
    }
}

/// "today 07:52", "yesterday 21:10", else "14.11.2026 20:33", in local
/// time.
pub fn changed_text(changed: i64, now: i64) -> String {
    let clock = &crate::localtime::clock_text(changed)[..5];
    let today = crate::localtime::to_local(now).div_euclid(86_400);
    let day = crate::localtime::to_local(changed).div_euclid(86_400);
    if day == today {
        format!("today {clock}")
    } else if day == today - 1 {
        format!("yesterday {clock}")
    } else {
        let (year, month, day) = crate::localtime::civil_from_days(day);
        format!("{day:02}.{month:02}.{year} {clock}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library() -> Library {
        let mut library = Library::default();
        library.save("DPS", "Thanatos", "[Thanatos, DPS]", 100);
        library.save("Links", "Salvation", "[Salvation, Links]", 300);
        library.save("Max yield", "Hulk", "[Hulk, Max yield]", 200);
        library.save("Tank", "Hulk", "[Hulk, Tank]", 50);
        library
    }

    #[test]
    fn same_name_and_type_replaces() {
        let mut library = library();
        let index = library.save(" dps ", "Thanatos", "[Thanatos, DPS v2]", 400);
        assert_eq!(index, 0);
        assert_eq!(library.fits.len(), 4);
        assert_eq!(library.fits[0].text, "[Thanatos, DPS v2]");
        assert_eq!(library.fits[0].changed, 400);
        // The same name on another ship type is a new fit.
        library.save("DPS", "Nidhoggur", "[Nidhoggur, DPS]", 500);
        assert_eq!(library.fits.len(), 5);
        // Case is ignored beyond ASCII too.
        library.save("Ölfass", "Hulk", "[Hulk, Ölfass]", 600);
        library.save("ölfass", "Hulk", "[Hulk, ölfass]", 700);
        assert_eq!(library.fits.len(), 6);
    }

    #[test]
    fn search_filter_and_sort() {
        let library = library();
        let names = |list: Vec<usize>| -> Vec<&str> {
            list.iter()
                .map(|&i| library.fits[i].name.as_str())
                .collect()
        };
        assert_eq!(
            names(library.listed("", None, SortBy::Changed)),
            ["Links", "Max yield", "DPS", "Tank"]
        );
        assert_eq!(
            names(library.listed("", None, SortBy::Name)),
            ["DPS", "Links", "Max yield", "Tank"]
        );
        assert_eq!(
            names(library.listed("", None, SortBy::Hull)),
            ["Max yield", "Tank", "Links", "DPS"]
        );
        assert_eq!(
            names(library.listed("", Some("Hulk"), SortBy::Changed)),
            ["Max yield", "Tank"]
        );
        assert_eq!(names(library.listed("LIN", None, SortBy::Name)), ["Links"]);
        assert_eq!(library.hulls(), ["Hulk", "Salvation", "Thanatos"]);
    }

    #[test]
    fn rename_refuses_duplicates() {
        let mut library = library();
        assert!(library.rename(2, "tank", 900).is_err());
        assert!(library.rename(2, "  ", 900).is_err());
        assert!(library.rename(2, "Yield", 900).is_ok());
        assert_eq!(library.fits[2].name, "Yield");
        assert_eq!(library.fits[2].changed, 900);
        // The same name on another ship type is fine.
        assert!(library.rename(0, "Links", 900).is_ok());
        library.delete(0);
        assert_eq!(library.fits.len(), 3);
    }

    #[test]
    fn file_reads_back() {
        let library = library();
        let text = toml::to_string(&library).unwrap();
        let back: Library = toml::from_str(&text).unwrap();
        assert_eq!(back, library);
        let empty: Library = toml::from_str("").unwrap();
        assert!(empty.fits.is_empty());
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("athena-lib-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn missing_file_is_empty_and_writable() {
        let dir = temp_dir("missing");
        let (library, problem) = Library::load(&dir, 1);
        assert!(library.fits.is_empty() && !library.read_only);
        assert!(problem.is_none());
    }

    #[test]
    fn broken_file_is_copied_aside() {
        let dir = temp_dir("broken");
        let path = Library::file_path(&dir);
        std::fs::write(&path, "[[fits]]\nname = \"half a fi").unwrap();
        let (library, problem) = Library::load(&dir, 1234);
        assert!(library.fits.is_empty() && !library.read_only);
        assert!(problem.unwrap().contains("fittings_saved.broken-1234.toml"));
        let backup = std::fs::read_to_string(dir.join("fittings_saved.broken-1234.toml")).unwrap();
        assert_eq!(backup, "[[fits]]\nname = \"half a fi");
        // A file that isn't UTF-8 is not "missing" either.
        std::fs::write(&path, [0xff, 0xfe, 0x00]).unwrap();
        let (_, problem) = Library::load(&dir, 1235);
        assert!(problem.is_some());
        assert!(dir.join("fittings_saved.broken-1235.toml").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn changed_texts() {
        // Linux test builds use UTC as local time.
        let now = 1_763_251_200 + 7 * 3600 + 52 * 60; // 2025-11-16 07:52 UTC
        assert_eq!(changed_text(now - 60, now), "today 07:51");
        assert_eq!(changed_text(now - 11 * 3600, now), "yesterday 20:52");
        assert_eq!(changed_text(now - 2 * 86_400, now), "14.11.2025 07:52");
    }
}
