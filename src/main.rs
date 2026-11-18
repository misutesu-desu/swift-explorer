#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod archive;
#[cfg(not(windows))]
mod desktop_apps;
mod folder_lock;
mod fsutil;
mod i18n;
mod images;
mod palette;
mod rename;
#[cfg(not(windows))]
mod properties;
mod scheme;
#[cfg(feature = "screenshot")]
mod screenshot;
mod tab;
mod update;
#[cfg_attr(not(windows), path = "shell_unix.rs")]
mod shell;
#[cfg_attr(not(windows), path = "win_unix.rs")]
mod win;

use std::path::PathBuf;

pub const APP_NAME: &str = "Swift Explorer";
/// Wayland/X11 pencere kimliği; `.desktop` dosyasının adıyla aynı olmalı ki masaüstü ikonu ve adı eşleştirsin
pub const APP_ID: &str = "io.github.misutesu_desu.SwiftExplorer";

/// Kaldırma betikleri için: varsayılan dosya yöneticisi ve otomatik başlatma kaydını geri alır
const UNREGISTER_ARG: &str = "--unregister";

fn main() -> eframe::Result<()> {
    let arg = std::env::args().nth(1);
    if arg.as_deref() == Some(UNREGISTER_ARG) {
        app::unregister();
        return Ok(());
    }
    let after_update = arg.as_deref() == Some(update::AFTER_UPDATE_ARG);
    let restart = arg.as_deref() == Some(update::RESTART_ARG);
    let hidden = after_update || arg.as_deref() == Some(win::BACKGROUND_ARG);
    if after_update || restart {
        // Eski sürüm boruyu bırakıp çıkana dek bekle
        for _ in 0..100 {
            if !win::is_serving() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
    // Explorer sürücüyü "%1" ile "C:\" diye verir; Windows bunu C:" olarak ayrıştırır
    let start = arg.filter(|_| !hidden && !restart).map(|a| PathBuf::from(a.trim_end_matches('"')));
    if hidden && win::is_serving() || !hidden && win::hand_off(start.as_deref()) {
        return Ok(());
    }
    // Açılışı başka sürece devredenler sayılmasın diye burada
    update::startup_guard();
    // İlk çalıştırmada (kayıtlı pencere konumu yoksa) ekranı kaplayarak aç
    let first_run = eframe::storage_dir(APP_NAME).is_none_or(|d| !d.join("app.ron").exists());
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_maximized(first_run)
            // Oturum açılışında gizli başlarken kullanıcının penceresinden odağı almasın
            .with_active(!hidden)
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
    eframe::run_native(APP_NAME, options, Box::new(move |cc| Ok(Box::new(app::App::new(cc, start, hidden)))))
}
