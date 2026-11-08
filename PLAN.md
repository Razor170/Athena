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
| Render backend | `glow` (OpenGL) by default; `wgpu` available behind the `wgpu` cargo feature until both are measured on Windows | `glow` is usually lighter; a 2D map doesn't need `wgpu` |
| Dockable panels | `egui_dock` (decided in step 2) | Tabs that can be docked, split, resized and pulled out into floating windows, with built-in `serde` support for saving the layout |
| Log watching | `notify` for new files + a cheap size poll of active files | Windows doesn't reliably raise change events for files another process keeps open |
| HTTP / ESI | `reqwest` (with `rustls`) + `tokio` | Async ESI calls; `rustls` keeps the exe free of extra TLS DLLs |
| Local storage | SQLite via `rusqlite` (bundled) | Static data and history in one file |
| Tokens | Windows Credential Manager via `keyring` | Refresh tokens are not stored in plain text |
| Config | `serde` + TOML | Human-readable settings and layout next to the exe (`data\settings.toml`, `data\layout.toml`) |

Considered and rejected: **Tauri** (WebView uses more memory), **C# / WPF** (large maps tend to feel sluggish).

## Data sources

### Local logs
- Default location: `Documents\EVE\logs\Chatlogs` and `...\Gamelogs` (configurable).
- **Encoding:** chat logs are UTF-16 LE, game logs are UTF-8. The parser handles both.
- **Watching:** a new file starts each session. `notify` catches new files; the currently active files are also polled for size every 250 to 500 ms, and only appended bytes are read. Verify the latency early in step 3 with a live client.
- **Multiple clients:** all clients write to the same folders. Each file's header names the character (`Listener:` in chat logs, the character line in game logs), and files are matched to characters through that header.
- **Character location:** the Local chat log writes `Channel changed to Local : <system>` on every jump. That is the primary location source (faster than ESI); the ESI location endpoint is only a fallback.
- Game logs contain combat, mining and notification lines (used by the Mining Supervisor).
- Intel channel lines get matched against system names, ship names and keywords from static data (see the Intel Supervisor below). Which intel channels to watch is a setting.

### ESI
- Register an application at developers.eveonline.com (Kevin does this, Athena never ships a secret).
- EVE SSO with **PKCE** and a localhost callback, with support for multiple characters.
- Send the `X-Compatibility-Date` header, respect cache headers and ETags, and read both the error-limit and the newer token-bucket rate-limit headers.

### Static data
- System names, IDs, positions and stargate connections from EVE's static data export, bundled as SQLite.
- Used for intel parsing, jump distances and drawing the map.
- Built by a small `xtask` that converts the official SDE (in its current distribution format) into a compact SQLite with the game data Athena needs: ore types and their reprocessing (Mining Supervisor) and, for the Intel Supervisor, systems, constellations, regions, gates, positions and ship type names. It's re-run by hand or by a manual CI workflow after patches that change the map. The generated file is produced in CI and bundled into the release zip rather than committed, so the repo stays small.

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
- **Done:** `egui_dock` picked over `egui_tiles`. Baseline from the Linux sandbox (glow, software rendering under Xvfb): 0 CPU ticks over 30 s idle with the window open, so no idle repaints. RAM there (about 120 MB) is dominated by the software OpenGL driver and doesn't say much about Windows; the Windows numbers for both renderers are still open.

### Step 3: Data foundations
- Log watcher and parsers for chat and game logs, with unit tests from sample log lines. Fixtures taken from real logs are anonymized (character names, channel contents) before they're committed.
- ESI client with SSO (PKCE), token storage and multiple characters.
- **Done (public part):** `src/esi.rs` is a small blocking ESI client for background threads (`X-Compatibility-Date`, ETags, error-limit and 429 back-off). The Mining Supervisor uses it for Jita market orders. SSO and tokens come with the first module that needs a login.
- Static data import and lookup (systems, gates, jump routes).
- **Done (ore part):** `cargo xtask sde` reads the official SDE zip (JSON Lines format) and writes `static.sqlite` with every ore, ice, moon ore and gas type (volume, portion size, compressed type, reprocessing outputs) plus the materials they reprocess into. CI downloads the SDE (cached per SDE build) and ships the file next to the exe. Systems and gates get added to the same file for the Intel Supervisor.

### Step 4: First modules
- **Mining Supervisor:** for multiboxed miners, one row per character plus totals. Uses game logs, the SDE and ESI's public market data (no login).
  - Ore hold fill per character, split into uncompressed and compressed ore. The log has every mined cycle (including critical successes; residue is ignored as it never reaches the hold) and every in-space compression (1:1 in units). Unloading at a station writes no log line, so each row has an "Emptied" button. Hold size and alert level are global settings (default 11,500 m³ and 70%) that each character can override. Fullness comes from the m³ math, and EVE's own "cargo hold is full" line counts as full whatever the math says. EVE logs only the ore that fit, so nothing mined is lost.
  - Alerts: hold above a per-character threshold, hold full, miner stopped (the module's "deactivates" line, or no cycle for a while), idle for X minutes.
  - A session starts with the first mined cycle and ends only when Kevin clicks "End session", which ends it for all characters at once. History shows one entry per session that opens to per-pilot data. A finished session's ISK value (reprocessed value at Jita prices from ESI's public market data, cached and refreshed at most hourly; yield 90.63% for ore, ice and moon ore, 95% for gas) is stored once and never recalculated. History also shows daily and weekly totals.
  - Belt scan: Athena watches the clipboard for a copied Mining Survey Scanner result and shows m³ and rocks left, reprocessed value, the belt's mining rate (from the first and latest scan, so other miners count) and the time until the belt is cleared.
  - Covers all ore, ice and moon ore; volumes, compressed types and reprocessing outputs come from the SDE.
  - UI: a panel at most a quarter of 1080p wide. Compressed ore is purple; normal ore is green, yellow from the alert level, red when full. Mockups are shown to Kevin for approval before any panel is built.
  - **Done:** the panel follows the approved mockup (Live, History and Settings views). Settings live in `mining.toml`, the live hold and session state (with log file positions, so a restart neither misses nor double counts ore) in `mining_state.toml`, and finished sessions in `mining.sqlite`. A session can't be ended while no Jita prices are loaded, so its value is never frozen at 0. Days and weeks in History follow EVE time (UTC), weeks start Monday (confirmed by Kevin).
- **Intel Supervisor:** a RIFT-like region map with an intel list, faster and lighter than RIFT. Uses chat logs, static data and ESI's public endpoints (no login). Approved mockup: https://claude.ai/artifact/AAU2CByJPT4bJj9xbcFqeY (all names and numbers in it are made up, and its positions were placed by hand).
  - **Panel:** fits beside the Mining panel on a maximized 1080p window (about 1440 px wide). The map and the intel list sit side by side with a draggable split between them; the panel itself resizes like every dock tab. Two views: Map and Settings. Module id `intel`.
  - **Channels:** Settings lists the channels found in the Chatlogs folder, and Kevin ticks the ones to watch. Files are matched to a channel by the `Channel Name:` line in their header, not by file name, so the new file EVE starts after a relog or downtime is picked up on its own. Chat logs are UTF-16 LE, so the reader works differently from the UTF-8 game log reader: it skips the BOM, reads appended bytes in even counts (an odd trailing byte waits for the next read) and handles CRLF. Only appended bytes are read.
  - **One report, many files:** EVE writes a separate log for every character in a channel, so with four pilots in Branch Intel each report arrives four times. Lines are de-duplicated by (channel, sender, text) within a few seconds of each other; the same check drops a report pasted into two watched channels (then without the channel). Each report gives one list entry and at most one alert.
  - **Own pilots:** each character is placed on the map from `Channel changed to Local : <system>` in their Local chat log and shown in blue, with a chip per pilot in the toolbar. Only Local files started after the last downtime (11:00 UTC) count, so characters that haven't played for weeks don't show up; a pilot can also be hidden from their chip. Jump distance for alerts counts from the nearest shown pilot. "Follow" keeps the map on the active pilot's region and switches region when that pilot jumps out (setting, on by default). The active pilot is the one picked on the Follow button, by default whoever jumped most recently.
  - **Map:** one region at a time (picker in the toolbar), drawn like RIFT: a dot per system, the name on a small dark label to its lower right with the security status, the system's tags underneath, and gate lines between dots. Systems in neighbouring regions that connect by gate are drawn as hollow dots with dashed lines. Positions come from the SDE's `position2D` (CCP's schematic layout, the same one as the in-game 2D map, which is already grid-aligned; Branch's 94 systems sit on about 31 columns and 32 rows). Dotlan's hand-made layouts are not copied because they belong to Dotlan. The xtask scales each region and nudges overlapping labels apart once, so nothing is laid out at runtime. The region picker leaves out wormhole, Abyssal and other regions without gates or without `position2D` (PR 1 checks that every k-space system has one). Pan by dragging, zoom with the wheel, "Fit region" and zoom buttons. Smooth pan and zoom is a hard requirement: the map is drawn with egui's painter, skips anything off screen, and repaints only while it is being moved or when new data arrives.
  - **Report states:** a reported system's dot turns red with a "jumps · age" badge and its gate lines turn red. It fades from red through orange to grey over 10 minutes (setting). A message with a clear word ("clr", "clear", setting) turns it green. Fading needs no animation: the colour is worked out from the report's age whenever a frame is drawn, and while a report is fading the module asks for one repaint per 30 s at most, and not at all while Athena is minimized (new reports and alerts still repaint).
  - **Parsing a line:** `[ time ] Sender > text`. Words are matched against system names from static data (case-insensitive, full names plus unique prefixes of at least 3 characters, so "X7R" finds X7R-JW). Prefixes are looked up within the shown region, the regions of the own pilots, and the neighbours of both. Words are also matched against ship type names (from the SDE), "nv" (no visual) and the clear words. The remaining words, minus a small stop-word list of common intel words ("gate", "ratting", "nv" and the like are real character names too), form pilot-name candidates of one to three words, since EVE names have up to three. They're looked up with ESI's public `POST /universe/ids/` (at most 500 names per call; hits and misses are both cached), and the ones that are real characters become links to their zKillboard page. A line with no system in it is still listed, greyed out.
  - **Intel list:** newest first. Each entry shows time, system, jumps from the nearest own pilot, a ship chip when a ship type was reported, the clickable pilot names, the message text and the channel. An "Only in range" toggle hides reports beyond the alert range.
  - **Alerts:** a report within 5 jumps (setting) of any shown pilot plays a sound and shows a red banner across the top of the map with the system, the pilots it's close to, the ship and the reporter. Within 2 jumps (setting) the alert is loud: it plays at full volume whatever the volume slider says, and twice. "Mute 5 min" and "Dismiss" buttons. Settings chooses the sound: Voice (default) or Beep.
    - The voice lines are WAV files in a `sounds` folder next to the exe: "Enemy in system", "Enemy 1 jump", then "Enemy 2 jumps" and so on up to "Enemy 10 jumps" (the alert range setting goes up to 10). Kevin can replace any file with his own recording. If a file is missing, the beep plays instead.
    - The CI job makes them with the Windows built-in voice through `System.Speech`. That needs Windows PowerShell 5.1 (`shell: powershell`), not `pwsh`. The step fails the build if the runner has no voice installed, rather than shipping an empty folder.
    - Playback uses `PlaySoundW` (the `Win32_Media_Audio` feature of the existing `windows-sys` crate, no new crate). `PlaySoundW` has no volume control, so Athena loads each WAV once, scales its samples in memory by the volume slider, and plays the copy with `SND_MEMORY`; that changes only the alert, not other sounds. Sounds play synchronously on a background thread (so the second play of a loud alert isn't cut off), never on the UI thread.
    - Settings has a play button per file, the volume slider and a "flash the map and tab" toggle.
  - **Tags:** labels on systems, shown under the system name, with a colour, an icon and a group (for example `4DTQ-K;POS;#F5FF83;structure_skyhook;false;POS`). Import and Export use RIFT's text format through the clipboard, one tag per line (`System;Label;Colour;Icon;flag;Group`), so a RIFT export pastes straight in; fields Athena doesn't use (the `false` flag, unknown icon names) are kept and written back unchanged. Known icons get their own small symbol (`resources_isk`, `mining_diamond`, `structure_skyhook`), others a plain dot in the tag's colour. Right-clicking a system on the map opens a menu with "Add tag…", "Mark clear" and "Open on Dotlan" (opens the browser). The mockup's "Set as destination" is left out for now because it needs an SSO login with the `esi-ui.write_waypoint` scope; it comes back once Athena has SSO (see Later). Each tag group can be hidden on the map. Tags are saved in `intel_tags.txt` in the data folder, in the same format.
  - **ESI kills and jumps tint:** optional, off by default. Tints systems by ESI's public kills and jumps of the last hour, fetched at most once an hour.
  - **Storage:** settings in `intel.toml`; tags in `intel_tags.txt`; reports are kept in memory only (they're old news after a restart, so nothing is saved).
  - **Static data:** the xtask adds regions, constellations, systems (name, security, `position2D`, region), stargate connections and ship type names to `static.sqlite`. Jump distances are a breadth-first search over the gate graph in memory, which takes well under a millisecond for the jump ranges used here.
    - **Done (PR 1):** `static.sqlite` schema version 2 adds `regions`, `constellations`, `systems`, `gates`, `region_borders` and `ship_types`. Only systems with stargates are kept (5,268 systems in 68 regions: known space, Pochven and Zarzakh); every one of them has a `position2D`, and the xtask fails if that ever changes. Each region's map is laid out once in the xtask: one grid step of `position2D` becomes 36 points at zoom 1.0 (Branch comes out about 920 × 800 points), and overlapping labels are nudged apart, mostly up or down. Systems of neighbouring regions are placed 64 points out from the system they connect to, in their real direction. `src/universe.rs` loads it; a 10-jump search from four systems takes about 15 µs in a release build.
  - **Build order (one PR each):** 1) static map data and jump distances; 2) chat log watcher, de-duplication, line parser, own-pilot locations and the channel picker, with anonymized fixtures; 3) map widget with pan, zoom, region picker, Follow and the pilot chips; 4) intel list, alerts, alert and sound settings, voice files and the CI step that makes them; 5) tags (import/export, right-click menu, group filter); 6) pilot lookup for zKillboard links and the optional ESI tint.

### Later
- **Overlay / on-screen alerts:** transparent, always-on-top, click-through windows for alerts or a mini intel panel. Only visible when EVE runs in *Window* or *Fixed Window* mode.
- **Intel: Set as destination** from the map's right-click menu, once SSO exists (needs `esi-ui.write_waypoint`).
- More modules as needed.

## Working agreement

- Every change goes through a pull request and is checked by an independent reviewer unless Kevin says otherwise.
- Rust is new to Kevin, so code should favor clarity over cleverness, with short comments on the non-obvious parts.
- Modules call `request_repaint` only when new data arrives and never animate while idle. This is what keeps the immediate-mode UI cheap.

## Open questions

- Which ESI scopes are needed (depends on the modules). Known so far: `esi-ui.write_waypoint` for the Intel map's "Set as destination".
- `glow` vs `wgpu`: needs idle CPU/RAM and startup numbers from Kevin's Windows PC (the sandbox only has a software renderer). Build both from the step 2 PR and compare with Task Manager; the Settings tab shows the renderer and a frame counter.
