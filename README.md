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

## Building from source

```
cargo run                       # debug build, with a console window for messages
cargo build --release           # target/release/athena.exe
cargo build --release --features wgpu   # same, with the wgpu renderer instead of OpenGL
```

Before pushing, run:

```
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
```

## Adding a module

See the top of [`src/modules/mod.rs`](src/modules/mod.rs). In short: create
`src/modules/<name>/mod.rs`, implement the `Module` trait, and add one line to
`create_all`.
