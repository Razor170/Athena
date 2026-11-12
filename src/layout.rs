//! The dashboard layout: which tabs are open and where they are docked,
//! where closed tabs used to be, and where the main window sits on screen.
//! Saved as `layout.toml` in the data folder whenever it changes.

use std::fs;
use std::path::{Path, PathBuf};

use eframe::egui;
use egui_dock::{
    DockState, Node, NodeIndex, NodePath, Split, SurfaceIndex, TabIndex, TabPath, Tree,
};
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
    /// Where each closed tab used to be, so opening it again puts it back
    /// in the same spot with the same size. `serde(default)` lets older
    /// layout files without this list load fine.
    #[serde(default)]
    pub closed: Vec<ClosedTab>,
    /// The main window's position and size. `None` until Athena has run once
    /// with this feature; then the window starts maximized as before.
    #[serde(default)]
    pub window: Option<WindowPlacement>,
}

/// A tab that is closed, and where it was.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClosedTab {
    pub tab: Tab,
    pub spot: Spot,
}

/// Where a tab was when it was closed. Tabs are named rather than numbered,
/// because node numbers change whenever the user rearranges the dock.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Spot {
    /// It shared a tab group with other tabs. It goes back into the group
    /// that holds one of `neighbours`, at position `index`.
    InGroup { neighbours: Vec<Tab>, index: usize },
    /// It had a group of its own, split off from the area that holds the
    /// tabs in `beside`. `side` says where it was relative to that area, and
    /// `fraction` is the share of the left (or top) part of that split, as
    /// egui_dock stores it.
    Split {
        beside: Vec<Tab>,
        side: Side,
        fraction: f32,
    },
    /// It was the only tab in a floating window. The rect is in egui points.
    Window {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    },
    /// It was the only tab left in the main area.
    Alone,
}

/// Which side of the neighbouring area a tab group was on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    Left,
    Right,
    Above,
    Below,
}

/// The main window's "normal" (not maximized) position and size in screen
/// pixels, plus whether it was maximized. This is what Windows itself keeps
/// for a window (`GetWindowPlacement`), so a maximized window remembers
/// both its monitor and the size it goes back to when un-maximized.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowPlacement {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub maximized: bool,
}

/// Only the window part of `layout.toml`. `main` reads this before the
/// window exists; the rest of the file is read later by `Layout::load`.
/// serde skips the fields this struct doesn't have.
#[derive(Deserialize)]
struct WindowOnly {
    window: Option<WindowPlacement>,
}

/// Reads the saved main window placement, if there is one.
pub fn saved_window_placement(data_dir: &Path) -> Option<WindowPlacement> {
    let text = fs::read_to_string(Layout::file_path(data_dir)).ok()?;
    toml::from_str::<WindowOnly>(&text).ok()?.window
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
            closed: Vec::new(),
            window: None,
        }
    }

    /// Loads the saved layout. A missing file gives the default layout. A
    /// broken file also gives the default, plus a warning to show the user.
    pub fn load(data_dir: &Path, module_ids: &[String]) -> (Layout, Option<String>) {
        let path = Layout::file_path(data_dir);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(_) => return (Layout::default_for(module_ids), None),
        };
        match Layout::from_toml(&text) {
            Ok(mut layout) => {
                layout.fit_to_modules(module_ids);
                (layout, None)
            }
            Err(e) => (
                Layout::default_for(module_ids),
                Some(format!(
                    "{} could not be read, so the default layout is used: {e}",
                    path.display()
                )),
            ),
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
        // Forget where removed modules were, and any tab that is open anyway.
        let dock = &self.dock;
        self.closed.retain(|closed| {
            let exists = match &closed.tab {
                Tab::Module(id) => module_ids.contains(id),
                Tab::Settings => true,
            };
            exists && dock.find_tab(&closed.tab).is_none()
        });
    }

    pub fn is_open(&self, tab: &Tab) -> bool {
        self.dock.find_tab(tab).is_some()
    }

    /// Shows the tab: focuses it if it's already open, otherwise puts it back
    /// where it was when it was closed (or, if that spot is gone, into the
    /// focused tab group).
    pub fn open(&mut self, tab: Tab) {
        if let Some(path) = self.dock.find_tab(&tab) {
            let _ = self.dock.set_active_tab(path);
            return;
        }
        let spot = match self.closed.iter().position(|closed| closed.tab == tab) {
            Some(i) => Some(self.closed.remove(i).spot),
            None => None,
        };
        let tab = match spot {
            Some(spot) => match self.put_back(tab, spot) {
                Ok(()) => return,
                Err(tab) => tab,
            },
            None => tab,
        };
        self.dock.push_to_focused_leaf(tab);
    }

    /// Closes the tab and remembers where it was. `ctx` tells us where
    /// floating windows are on screen.
    pub fn close(&mut self, tab: &Tab, ctx: &egui::Context) {
        let Some(path) = self.dock.find_tab(tab) else {
            return;
        };
        let spot = self.spot_of(path, ctx);
        self.dock.remove_tab(path);
        self.closed.retain(|closed| &closed.tab != tab);
        if let Some(spot) = spot {
            self.closed.push(ClosedTab {
                tab: tab.clone(),
                spot,
            });
        }
    }

    /// Works out where the tab at `path` is, in a way that still makes sense
    /// after the tab is gone. `None` if the dock looks unexpected.
    fn spot_of(&self, path: TabPath, ctx: &egui::Context) -> Option<Spot> {
        let tree = &self.dock[path.surface];
        let tabs = tree[path.node].tabs()?;
        if tabs.len() > 1 {
            let neighbours = tabs
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != path.tab.0)
                .map(|(_, tab)| tab.clone())
                .collect();
            return Some(Spot::InGroup {
                neighbours,
                index: path.tab.0,
            });
        }
        // The tab is alone in its group. When the group goes, egui_dock
        // removes the split it was part of, and the other half of that split
        // (the "sibling") takes the whole space.
        let Some(parent) = path.node.parent() else {
            // The group was the whole surface.
            if path.surface.is_main() {
                return Some(Spot::Alone);
            }
            let rect = self.floating_rect(path.surface, ctx)?;
            return Some(Spot::Window {
                x: rect.min.x,
                y: rect.min.y,
                width: rect.width(),
                height: rect.height(),
            });
        };
        let is_first = path.node.is_left();
        let sibling = if is_first {
            parent.right()
        } else {
            parent.left()
        };
        let (fraction, side) = match &tree[parent] {
            Node::Horizontal(split) => (
                split.fraction,
                if is_first { Side::Left } else { Side::Right },
            ),
            Node::Vertical(split) => (
                split.fraction,
                if is_first { Side::Above } else { Side::Below },
            ),
            _ => return None,
        };
        let beside = tabs_under(tree, sibling);
        if beside.is_empty() {
            return None;
        }
        Some(Spot::Split {
            beside,
            side,
            fraction,
        })
    }

    /// Where a floating window is on screen, in egui points, as
    /// `WindowState::set_position` and `set_size` expect them. `None` if it
    /// hasn't been drawn yet.
    fn floating_rect(&self, surface: SurfaceIndex, ctx: &egui::Context) -> Option<egui::Rect> {
        // egui_dock never fills in `WindowState::rect`, so ask egui. egui_dock
        // names each floating window "window SurfaceIndex(n)" (see its
        // `window_surface.rs`). If that ever changes we fall back to the
        // rect of the window's content, which is a few points smaller.
        let id = egui::Id::new(format!("window {surface:?}"));
        let rect = match ctx.memory(|memory| memory.area_rect(id)) {
            Some(rect) => rect,
            None => self.dock[surface].root_node()?.rect()?,
        };
        (rect.is_finite() && rect.width() > 0.0 && rect.height() > 0.0).then_some(rect)
    }

    /// Stores where every floating window is, so they open in the same
    /// place after a restart. Called just before the layout is saved on exit.
    pub fn remember_floating_windows(&mut self, ctx: &egui::Context) {
        let surfaces: Vec<SurfaceIndex> = self
            .dock
            .iter_surfaces_indexed()
            .filter(|(index, surface)| !index.is_main() && !surface.is_empty())
            .map(|(index, _)| index)
            .collect();
        for surface in surfaces {
            let Some(rect) = self.floating_rect(surface, ctx) else {
                continue;
            };
            if let Some(state) = self.dock.get_window_state_mut(surface) {
                state.set_position(rect.min).set_size(rect.size());
            }
        }
    }

    /// Puts a tab back into `spot`. Gives the tab back if the spot no longer
    /// exists (for example, every neighbour is closed too).
    fn put_back(&mut self, tab: Tab, spot: Spot) -> Result<(), Tab> {
        match spot {
            Spot::InGroup { neighbours, index } => {
                let Some(found) = neighbours.iter().find_map(|n| self.dock.find_tab(n)) else {
                    return Err(tab);
                };
                let node_path = found.node_path();
                let Ok(leaf) = self.dock.leaf_mut(node_path) else {
                    return Err(tab);
                };
                let index = index.min(leaf.tabs.len());
                leaf.insert_tab(index, tab);
                leaf.active = TabIndex(index);
                self.dock.set_focused_node_and_surface(node_path);
                Ok(())
            }
            Spot::Split {
                beside,
                side,
                fraction,
            } => {
                let Some(node_path) = self.area_holding(&beside) else {
                    return Err(tab);
                };
                let split = match side {
                    Side::Left => Split::Left,
                    Side::Right => Split::Right,
                    Side::Above => Split::Above,
                    Side::Below => Split::Below,
                };
                // egui_dock stores `fraction` as given in the new split, and
                // it is always the share of the left (or top) part, which is
                // exactly what we saved. (Its docs describe it differently;
                // the tests below check what it really does.) `split` panics
                // outside 0..=1, so keep both parts visible.
                let fraction = if fraction.is_finite() {
                    fraction.clamp(0.05, 0.95)
                } else {
                    0.5
                };
                self.dock.split(node_path, split, fraction, Node::leaf(tab));
                Ok(())
            }
            Spot::Window {
                x,
                y,
                width,
                height,
            } => {
                let surface = self.dock.add_window(vec![tab]);
                if let Some(state) = self.dock.get_window_state_mut(surface) {
                    state
                        .set_position(egui::pos2(x, y))
                        .set_size(egui::vec2(width, height));
                }
                Ok(())
            }
            Spot::Alone => {
                if self.dock.main_surface().num_tabs() == 0 {
                    self.dock.push_to_first_leaf(tab);
                    Ok(())
                } else {
                    Err(tab)
                }
            }
        }
    }

    /// The smallest dock area (a tab group or a split) that holds every
    /// still-open tab of `tabs`. Only looks at the surface (main area or
    /// floating window) of the first open one.
    fn area_holding(&self, tabs: &[Tab]) -> Option<NodePath> {
        let paths: Vec<TabPath> = tabs.iter().filter_map(|t| self.dock.find_tab(t)).collect();
        let surface: SurfaceIndex = paths.first()?.surface;
        let mut area = paths[0].node;
        for path in paths.iter().filter(|p| p.surface == surface) {
            area = common_ancestor(area, path.node);
        }
        Some(NodePath::new(surface, area))
    }
}

/// All tabs in `node` and the nodes below it.
fn tabs_under(tree: &Tree<Tab>, node: NodeIndex) -> Vec<Tab> {
    // A tree is stored as a list in which node `i` has its children at
    // `2i+1` and `2i+2`, so the list position is the node's index.
    tree.iter()
        .enumerate()
        .filter(|(i, _)| is_under(NodeIndex(*i), node))
        .flat_map(|(_, n)| n.iter_tabs().cloned())
        .collect()
}

/// True if `node` is `area` or somewhere below it.
fn is_under(node: NodeIndex, area: NodeIndex) -> bool {
    let mut current = Some(node);
    while let Some(n) = current {
        if n == area {
            return true;
        }
        current = n.parent();
    }
    false
}

/// The lowest node that has both `a` and `b` below it (or is one of them).
fn common_ancestor(mut a: NodeIndex, mut b: NodeIndex) -> NodeIndex {
    while a != b {
        // Step up from whichever is deeper; the root is `0`, so a bigger
        // index is never above a smaller one.
        let (deeper, other) = if a.0 > b.0 { (a, b) } else { (b, a) };
        a = deeper.parent().unwrap_or(NodeIndex::root());
        b = other;
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    /// An egui context that has drawn nothing, for `Layout::close`.
    fn ctx() -> egui::Context {
        egui::Context::default()
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
    fn broken_file_gives_default_and_warning() {
        let dir = std::env::temp_dir().join(format!("athena-test-layout-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(Layout::file_path(&dir), "dock = [").unwrap();
        let (layout, warning) = Layout::load(&dir, &ids(&["a"]));
        assert!(layout.is_open(&module("a")));
        assert!(warning.is_some());
        fs::remove_dir_all(&dir).unwrap();
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
        layout.close(&module("b"), &ctx());
        layout.fit_to_modules(&ids(&["a", "b"]));
        assert!(!layout.is_open(&module("b")));
    }

    #[test]
    fn open_adds_a_closed_tab_back() {
        let mut layout = Layout::default_for(&ids(&["a"]));
        layout.close(&Tab::Settings, &ctx());
        assert!(!layout.is_open(&Tab::Settings));
        layout.open(Tab::Settings);
        assert!(layout.is_open(&Tab::Settings));
    }

    /// The share of the first (left or top) part of the split that holds `tab`'s group.
    fn split_fraction_around(layout: &Layout, tab: &Tab) -> f32 {
        let path = layout.dock.find_tab(tab).unwrap();
        let parent = path.node.parent().unwrap();
        match &layout.dock[path.surface][parent] {
            Node::Horizontal(split) | Node::Vertical(split) => split.fraction,
            _ => panic!("parent is not a split"),
        }
    }

    #[test]
    fn reopened_tab_goes_back_into_its_group_at_the_same_place() {
        let mut layout = Layout::default_for(&ids(&["a", "b", "c"]));
        layout.close(&module("b"), &ctx());
        layout.open(module("b"));
        let path = layout.dock.find_tab(&module("b")).unwrap();
        assert_eq!(path.tab.0, 1);
        assert_eq!(layout.dock.find_tab(&module("a")).unwrap().node, path.node);
    }

    #[test]
    fn reopened_split_tab_gets_its_side_and_size_back() {
        // a | Settings, with a taking 75%. Close Settings and open it again.
        let mut layout = Layout::default_for(&ids(&["a"]));
        let before = split_fraction_around(&layout, &Tab::Settings);
        assert_eq!(before, 0.75);
        layout.close(&Tab::Settings, &ctx());
        assert_eq!(layout.dock.main_surface().num_tabs(), 1);
        layout.open(Tab::Settings);
        assert_eq!(split_fraction_around(&layout, &Tab::Settings), 0.75);
        let settings = layout.dock.find_tab(&Tab::Settings).unwrap();
        assert!(
            settings.node.is_right(),
            "Settings should be on the right again"
        );
    }

    #[test]
    fn reopened_left_tab_returns_on_the_left() {
        // mining (30%) | intel above settings. Close mining, reopen it.
        let mut dock = DockState::new(vec![module("mining")]);
        let [_, right] =
            dock.main_surface_mut()
                .split_right(NodeIndex::root(), 0.3, vec![module("intel")]);
        dock.main_surface_mut()
            .split_below(right, 0.6, vec![Tab::Settings]);
        let mut layout = Layout {
            dock,
            known_modules: ids(&["mining", "intel"]),
            closed: Vec::new(),
            window: None,
        };
        layout.close(&module("mining"), &ctx());
        layout.open(module("mining"));
        let mining = layout.dock.find_tab(&module("mining")).unwrap();
        assert!(mining.node.is_left());
        assert!((split_fraction_around(&layout, &module("mining")) - 0.3).abs() < 1e-6);
        // Intel and Settings are still stacked with the same split.
        assert!((split_fraction_around(&layout, &Tab::Settings) - 0.6).abs() < 1e-6);
    }

    #[test]
    fn reopened_floating_tab_gets_its_window_back() {
        let mut layout = Layout::default_for(&ids(&["a", "b"]));
        let path = layout.dock.find_tab(&module("b")).unwrap();
        let rect = egui::Rect::from_min_size(egui::pos2(100.0, 100.0), egui::vec2(400.0, 300.0));
        let surface = layout.dock.detach_tab(path, rect);
        // egui_dock sets the rect when it draws the window; pretend it did.
        layout.dock[surface][NodeIndex::root()].set_rect(rect);
        layout.close(&module("b"), &ctx());
        assert_eq!(
            layout.closed[0].spot,
            Spot::Window {
                x: 100.0,
                y: 100.0,
                width: 400.0,
                height: 300.0
            }
        );
        layout.open(module("b"));
        let path = layout.dock.find_tab(&module("b")).unwrap();
        assert!(!path.surface.is_main());
    }

    #[test]
    fn spot_survives_a_restart() {
        let mut layout = Layout::default_for(&ids(&["a"]));
        layout.close(&Tab::Settings, &ctx());
        let text = layout.to_toml().unwrap();
        let mut loaded = Layout::from_toml(&text).unwrap();
        loaded.fit_to_modules(&ids(&["a"]));
        assert_eq!(loaded.closed, layout.closed);
        loaded.open(Tab::Settings);
        assert_eq!(split_fraction_around(&loaded, &Tab::Settings), 0.75);
    }

    #[test]
    fn spot_falls_back_when_its_neighbours_are_gone() {
        // a | Settings. Close Settings, then a: nothing is left to sit beside.
        let mut layout = Layout::default_for(&ids(&["a"]));
        layout.close(&Tab::Settings, &ctx());
        layout.close(&module("a"), &ctx());
        layout.open(Tab::Settings);
        assert!(layout.is_open(&Tab::Settings));
        // a was alone in the main area, so it comes back next to Settings.
        layout.open(module("a"));
        assert!(layout.is_open(&module("a")));
    }

    #[test]
    fn old_layout_files_without_the_new_fields_still_load() {
        let layout = Layout::default_for(&ids(&["a"]));
        let text = layout.to_toml().unwrap();
        // Drop the new fields, as a file from an older Athena wouldn't have them.
        let old: Vec<&str> = text
            .lines()
            .filter(|line| !line.starts_with("closed") && !line.starts_with("window"))
            .collect();
        let loaded = Layout::from_toml(&old.join("\n")).unwrap();
        assert!(loaded.closed.is_empty());
        assert!(loaded.window.is_none());
    }

    #[test]
    fn window_placement_round_trip() {
        let mut layout = Layout::default_for(&ids(&["a"]));
        let placement = WindowPlacement {
            x: -1900,
            y: 40,
            width: 1600,
            height: 900,
            maximized: true,
        };
        layout.window = Some(placement);
        let text = layout.to_toml().unwrap();
        let dir = std::env::temp_dir().join(format!("athena-test-window-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(Layout::file_path(&dir), &text).unwrap();
        assert_eq!(saved_window_placement(&dir), Some(placement));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn common_ancestor_finds_the_split_above_both() {
        let root = NodeIndex::root();
        assert_eq!(common_ancestor(root.left(), root.right()), root);
        assert_eq!(
            common_ancestor(root.right().left(), root.right().right()),
            root.right()
        );
        assert_eq!(common_ancestor(root.left(), root.left()), root.left());
    }
}
