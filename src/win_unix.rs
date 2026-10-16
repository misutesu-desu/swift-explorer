//! Linux karşılıkları: bağlama noktaları, disk alanı, dosya yöneticisi ve terminal açma, pano.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};

#[derive(Clone)]
pub struct DriveInfo {
    pub root: PathBuf,
    pub label: String,
    pub free: u64,
    pub total: u64,
}

/// /proc/mounts alanlarındaki kaçışlar: boşluk "\040" olarak yazılır.
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 3 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 4], 8) {
                out.push(v);
                i += 4;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Kök ve kullanıcının taktığı diskler (/media, /run/media, /mnt altındakiler)
pub fn mount_roots() -> Vec<PathBuf> {
    let mut v = vec![PathBuf::from("/")];
    let Ok(text) = std::fs::read_to_string("/proc/self/mounts") else { return v };
    for line in text.lines() {
        let mut f = line.split_whitespace();
        let (Some(_dev), Some(mnt), Some(fstype)) = (f.next(), f.next(), f.next()) else { continue };
        let mnt = unescape(mnt);
        let user_mount = mnt.starts_with("/media/") || mnt.starts_with("/run/media/") || mnt.starts_with("/mnt/");
        let virtual_fs = matches!(fstype, "tmpfs" | "proc" | "sysfs" | "devtmpfs" | "autofs" | "squashfs" | "overlay" | "fuse.portal");
        if user_mount && !virtual_fs && !v.iter().any(|p| p.as_os_str() == mnt.as_str()) {
            v.push(PathBuf::from(mnt));
        }
    }
    v
}

pub fn drives() -> Vec<DriveInfo> {
    mount_roots()
        .into_iter()
        .map(|root| {
            let label = if root == Path::new("/") {
                format!("{} (/)", crate::i18n::t("Yerel Disk"))
            } else {
                root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| root.display().to_string())
            };
            let (free, total) = disk_space(&root).unwrap_or((0, 0));
            DriveInfo { root, label, free, total }
        })
        .collect()
}

/// (boş, toplam) bayt
pub fn disk_space(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    let frsize = if st.f_frsize > 0 { st.f_frsize as u64 } else { st.f_bsize as u64 };
    Some((st.f_bavail as u64 * frsize, st.f_blocks as u64 * frsize))
}

fn file_uri(p: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut s = String::from("file://");
    for &b in p.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            s.push(b as char);
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s
}

/// freedesktop FileManager1 arayüzü (Nautilus, Dolphin, Nemo, Thunar destekler)
fn file_manager_call(method: &str, path: &Path) -> bool {
    Command::new("dbus-send")
        .args([
            "--session",
            "--dest=org.freedesktop.FileManager1",
            "--type=method_call",
            "/org/freedesktop/FileManager1",
            &format!("org.freedesktop.FileManager1.{method}"),
            &format!("array:string:{}", file_uri(path)),
            "string:",
        ])
        .status()
        .is_ok_and(|s| s.success())
}

/// Dosyanın MIME türü: içeriğe bakan `xdg-mime`, olmazsa uzantıdan
pub fn mime_of(path: &Path) -> Option<String> {
    xdg_mime(&["query", "filetype", &path.display().to_string()]).or_else(|| mime_guess::from_path(path).first_raw().map(String::from))
}

fn xdg_mime(args: &[&str]) -> Option<String> {
    let out = host_command("xdg-mime").args(args).output().ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !s.is_empty()).then_some(s)
}

/// Bir MIME türünün varsayılan uygulamasının `.desktop` kimliği
pub fn default_app_id(mime: &str) -> Option<String> {
    xdg_mime(&["query", "default", mime])
}

/// Dosya türünü açan varsayılan uygulama: `xdg-mime` ile bulunur, adı `.desktop` dosyasından okunur
pub fn default_app(path: &Path) -> Option<crate::properties::AppInfo> {
    let mime = mime_of(path)?;
    let id = default_app_id(&mime)?;
    let name = crate::desktop_apps::app_name(&id).unwrap_or_else(|| id.trim_end_matches(".desktop").to_string());
    Some(crate::properties::AppInfo { name, exe: None })
}

pub fn open_with(path: &Path) {
    let _ = open::that_detached(path);
}

pub fn show_in_explorer(path: &Path) {
    if !file_manager_call("ShowItems", path) {
        if let Some(dir) = path.parent() {
            let _ = open::that_detached(dir);
        }
    }
}

/// Flatpak sandbox'ında mı: host'taki programlar orada doğrudan görünmez
fn in_flatpak() -> bool {
    static FLATPAK: OnceLock<bool> = OnceLock::new();
    *FLATPAK.get_or_init(|| Path::new("/.flatpak-info").exists())
}

/// Host'taki bir programı çalıştıran komut; Flatpak içinde `flatpak-spawn --host` üzerinden
pub fn host_command(program: &str) -> Command {
    host_command_in(program, None)
}

fn host_command_in(program: &str, dir: Option<&Path>) -> Command {
    if in_flatpak() {
        let mut c = Command::new("flatpak-spawn");
        c.arg("--host");
        if let Some(d) = dir {
            c.arg(format!("--directory={}", d.display()));
        }
        c.arg(program);
        c
    } else {
        let mut c = Command::new(program);
        if let Some(d) = dir {
            c.current_dir(d);
        }
        c
    }
}

/// Program host'ta var mı. Flatpak dışında bunu `spawn` hatası zaten söyler; içinde
/// `flatpak-spawn` her zaman başlar, bu yüzden önce sorulur.
fn host_has(program: &str) -> bool {
    !in_flatpak() || host_command("sh").args(["-c", "command -v \"$1\" >/dev/null", "sh", program]).status().is_ok_and(|s| s.success())
}

const TERMINALS: [&str; 11] = ["x-terminal-emulator", "gnome-terminal", "kgx", "ptyxis", "konsole", "xfce4-terminal", "kitty", "alacritty", "foot", "wezterm", "xterm"];

pub fn open_terminal(dir: &Path) -> bool {
    let mut candidates: Vec<String> = std::env::var("TERMINAL").into_iter().collect();
    candidates.extend(TERMINALS.map(String::from));
    candidates.iter().any(|c| host_has(c) && host_command_in(c, Some(dir)).spawn().is_ok())
}

/// Adres çubuğuna yazılınca o klasörde açılan terminaller: "terminal" varsayılanı açar
pub fn is_terminal(name: &str) -> bool {
    name == "terminal" || TERMINALS.contains(&name) || std::env::var("TERMINAL").is_ok_and(|t| t == name)
}

/// `name` terminalini `dir` klasöründe, varsa `args` ile başlatır
pub fn run_terminal(name: &str, args: &str, dir: &Path) -> bool {
    if name == "terminal" {
        return open_terminal(dir);
    }
    host_has(name) && host_command_in(name, Some(dir)).args(args.split_whitespace()).spawn().is_ok()
}

/// Windows'ta tuş doğrudan sorulur; burada Ctrl+V egui'nin yapıştırma olayından gelir.
pub fn ctrl_v_down() -> bool {
    false
}

// ---------- Pano: dosyalar metin olarak (her satıra bir yol ya da file:// adresi)

static PASTE_TEXT: Mutex<Option<String>> = Mutex::new(None);

/// Ctrl+V ile gelen pano metni; bir sonraki `clipboard_get_files` çağrısı onu okur.
pub fn note_paste_text(text: &str) {
    *PASTE_TEXT.lock().unwrap() = Some(text.to_string());
}

/// Panoya yazılacak metin: dosya yöneticileri bunu yol listesi olarak tanır.
pub fn files_as_text(paths: &[PathBuf]) -> String {
    paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("\n")
}

pub fn clipboard_set_files(_paths: &[PathBuf], _cut: bool) -> bool {
    false
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// (dosyalar, kesme_mi)
pub fn clipboard_get_files() -> Option<(Vec<PathBuf>, bool)> {
    let text = PASTE_TEXT.lock().unwrap().take()?;
    let mut cut = false;
    let mut files = Vec::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        match line {
            "cut" => cut = true,
            "copy" | "x-special/nautilus-clipboard" => {}
            _ => {
                let p = match line.strip_prefix("file://") {
                    Some(rest) => PathBuf::from(percent_decode(rest)),
                    None => PathBuf::from(line),
                };
                if !p.is_absolute() || !p.exists() {
                    return None;
                }
                files.push(p);
            }
        }
    }
    (!files.is_empty()).then_some((files, cut))
}
