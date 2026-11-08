# Athena

A lightweight, portable Windows desktop companion for EVE Online, built in Rust with egui.

See [PLAN.md](PLAN.md) for the project plan.

## Download and run

1. Download `Athena-windows.zip` from the latest [release](https://github.com/Razor170/Athena/releases)
   (or from the artifacts of a CI run on the Actions tab).
2. Unpack it anywhere, for example `D:\Tools\Athena`.
3. Run `Athena.exe`.

There is no installer. Athena keeps its settings and layout in a `data` folder next to
`Athena.exe`, so you can move or copy the whole folder. If that folder isn't writable
(for example under `Program Files`), Athena uses `%APPDATA%\Athena` instead and says so
in the Settings tab. Login tokens (later) are stored in Windows Credential Manager, not
in the folder.

**Windows SmartScreen:** the exe isn't code-signed, so Windows warns the first time you
run it. Click **More info**, then **Run anyway**.

## Using the dashboard

- Every module is a tab. Drag a tab by its title to move it, dock it beside another
  tab, or pull it out into its own floating window. Drag the gaps between panels to resize.
- Closed tabs come back through the **View** menu.
- The layout is saved when Athena closes.

## Mining Supervisor

One card per character, filled from the game logs (`Documents\EVE\logs\Gamelogs`; set
another log folder in Athena's settings):

- **Hold bar:** purple is compressed ore, green normal ore, yellow above your alert level,
  red when full. EVE writes nothing when you unload at a station, so click **Emptied**
  afterwards.
- **Borders:** red dashed means the miners stopped (or the hold is full), purple means a
  mining crystal broke.
- **Belt scan:** copy a Mining Survey Scanner result in EVE (Ctrl+A, Ctrl+C) and it shows up
  in the panel. Copy another one later to see how fast the belt is going.
- **End session** ends mining for all characters at once and stores it in History, valued
  with the Jita prices of that moment. That value never changes afterwards.
- **Settings:** hold size and alert level for everyone (11,500 m³ and 70% by default),
  with per-character overrides, Jita buy or sell, and the refining yields.

Files in the data folder: `mining.toml` (settings), `mining_state.toml` (hold contents
and the running session), `mining.sqlite` (history), `jita_prices.toml` (price cache).

## Building from source

```
cargo run                       # debug build, with a console window for messages
cargo build --release           # target/release/athena.exe
cargo build --release --features wgpu   # same, with the wgpu renderer instead of OpenGL
```

### Static game data

Ore volumes, compression and reprocessing come from EVE's static data export (SDE).
`cargo xtask sde` turns it into a small `static.sqlite`, which Athena reads from the
folder next to the exe (or the current folder when you use `cargo run`). CI does this
for every build and puts the file in the zip. To do it by hand:

1. Download the "JSON Lines" zip from <https://developers.eveonline.com/static-data>
   (for example `eve-online-static-data-latest-jsonl.zip`).
2. Run `cargo xtask sde path\to\that.zip` in the repo folder. It writes `static.sqlite`.

`static.sqlite` and the SDE zip are not committed.

Before pushing, run:

```
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Adding a module

See the top of [`src/modules/mod.rs`](src/modules/mod.rs). In short: create
`src/modules/<name>/mod.rs`, implement the `Module` trait, and add one line to
`create_all`.
