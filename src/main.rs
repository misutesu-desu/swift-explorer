#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
#[cfg(not(windows))]
mod desktop_apps;
mod fsutil;
mod i18n;
mod images;
mod properties;
mod scheme;
#[cfg(feature = "screenshot")]
mod screenshot;
mod tab;
#[cfg_attr(not(windows), path = "shell_unix.rs")]
mod shell;
#[cfg_attr(not(windows), path = "win_unix.rs")]
mod win;

use std::path::PathBuf;

pub const APP_NAME: &str = "Swift Explorer";
/// Wayland/X11 pencere kimliği; `.desktop` dosyasının adıyla aynı olmalı ki masaüstü ikonu ve adı eşleştirsin
pub const APP_ID: &str = "io.github.misutesu_desu.SwiftExplorer";

fn main() -> eframe::Result<()> {
    let start = std::env::args().nth(1).map(PathBuf::from);
    // İlk çalıştırmada (kayıtlı pencere konumu yoksa) ekranı kaplayarak aç
    let first_run = eframe::storage_dir(APP_NAME).is_none_or(|d| !d.join("app.ron").exists());
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_maximized(first_run)
            .with_title(APP_NAME)
            .with_app_id(APP_ID)
            // Başlık çubuğu yok: sekmeler ve pencere düğmeleri Explorer gibi en üstte
            .with_decorations(false)
            .with_inner_size([1060.0, 640.0])
            .with_min_inner_size([640.0, 400.0])
            .with_drag_and_drop(true)
            .with_icon(eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")).unwrap_or_default()),
        ..Default::default()
    };
    #[cfg(windows)]
    shell::begin_startup_cloak();
    app::prefetch_startup(start.clone());
    #[cfg(feature = "screenshot")]
    screenshot::init();
    eframe::run_native(APP_NAME, options, Box::new(move |cc| Ok(Box::new(app::App::new(cc, start)))))
}
