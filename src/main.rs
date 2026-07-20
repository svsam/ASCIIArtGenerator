#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("ASCII Art Generator")
            .with_inner_size([1_400.0, 900.0])
            .with_min_inner_size([900.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native(
        "ASCII Art Generator",
        options,
        Box::new(|creation_context| Ok(Box::new(app::AsciiArtApp::new(creation_context)))),
    )
}
