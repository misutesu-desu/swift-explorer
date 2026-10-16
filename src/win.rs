//! İnce Win32 sarmalayıcıları: sürücüler, disk alanı, özellikler penceresi, pano.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use windows_sys::Win32::Storage::FileSystem::{
    GetDiskFreeSpaceExW, GetDiskFreeSpaceW, GetDriveTypeW, GetFileAttributesW, GetLogicalDrives, GetVolumeInformationW, SetFileAttributesW,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows_sys::Win32::UI::Shell::{AssocQueryStringW, ShellExecuteExW, SHELLEXECUTEINFOW};

const CREATE_NEW_CONSOLE: u32 = 0x10;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

#[derive(Clone)]
pub struct DriveInfo {
    pub root: PathBuf,
    pub label: String,
    pub free: u64,
    pub total: u64,
}

pub fn drives() -> Vec<DriveInfo> {
    let mask = unsafe { GetLogicalDrives() };
    let mut out = Vec::new();
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = (b'A' + i as u8) as char;
        let root = PathBuf::from(format!("{letter}:\\"));
        let w = wide(root.as_os_str());
        let kind = unsafe { GetDriveTypeW(w.as_ptr()) };
        let kind_name = match kind {
            2 => crate::i18n::t("Çıkarılabilir disk"),
            3 => crate::i18n::t("Yerel Disk"),
            4 => crate::i18n::t("Ağ sürücüsü"),
            5 => "CD/DVD",
            6 => "RAM disk",
            _ => crate::i18n::t("Sürücü"),
        };
        let mut name_buf = [0u16; 261];
        let ok = unsafe {
            GetVolumeInformationW(
                w.as_ptr(),
                name_buf.as_mut_ptr(),
                name_buf.len() as u32,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                0,
            )
        };
        let vol = if ok != 0 {
            let len = name_buf.iter().position(|&c| c == 0).unwrap_or(0);
            String::from_utf16_lossy(&name_buf[..len])
        } else {
            String::new()
        };
        let label = if vol.is_empty() {
            format!("{kind_name} ({letter}:)")
        } else {
            format!("{vol} ({letter}:)")
        };
        let (free, total) = disk_space(&root).unwrap_or((0, 0));
        out.push(DriveInfo { root, label, free, total });
    }
    out
}

/// (boş, toplam) bayt
pub fn disk_space(path: &Path) -> Option<(u64, u64)> {
    let w = wide(path.as_os_str());
    let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
    let ok = unsafe { GetDiskFreeSpaceExW(w.as_ptr(), &mut avail, &mut total, &mut free) };
    (ok != 0).then_some((avail, total))
}

/// Windows'un kendi "Özellikler" penceresini açar; `tab` verilirse o sekmede ("Security" gibi)
pub fn show_properties(path: &Path, tab: Option<&str>) {
    let file = wide(path.as_os_str());
    let verb = wide(OsStr::new("properties"));
    let params = tab.map(|t| wide(OsStr::new(t)));
    unsafe {
        let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = 0x0000_000C; // SEE_MASK_INVOKEIDLIST
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        if let Some(p) = &params {
            info.lpParameters = p.as_ptr();
        }
        info.nShow = 5; // SW_SHOW
        ShellExecuteExW(&mut info);
    }
}

/// Dosya türünü açan varsayılan uygulama (görünen adı ve exe yolu)
pub fn default_app(path: &Path) -> Option<crate::properties::AppInfo> {
    const ASSOCSTR_EXECUTABLE: i32 = 2;
    const ASSOCSTR_FRIENDLYAPPNAME: i32 = 4;
    let ext = wide(OsStr::new(&format!(".{}", path.extension()?.to_string_lossy())));
    let query = |what: i32| -> Option<String> {
        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        let hr = unsafe { AssocQueryStringW(0, what, ext.as_ptr(), std::ptr::null(), buf.as_mut_ptr(), &mut len) };
        (hr == 0 && len > 1).then(|| String::from_utf16_lossy(&buf[..len as usize - 1]))
    };
    let name = query(ASSOCSTR_FRIENDLYAPPNAME)?;
    Some(crate::properties::AppInfo { name, exe: query(ASSOCSTR_EXECUTABLE).map(PathBuf::from) })
}

/// Authenticode imzası; imzasızsa boş liste. PowerShell'den okunur (arka plan iş parçacığında çağrılır).
pub fn signatures(path: &Path) -> Vec<crate::properties::Signer> {
    const SCRIPT: &str = "[Console]::OutputEncoding=[Text.Encoding]::UTF8; \
        $s = Get-AuthenticodeSignature -LiteralPath $env:SE_PATH; $c = $s.SignerCertificate; \
        [pscustomobject]@{ status=[string]$s.Status; message=$s.StatusMessage; \
        signer=$(if($c){$c.GetNameInfo('SimpleName',$false)}else{''}); \
        algo=$(if($c){$c.SignatureAlgorithm.FriendlyName}else{''}) } | ConvertTo-Json -Compress";
    let Ok(out) = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT])
        .env("SE_PATH", path)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&out.stdout) else { return Vec::new() };
    let field = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    if field("status") == "NotSigned" || field("signer").is_empty() {
        return Vec::new();
    }
    // "sha256RSA" → "sha256": imza algoritmasından özet algoritması
    let algo = field("algo");
    let digest = algo.strip_suffix("RSA").or_else(|| algo.strip_suffix("ECDSA")).unwrap_or(&algo).to_string();
    vec![crate::properties::Signer { name: field("signer"), digest, message: field("message") }]
}

/// Dosyanın bulunduğu birimin küme boyutu (diskte kapladığı yeri hesaplamak için)
pub fn cluster_size(path: &Path) -> Option<u64> {
    let root = wide(path.ancestors().last()?.as_os_str());
    let (mut sectors, mut bytes, mut free, mut total) = (0u32, 0u32, 0u32, 0u32);
    let ok = unsafe { GetDiskFreeSpaceW(root.as_ptr(), &mut sectors, &mut bytes, &mut free, &mut total) };
    (ok != 0).then(|| sectors as u64 * bytes as u64)
}

/// Salt okunur / gizli özniteliklerini ayarlar; `None` olan değişmez
pub fn set_attributes(path: &Path, readonly: Option<bool>, hidden: Option<bool>) -> std::io::Result<()> {
    const READONLY: u32 = 0x1;
    const HIDDEN: u32 = 0x2;
    const NORMAL: u32 = 0x80;
    let w = wide(path.as_os_str());
    unsafe {
        let cur = GetFileAttributesW(w.as_ptr());
        if cur == u32::MAX {
            return Err(std::io::Error::last_os_error());
        }
        let mut attrs = cur;
        for (bit, v) in [(READONLY, readonly), (HIDDEN, hidden)] {
            match v {
                Some(true) => attrs |= bit,
                Some(false) => attrs &= !bit,
                None => {}
            }
        }
        // FILE_ATTRIBUTE_NORMAL tek başına geçerlidir
        if attrs & !NORMAL == 0 {
            attrs = NORMAL;
        } else {
            attrs &= !NORMAL;
        }
        if SetFileAttributesW(w.as_ptr(), attrs) == 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

pub fn open_with(path: &Path) {
    let _ = Command::new("rundll32.exe")
        .raw_arg(format!("shell32.dll,OpenAs_RunDLL {}", path.display()))
        .spawn();
}

/// Windows'un "Birlikte aç" penceresini açar ve kapanana kadar bekler; "Her zaman" seçilirse
/// dosya türünün varsayılan uygulaması değişir.
pub fn choose_default_app(path: &Path) {
    let _ = Command::new("rundll32.exe").raw_arg(format!("shell32.dll,OpenAs_RunDLL {}", path.display())).status();
}

pub fn show_in_explorer(path: &Path) {
    let _ = Command::new("explorer.exe")
        .raw_arg(format!("/select,\"{}\"", path.display()))
        .spawn();
}

pub fn open_terminal(dir: &Path) {
    let wt = Command::new("wt.exe")
        .arg("-d")
        .arg(dir)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
    if wt.is_err() {
        let _ = Command::new("powershell.exe")
            .current_dir(dir)
            .creation_flags(CREATE_NEW_CONSOLE)
            .spawn();
    }
}

/// Adres çubuğuna yazılınca o klasörde açılan terminaller (Explorer'daki gibi);
/// "terminal" Windows Terminal'i, yoksa PowerShell'i açar
const TERMINALS: [&str; 5] = ["terminal", "cmd", "powershell", "pwsh", "wt"];

pub fn is_terminal(name: &str) -> bool {
    let name = name.to_lowercase();
    TERMINALS.contains(&name.strip_suffix(".exe").unwrap_or(&name))
}

/// `name` terminalini `dir` klasöründe, varsa `args` ile başlatır
pub fn run_terminal(name: &str, args: &str, dir: &Path) -> bool {
    if name.eq_ignore_ascii_case("terminal") {
        open_terminal(dir);
        return true;
    }
    let mut cmd = Command::new(name);
    if name.to_lowercase().starts_with("wt") {
        cmd.arg("-d").arg(dir).creation_flags(CREATE_NO_WINDOW);
    } else {
        cmd.current_dir(dir).creation_flags(CREATE_NEW_CONSOLE);
    }
    if !args.is_empty() {
        cmd.raw_arg(args);
    }
    cmd.spawn().is_ok()
}

/// egui, panoda metin yoksa Ctrl+V için hiç olay üretmiyor; bu yüzden tuşu doğrudan soruyoruz.
pub fn ctrl_v_down() -> bool {
    unsafe {
        let ctrl = GetAsyncKeyState(0x11) as u16 & 0x8000 != 0;
        let v = GetAsyncKeyState(0x56) as u16 & 0x8000 != 0;
        ctrl && v
    }
}

// ---------- Pano (CF_HDROP) — Explorer ile karşılıklı kopyala/yapıştır ----------

const DROPEFFECT_COPY: u32 = 1;
const DROPEFFECT_MOVE: u32 = 2;

pub fn clipboard_set_files(paths: &[PathBuf], cut: bool) -> bool {
    use clipboard_win::{options::NoClear, raw, Clipboard};
    let Ok(_clip) = Clipboard::new_attempts(10) else { return false };
    if raw::empty().is_err() {
        return false;
    }
    let list: Vec<String> = paths.iter().map(|p| p.to_string_lossy().into_owned()).collect();
    if raw::set_file_list_with(&list, NoClear).is_err() {
        return false;
    }
    if let Some(fmt) = raw::register_format("Preferred DropEffect") {
        let effect = if cut { DROPEFFECT_MOVE } else { DROPEFFECT_COPY };
        let _ = raw::set_without_clear(fmt.get(), &effect.to_le_bytes());
    }
    true
}

/// (dosyalar, kesme_mi)
pub fn clipboard_get_files() -> Option<(Vec<PathBuf>, bool)> {
    use clipboard_win::{raw, Clipboard};
    let _clip = Clipboard::new_attempts(10).ok()?;
    let mut files = Vec::new();
    raw::get_file_list_path(&mut files).ok()?;
    if files.is_empty() {
        return None;
    }
    let mut cut = false;
    if let Some(fmt) = raw::register_format("Preferred DropEffect") {
        let mut buf = Vec::new();
        if raw::get_vec(fmt.get(), &mut buf).is_ok() && buf.len() >= 4 {
            let v = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
            cut = v & DROPEFFECT_MOVE != 0 && v & DROPEFFECT_COPY == 0;
        }
    }
    Some((files, cut))
}
