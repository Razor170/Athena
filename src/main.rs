// On Windows, build as a GUI app so no console window opens next to Athena.
// Debug builds keep the console so `println!`/panic messages stay visible while developing.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod auth;
mod esi;
mod layout;
mod localtime;
mod modules;
mod overlay;
mod settings;
mod shell;
mod static_data;
mod storage;
mod universe;
mod updater;
mod version;
mod widgets;
mod window_place;

use std::path::PathBuf;

use anyhow::{Context, Result};

const STARTUP_ERROR_FILE: &str = "athena-startup-error.txt";

fn main() -> Result<()> {
    let result = run();
    if let Err(error) = &result {
        // Release builds have no console, so a startup error would vanish
        // without a trace. Write it to a text file the user can find.
        write_startup_error(&format!("{error:#}"));
    }
    result
}

/// Writes the error next to the exe, or into the temp folder if that fails.
fn write_startup_error(text: &str) {
    let next_to_exe = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(STARTUP_ERROR_FILE)));
    let places: Vec<PathBuf> = next_to_exe
        .into_iter()
        .chain(std::iter::once(
            std::env::temp_dir().join(STARTUP_ERROR_FILE),
        ))
        .collect();
    for path in places {
        if std::fs::write(&path, text).is_ok() {
            return;
        }
    }
}

fn run() -> Result<()> {
    // Startup code: if we can't find a writable data folder there is no point
    // in running, so errors here end the app.
    let data_dir = storage::DataDir::resolve().context("no writable data folder found")?;

    // Auto-updater: "Restart now" starts us with `--wait-for-exit <pid>`;
    // wait until the old Athena has closed all its files. Then install a
    // downloaded update (or roll back a broken one) before anything else
    // opens a file. See src/updater/install.rs.
    let args: Vec<String> = std::env::args().collect();
    if let Some(pid) = args
        .iter()
        .position(|arg| arg == "--wait-for-exit")
        .and_then(|i| args.get(i + 1))
        .and_then(|pid| pid.parse().ok())
    {
        updater::install::wait_for_exit(pid);
    }
    let exe = std::env::current_exe().context("can't find the exe's location")?;
    let exe_dir = exe.parent().context("the exe has no parent folder")?;
    let update_info =
        match updater::install::on_startup(&data_dir.path, exe_dir, version::BUILT_VERSION) {
            updater::Startup::LaunchAndExit => {
                updater::install::launch(exe_dir)?;
                return Ok(());
            }
            updater::Startup::Continue(info) => info,
        };

    let saver = storage::Saver::start();
    let (settings, settings_warning) = settings::Settings::load(&data_dir.path);

    // Before asking Windows about monitors, see `become_dpi_aware`.
    window_place::become_dpi_aware();
    // Where the window was last time, if it still fits on the monitors
    // connected now. Reading the small layout file here is fine: the window
    // doesn't exist yet.
    let window_placement =
        layout::saved_window_placement(&data_dir.path).and_then(window_place::usable);

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Athena")
            // The first time, start maximized. The inner size is what Windows
            // uses when the user un-maximizes the window. Sizes are in logical
            // points, so Windows display scaling (125%, 150%) is applied
            // automatically; 1280x720 points still fits a 1080p screen at 150%.
            // With a saved placement, the window starts normal and hidden and
            // `AthenaApp::new` moves it into place before it is shown.
            .with_maximized(window_placement.is_none())
            .with_inner_size([1280.0, 720.0])
            .with_min_inner_size([800.0, 600.0]),
        renderer: renderer(),
        ..Default::default()
    };

    eframe::run_native(
        "Athena",
        options,
        Box::new(move |cc| {
            let main_window = window_place::MainWindow::new(cc);
            if let Some(placement) = window_placement {
                main_window.restore(placement);
            }
            Ok(Box::new(app::AthenaApp::new(
                &cc.egui_ctx,
                main_window,
                data_dir,
                settings,
                settings_warning,
                saver,
                update_info,
            )))
        }),
    )
    // eframe's error type isn't `Send + Sync`, so turn it into text for anyhow.
    .map_err(|e| anyhow::anyhow!("failed to start the window: {e}"))
}

/// Which GPU backend draws the window. glow (OpenGL) is the default; the
/// `wgpu` cargo feature switches to wgpu so the two can be measured.
/// `#[cfg(...)]` picks one of the two functions at compile time.
#[cfg(not(feature = "wgpu"))]
fn renderer() -> eframe::Renderer {
    eframe::Renderer::Glow
}

#[cfg(feature = "wgpu")]
fn renderer() -> eframe::Renderer {
    eframe::Renderer::Wgpu
}
