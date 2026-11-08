// On Windows, build as a GUI app so no console window opens next to Athena.
// Debug builds keep the console so `println!`/panic messages stay visible while developing.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod layout;
mod modules;
mod settings;
mod storage;

use anyhow::{Context, Result};

fn main() -> Result<()> {
    // Startup code: if we can't find a writable data folder there is no point
    // in running, so errors here end the app.
    let data_dir = storage::DataDir::resolve().context("no writable data folder found")?;
    let saver = storage::Saver::start();
    let (settings, settings_warning) = settings::Settings::load(&data_dir.path);

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Athena")
            // Start maximized. The inner size is what Windows uses when the
            // user un-maximizes the window. Sizes are in logical points, so
            // Windows display scaling (125%, 150%) is applied automatically.
            .with_maximized(true)
            .with_inner_size([1600.0, 900.0])
            .with_min_inner_size([800.0, 600.0]),
        renderer: renderer(),
        ..Default::default()
    };

    eframe::run_native(
        "Athena",
        options,
        Box::new(move |cc| {
            Ok(Box::new(app::AthenaApp::new(
                &cc.egui_ctx,
                data_dir,
                settings,
                settings_warning,
                saver,
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
