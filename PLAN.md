# Athena — Project Plan

Athena is a personal desktop companion for EVE Online. It reads two sources, the **ESI API** and the **local chat and game logs**, and shows the results in a modular dashboard of movable panels.

## Goals

- **Low resource use first.** It runs next to several EVE clients and should feel snappy, including the intel map. Targets, measured in step 2 and checked again after each module:
  - Idle CPU under 1% with the window open and nothing changing (no repaints while idle).
  - Minimized or unfocused: no repaints at all except when a module has new data to show.
  - RAM under 100 MB with the static data loaded.
- **Modular dashboard.** Modules are panels you can move, dock and resize. The layout is saved between runs.
- **Designed for a 1080p maximized window** (not fullscreen). Panels resize properly under Windows display scaling (for example 125%).
- **Portable on Windows.** Download a zip, unpack it and run the exe, with no installer. Everything except login tokens (settings, layout, cached data) lives in a folder next to the exe. Tokens stay in Windows Credential Manager, so moving the folder to another PC means logging in again. If the exe's folder isn't writable (for example under Program Files), Athena falls back to `%APPDATA%\Athena` and says so in the settings panel.
- **EULA-safe.** Athena only uses ESI and log files. It never reads client memory, injects into the client or automates input.

**Audience:** Kevin, possibly his corp later.

## Tech stack

| Area | Choice | Why |
|---|---|---|
| Language | Rust | No GC pauses or heavy runtime, a small single exe |
| UI | `egui` / `eframe` | Redraws only on change, GPU drawn, supports transparent always-on-top windows for overlays |
| Render backend | `glow` (OpenGL) or `wgpu`, decided in step 2 by measuring idle RAM/CPU and startup time | `glow` is usually lighter; a 2D map doesn't need `wgpu` |
| Dockable panels | `egui_tiles` or `egui_dock` (decided in step 2) | Movable dashboard modules |
| Log watching | `notify` for new files + a cheap size poll of active files | Windows doesn't reliably raise change events for files another process keeps open |
| HTTP / ESI | `reqwest` (with `rustls`) + `tokio` | Async ESI calls; `rustls` keeps the exe free of extra TLS DLLs |
| Local storage | SQLite via `rusqlite` (bundled) | Static data and history in one file |
| Tokens | Windows Credential Manager via `keyring` | Refresh tokens are not stored in plain text |
| Config | `serde` + TOML | Human-readable settings next to the exe |

Considered and rejected: **Tauri** (WebView uses more memory), **C# / WPF** (large maps tend to feel sluggish).

## Data sources

### Local logs
- Default location: `Documents\EVE\logs\Chatlogs` and `...\Gamelogs` (configurable).
- **Encoding:** chat logs are UTF-16 LE, game logs are UTF-8. The parser handles both.
- **Watching:** a new file starts each session. `notify` catches new files; the currently active files are also polled for size every 250 to 500 ms, and only appended bytes are read. Verify the latency early in step 3 with a live client.
- **Multiple clients:** all clients write to the same folders. Each file's header names the character (`Listener:` in chat logs, the character line in game logs), and files are matched to characters through that header.
- **Character location:** the Local chat log writes `Channel changed to Local : <system>` on every jump. That is the primary location source (faster than ESI); the ESI location endpoint is only a fallback.
- Game logs contain combat, mining and notification lines (used by the Mining Supervisor).
- Intel channel lines get matched against system names from static data. Which intel channels to watch is a setting.

### ESI
- Register an application at developers.eveonline.com (Kevin does this, Athena never ships a secret).
- EVE SSO with **PKCE** and a localhost callback, with support for multiple characters.
- Send the `X-Compatibility-Date` header, respect cache headers and ETags, and read both the error-limit and the newer token-bucket rate-limit headers.

### Static data
- System names, IDs, positions and stargate connections from EVE's static data export, bundled as SQLite.
- Used for intel parsing, jump distances and drawing the map.
- Built by a small `xtask` that converts the official SDE (in its current distribution format) into a compact SQLite with only systems, constellations, regions, gates and positions. It's re-run by hand or by a manual CI workflow after patches that change the map. The generated file is produced in CI and bundled into the release zip rather than committed, so the repo stays small.

## Roadmap

### Step 1: Plan *(this PR)*
Agree on goals, the stack and the roadmap.

### Step 2: Skeleton app and CI
- `eframe` window sized for a 1080p maximized layout, with DPI-aware sizing.
- Dockable panel layout with placeholder modules, and the layout saved/restored from the portable data folder.
- Settings panel (log folder path and similar).
- A small module trait so new modules plug in the same way.
- `windows_subsystem = "windows"` so no console window opens next to the app.
- GitHub Actions on `windows-latest`: `cargo fmt --check`, `cargo clippy -- -D warnings`, tests and a release build, uploaded as a zip artifact. Version tags (`v*`) create a GitHub Release with the portable zip.
- Measure idle CPU/RAM against the targets above with both render backends, pick one, and record the numbers as the baseline.
- Note in the README that the exe isn't code-signed, so Windows SmartScreen warns on first run ("More info" then "Run anyway").

### Step 3: Data foundations
- Log watcher and parsers for chat and game logs, with unit tests from sample log lines. Fixtures taken from real logs are anonymized (character names, channel contents) before they're committed.
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
- Modules call `request_repaint` only when new data arrives and never animate while idle. This is what keeps the immediate-mode UI cheap.

## Open questions

- Details of the Mining Supervisor and Intel Supervisor.
- Which ESI scopes are needed (depends on the modules).
- `egui_tiles` vs `egui_dock`, and `glow` vs `wgpu`, to be decided in step 2 with a quick prototype and measurements.
