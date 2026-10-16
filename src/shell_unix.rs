//! Linux masaüstü entegrasyonu: freedesktop ikon teması, küçük resimler, tür adları.
//! Windows'taki yerel sağ tık menüsü ve OLE sürükle-bırak burada yok; uygulamanın kendi menüsü kullanılır.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub use crate::images::{ImageCache, Img};

/// Görüntü iş parçacığı başlarken bir kez çağrılır (Linux'ta hazırlık gerekmez).
pub fn init_worker_thread() {}

/// Sistem ölçeği; Linux'ta pencere açılmadan bilinemez
pub fn system_scale() -> f32 {
    1.0
}

fn themes() -> &'static [String] {
    static THEMES: OnceLock<Vec<String>> = OnceLock::new();
    THEMES.get_or_init(|| {
        let mut v = Vec::new();
        if let Some(t) = freedesktop_icons::default_theme_gtk() {
            v.push(t);
        }
        for t in ["Adwaita", "breeze", "Papirus", "hicolor"] {
            if !v.iter().any(|x| x == t) {
                v.push(t.to_string());
            }
        }
        v
    })
}

fn find_icon(names: &[&str], size: i32) -> Option<PathBuf> {
    for theme in themes() {
        for name in names {
            if let Some(p) = freedesktop_icons::lookup(name).with_size(size.clamp(16, 512) as u16).with_theme(theme).with_cache().find() {
                return Some(p);
            }
        }
    }
    None
}

fn load_icon(path: &Path, size: i32) -> Option<egui::ColorImage> {
    let is_svg = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("svg") || e.eq_ignore_ascii_case("svgz"));
    if is_svg {
        let data = std::fs::read(path).ok()?;
        let tree = resvg::usvg::Tree::from_data(&data, &resvg::usvg::Options::default()).ok()?;
        let side = size.max(1) as u32;
        let mut pm = resvg::tiny_skia::Pixmap::new(side, side)?;
        let sz = tree.size();
        let scale = side as f32 / sz.width().max(sz.height());
        resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pm.as_mut());
        Some(egui::ColorImage::from_rgba_premultiplied([side as usize, side as usize], pm.data()))
    } else {
        decode(path, size)
    }
}

fn decode(path: &Path, size: i32) -> Option<egui::ColorImage> {
    let img = image::ImageReader::open(path).ok()?.with_guessed_format().ok()?.decode().ok()?;
    let side = size.max(1) as u32;
    let img = if img.width() > side || img.height() > side { img.thumbnail(side, side) } else { img };
    let rgba = img.to_rgba8();
    Some(egui::ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw()))
}

/// Sanal konumların (Giriş, Bu bilgisayar, Ağ) ve özel klasörlerin tema ikon adları
fn special_names(path: &Path) -> Option<&'static [&'static str]> {
    let s = path.as_os_str();
    if s == crate::tab::THIS_PC {
        return Some(&["computer", "computer-symbolic"]);
    }
    if s.to_string_lossy().starts_with("::") {
        return Some(&["user-home", "go-home"]);
    }
    if path == Path::new("/") {
        return Some(&["drive-harddisk", "drive-harddisk-system"]);
    }
    let known: [(Option<PathBuf>, &'static [&'static str]); 7] = [
        (dirs::home_dir(), &["user-home", "folder-home"]),
        (dirs::desktop_dir(), &["user-desktop", "folder-desktop"]),
        (dirs::download_dir(), &["folder-download", "folder-downloads"]),
        (dirs::document_dir(), &["folder-documents"]),
        (dirs::picture_dir(), &["folder-pictures", "folder-images"]),
        (dirs::audio_dir(), &["folder-music"]),
        (dirs::video_dir(), &["folder-videos", "folder-video"]),
    ];
    known.into_iter().find(|(p, _)| p.as_deref() == Some(path)).map(|(_, n)| n)
}

fn is_mount_point(path: &Path) -> bool {
    crate::win::mount_roots().iter().any(|p| p == path)
}

/// Tema ikonu ya da küçük resim
pub fn shell_image(path: &Path, size: i32, icon_only: bool) -> Option<egui::ColorImage> {
    if let Some(names) = special_names(path) {
        let mut v: Vec<&str> = names.to_vec();
        v.push("folder");
        return load_icon(&find_icon(&v, size)?, size);
    }
    if path.is_dir() {
        let names: &[&str] = if is_mount_point(path) { &["drive-removable-media", "drive-harddisk", "folder"] } else { &["folder"] };
        return load_icon(&find_icon(names, size)?, size);
    }
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if !icon_only && matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp") {
        if let Some(img) = decode(path, size) {
            return Some(img);
        }
    }
    let mime = mime_guess::from_ext(&ext).first_raw().unwrap_or("application/octet-stream");
    let exact = mime.replace('/', "-");
    let top = mime.split('/').next().unwrap_or("");
    let generic = match top {
        "image" => "image-x-generic",
        "audio" => "audio-x-generic",
        "video" => "video-x-generic",
        "text" => "text-x-generic",
        "font" => "font-x-generic",
        _ if matches!(ext.as_str(), "zip" | "rar" | "7z" | "tar" | "gz" | "xz" | "bz2" | "zst") => "package-x-generic",
        _ if matches!(ext.as_str(), "sh" | "appimage" | "run" | "bin") => "application-x-executable",
        _ => "text-x-generic",
    };
    load_icon(&find_icon(&[&exact, generic, "text-x-generic", "unknown"], size)?, size)
}

/// Bir uygulamanın ikonu (`.desktop` dosyasındaki `Icon=`: tema adı ya da mutlak yol); bulunamazsa genel uygulama ikonu
pub fn app_icon(icon: Option<&str>, size: i32) -> Option<egui::ColorImage> {
    let path = match icon {
        Some(i) if i.starts_with('/') => Some(PathBuf::from(i)),
        Some(i) => find_icon(&[i], size),
        None => None,
    };
    let path = path.filter(|p| p.exists()).or_else(|| find_icon(&["application-x-executable", "application-default-icon"], size))?;
    load_icon(&path, size)
}

/// "PNG dosyası" gibi tür adı
pub fn type_name(ext: &str) -> String {
    if ext.is_empty() {
        return crate::i18n::t("Dosya").into();
    }
    crate::i18n::tf("{} Dosyası", &[&ext.to_uppercase()])
}

#[allow(dead_code)]
pub enum MenuResult {
    None,
    Rename,
    Invoked,
}

/// Sistem sağ tık menüsü Linux'ta yok.
pub fn context_menu(_hwnd: isize, _paths: &[PathBuf], _extended: bool) -> MenuResult {
    MenuResult::None
}

pub fn drag_out(_paths: &[PathBuf]) -> bool {
    false
}

pub fn cursor_outside_window(_hwnd: isize) -> bool {
    false
}

pub fn accent_color() -> Option<egui::Color32> {
    None
}
