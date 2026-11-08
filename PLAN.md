# Athena — Project Plan

Athena is a personal desktop companion for EVE Online. It reads two sources, the **ESI API** and the **local chat and game logs**, and shows the results in a modular dashboard of movable panels.

## Goals

- **Low resource use first.** It runs next to several EVE clients, so it should stay near zero CPU when idle and use little RAM. It should feel snappy, including the intel map.
- **Modular dashboard.** Modules are panels you can move, dock and resize. The layout is saved between runs.
- **Designed for a 1080p maximized window** (not fullscreen). Panels resize properly under Windows display scaling (for example 125%).
- **Portable on Windows.** Download a zip, unpack it and run the exe, with no installer. Settings and data live in a folder next to the exe.
- **EULA-safe.** Athena only uses ESI and log files. It never reads client memory, injects into the client or automates input.

**Audience:** Kevin, possibly his corp later.

## Tech stack

| Area | Choice | Why |
|---|---|---|
| Language | Rust | No GC pauses or heavy runtime, a small single exe |
| UI | `egui` / `eframe` with the `wgpu` backend | Redraws only on change, GPU drawn, supports transparent always-on-top windows for overlays |
| Dockable panels | `egui_tiles` or `egui_dock` (decided in step 2) | Movable dashboard modules |
| Log watching | `notify` | File system events instead of polling |
| HTTP / ESI | `reqwest` + `tokio` | Async ESI calls with ETag/cache handling |
| Local storage | SQLite via `rusqlite` (bundled) | Static data and history in one file |
| Tokens | Windows Credential Manager via `keyring` | Refresh tokens are not stored in plain text |
| Config | `serde` + TOML | Human-readable settings next to the exe |

Considered and rejected: **Tauri** (WebView uses more memory), **C# / WPF** (large maps tend to feel sluggish).

## Data sources

### Local logs
- Default location: `Documents\EVE\logs\Chatlogs` and `...\Gamelogs` (configurable).
- Chat logs are UTF-16 encoded. A new file starts each session, so the watcher follows new files as they appear and reads only appended lines.
- Game logs contain combat, mining and notification lines (used by the Mining Supervisor).
- Intel channel lines get matched against system names from static data.

### ESI
- Register an application at developers.eveonline.com (Kevin does this, Athena never ships a secret).
- EVE SSO with **PKCE** and a localhost callback, with support for multiple characters.
- Respect cache headers, ETags and error/rate limits.

### Static data
- System names, IDs, positions and stargate connections from EVE's static data export, bundled as SQLite.
- Used for intel parsing, jump distances and drawing the map.

## Roadmap

### Step 1: Plan *(this PR)*
Agree on goals, the stack and the roadmap.

### Step 2: Skeleton app and CI
- `eframe` window sized for a 1080p maximized layout, with DPI-aware sizing.
- Dockable panel layout with placeholder modules, and the layout saved/restored from the portable data folder.
- Settings panel (log folder path and similar).
- A small module trait so new modules plug in the same way.
- GitHub Actions on `windows-latest`: `cargo fmt --check`, `clippy`, tests and a release build, uploaded as a zip artifact. Version tags (`v*`) create a GitHub Release with the portable zip.
- Check idle CPU/RAM and keep it as a baseline.

### Step 3: Data foundations
- Log watcher and parsers for chat and game logs, with unit tests from sample log lines.
- ESI client with SSO (PKCE), token storage and multiple characters.
- Static data import and lookup (systems, gates, jump routes).

### Step 4: First modules
- **Mining Supervisor:** details to come from Kevin.
- **Intel Supervisor:** intel channel parsing plus a map of nearby systems with our own layout (from static data positions), highlighting reported systems and distance from your character. Smooth pan and zoom is a hard requirement. Details to come from Kevin.

### Later
- **Overlay / on-screen alerts:** transparent, always-on-top, click-through windows for alerts or a mini intel panel. Only visible when EVE runs in *Window* or *Fixed Window* mode.
- More modules as needed.

## Working agreement

- Every change goes through a pull request and is checked by an independent reviewer unless Kevin says otherwise.
- Rust is new to Kevin, so code should favor clarity over cleverness, with short comments on the non-obvious parts.

## Open questions

- Details of the Mining Supervisor and Intel Supervisor.
- Which ESI scopes are needed (depends on the modules).
- `egui_tiles` vs `egui_dock`, to be decided in step 2 with a quick prototype.
