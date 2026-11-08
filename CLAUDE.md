# CLAUDE.md

Guidelines for Claude working on Athena. Read `PLAN.md` first: it holds the goals, stack, roadmap and open questions. If this file and `PLAN.md` disagree, ask Kevin which one is right and fix the other.

## Project in one paragraph

Athena is a personal Windows desktop companion for EVE Online, written in Rust with egui/eframe. It reads the ESI API and the local EVE chat/game logs and shows modules (Mining Supervisor, Intel Supervisor, more later) as movable panels in a dashboard. It ships as a portable zip, with no installer. The owner, Kevin, is new to Rust.

## Hard rules

- **Never touch the EVE client.** No memory reading, injection, packet sniffing, screen scraping of the client, or input automation. Only ESI and log files. This keeps Athena within the EVE EULA. Drawing Athena's own windows over the client (the planned overlay) is fine. Reading from or sending anything to the client is not.
- **No secrets in the repo or the exe.** The ESI app needs only the Client ID and the localhost Callback URL. Both are public and may be in code. Never add a client secret, a field for one, or any flow that needs one. Never commit tokens or personal data.
- **Tokens live only in Windows Credential Manager** (via `keyring`). Never write access or refresh tokens to the data folder, config files, the SQLite DB, or log/debug output.
- **Stay portable.** Write app data (settings, layout, cache) only to the data folder next to the exe, or to `%APPDATA%\Athena` when that folder isn't writable. No registry, no other locations, no installer.
- **Never commit unanonymized logs.** Test fixtures taken from real logs get character names, corp/alliance names and channel contents replaced first.
- **Every change goes through a PR**, which an independent reviewer checks, unless Kevin says otherwise. Don't push to `main`.
- **Don't change the stack** (language, UI library, major crates) without asking Kevin first.

## Performance budget

Low resource use is the top goal, because Kevin runs several EVE clients next to Athena.

- Idle CPU under 1% with the window open. RAM under 100 MB with static data loaded.
- Call `ctx.request_repaint()` only when new data arrives. No idle animations, and no `request_repaint` every frame.
- Minimized or unfocused: no repaints unless a module has something new to show.
- Never block the UI thread with file or network I/O. That work runs on background tasks and reaches the UI through channels.
- When a change could affect performance, say how you checked it in the PR.

## Code style

Kevin is learning Rust and reviews the code, so readability beats cleverness.

- Prefer plain structs, enums and functions over heavy generics, trait magic or macros.
- Add a short comment where Rust-specific behavior isn't obvious (lifetimes, `Arc<Mutex<…>>`, async boundaries).
- Handle errors with `Result`, plus `anyhow` in the app or `thiserror` in library-like modules. No `unwrap()`/`expect()` outside tests and startup code where failure should end the app.
- Each dashboard module implements the `Module` trait from `src/modules/mod.rs` and lives in its own folder, `src/modules/<name>/mod.rs`. Register it with one line in `modules::create_all`. Its `id()` is saved in the layout file, so never change it once released. Background work talks to the module through a channel, which the module drains in `update()`.
- Add a crate only when it clearly earns its place, and say why in the PR.

## Checks before every push

```
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

CI (from step 2) runs `cargo fmt --check`, clippy with `-D warnings`, tests and a release build on `windows-latest`. If something is Windows-only and can't be tested in a Linux sandbox, say so in the PR rather than claiming it works.

## Domain notes

- Chat logs are UTF-16 LE, game logs are UTF-8. Both live under `Documents\EVE\logs\` by default (the path is configurable).
- Several clients write to the same folders. Map each file to a character through its header.
- The character's current system comes from `Channel changed to Local : <system>` in the Local chat log, with ESI as the fallback.
- For ESI, use SSO with PKCE (Client ID only), send `X-Compatibility-Date`, respect cache headers/ETags and both rate-limit schemes, and use `reqwest` with `rustls`.
- Static map data comes from the official SDE through the `xtask`, and the generated SQLite isn't committed.
- When unsure about an ESI or SSO detail, check https://developers.eveonline.com instead of relying on memory.

## Pull requests

- Keep PRs small and focused on one roadmap item or fix.
- Write the description in plain language: what changed, why, and how it was tested.
- Update `PLAN.md` when a decision changes or an open question gets answered.
