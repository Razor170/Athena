//! The dashboard layout: which tabs are open and where they are docked.
//! Saved as `layout.toml` in the data folder when Athena closes.

use std::fs;
use std::path::{Path, PathBuf};

use egui_dock::{DockState, NodeIndex};
use serde::{Deserialize, Serialize};

// TOML rather than JSON because egui_dock stores "not measured yet" rectangles
// as infinity, which JSON can't represent but TOML can.
const FILE_NAME: &str = "layout.toml";

/// One tab in the dock area.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Tab {
    /// A dashboard module, by its id (see `Module::id`).
    Module(String),
    /// The built-in settings panel.
    Settings,
}

/// What we write to `layout.toml`.
#[derive(Serialize, Deserialize)]
pub struct Layout {
    pub dock: DockState<Tab>,
    /// Every module id that existed when the layout was saved. A module that
    /// isn't listed here is new, so we open it once. A module that is listed
    /// but not in `dock` was closed by the user, so it stays closed.
    pub known_modules: Vec<String>,
}

impl Layout {
    pub fn file_path(data_dir: &Path) -> PathBuf {
        data_dir.join(FILE_NAME)
    }

    /// The first-run layout: all modules as tabs on the left, settings on the right.
    pub fn default_for(module_ids: &[String]) -> Layout {
        let tabs = module_ids.iter().cloned().map(Tab::Module).collect();
        let mut dock = DockState::new(tabs);
        dock.main_surface_mut()
            .split_right(NodeIndex::root(), 0.75, vec![Tab::Settings]);
        Layout {
            dock,
            known_modules: module_ids.to_vec(),
        }
    }

    /// Loads the saved layout, or the default one if there is none or it can't be read.
    pub fn load(data_dir: &Path, module_ids: &[String]) -> Layout {
        let saved = fs::read_to_string(Layout::file_path(data_dir))
            .ok()
            .and_then(|text| Layout::from_toml(&text).ok());
        match saved {
            Some(mut layout) => {
                layout.fit_to_modules(module_ids);
                layout
            }
            None => Layout::default_for(module_ids),
        }
    }

    pub fn from_toml(text: &str) -> Result<Layout, toml::de::Error> {
        toml::from_str(text)
    }

    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        toml::to_string(self)
    }

    /// Makes a saved layout match the modules this build actually has:
    /// drops tabs of modules that no longer exist and opens modules that are new.
    pub fn fit_to_modules(&mut self, module_ids: &[String]) {
        self.dock.retain_tabs(|tab| match tab {
            Tab::Module(id) => module_ids.contains(id),
            Tab::Settings => true,
        });
        for id in module_ids {
            if !self.known_modules.contains(id) {
                self.dock.push_to_first_leaf(Tab::Module(id.clone()));
            }
        }
        self.known_modules = module_ids.to_vec();
    }

    pub fn is_open(&self, tab: &Tab) -> bool {
        self.dock.find_tab(tab).is_some()
    }

    /// Shows the tab: focuses it if it's already open, otherwise adds it.
    pub fn open(&mut self, tab: Tab) {
        match self.dock.find_tab(&tab) {
            Some(path) => {
                let _ = self.dock.set_active_tab(path);
            }
            None => self.dock.push_to_focused_leaf(tab),
        }
    }

    pub fn close(&mut self, tab: &Tab) {
        if let Some(path) = self.dock.find_tab(tab) {
            self.dock.remove_tab(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui;

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn module(id: &str) -> Tab {
        Tab::Module(id.to_string())
    }

    #[test]
    fn default_layout_has_all_modules_and_settings() {
        let layout = Layout::default_for(&ids(&["a", "b"]));
        assert!(layout.is_open(&module("a")));
        assert!(layout.is_open(&module("b")));
        assert!(layout.is_open(&Tab::Settings));
    }

    #[test]
    fn toml_round_trip() {
        let layout = Layout::default_for(&ids(&["a"]));
        let text = layout.to_toml().unwrap();
        let loaded = Layout::from_toml(&text).unwrap();
        assert!(loaded.is_open(&module("a")));
        assert_eq!(loaded.known_modules, ids(&["a"]));
    }

    #[test]
    fn floating_window_round_trip() {
        let mut layout = Layout::default_for(&ids(&["a", "b"]));
        let path = layout.dock.find_tab(&module("b")).unwrap();
        let rect = egui::Rect::from_min_size(egui::pos2(100.0, 100.0), egui::vec2(400.0, 300.0));
        layout.dock.detach_tab(path, rect);
        let text = layout.to_toml().unwrap();
        let loaded = Layout::from_toml(&text).unwrap();
        assert!(loaded.is_open(&module("b")));
        assert_eq!(loaded.dock.surfaces_count(), 2);
    }

    #[test]
    fn removed_modules_are_dropped() {
        let mut layout = Layout::default_for(&ids(&["a", "gone"]));
        layout.fit_to_modules(&ids(&["a"]));
        assert!(layout.is_open(&module("a")));
        assert!(!layout.is_open(&module("gone")));
    }

    #[test]
    fn new_modules_are_opened() {
        let mut layout = Layout::default_for(&ids(&["a"]));
        layout.fit_to_modules(&ids(&["a", "new"]));
        assert!(layout.is_open(&module("new")));
    }

    #[test]
    fn closed_modules_stay_closed() {
        let mut layout = Layout::default_for(&ids(&["a", "b"]));
        layout.close(&module("b"));
        layout.fit_to_modules(&ids(&["a", "b"]));
        assert!(!layout.is_open(&module("b")));
    }

    #[test]
    fn open_adds_a_closed_tab_back() {
        let mut layout = Layout::default_for(&ids(&["a"]));
        layout.close(&Tab::Settings);
        assert!(!layout.is_open(&Tab::Settings));
        layout.open(Tab::Settings);
        assert!(layout.is_open(&Tab::Settings));
    }
}
