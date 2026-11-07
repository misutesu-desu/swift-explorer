//! İnce Win32 sarmalayıcıları: sürücüler, disk alanı, özellikler penceresi, pano.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use windows_sys::Win32::Storage::FileSystem::{
    GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows_sys::Win32::UI::Shell::{ShellExecuteExW, SHELLEXECUTEINFOW};
use windows_sys::Win32::Foundation::{BOOL, ERROR_MORE_DATA, FILETIME, HWND, LPARAM};
use windows_sys::Win32::System::RestartManager::{
    RmEndSession, RmGetList, RmRegisterResources, RmService, RmStartSession, CCH_RM_SESSION_KEY, RM_PROCESS_INFO,
};
use windows_sys::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, TerminateProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindow, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, GW_OWNER, WM_CLOSE,
};

use crate::fsutil::Locker;

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

/// Takılı sürücü harflerinin bit maskesi. USB takılıp çıkarılınca değişir, ucuzdur.
pub fn drive_mask() -> u64 {
    unsafe { GetLogicalDrives() as u64 }
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

/// Windows'un kendi "Özellikler" penceresini açar.
pub fn show_properties(path: &Path) {
    let file = wide(path.as_os_str());
    let verb = wide(OsStr::new("properties"));
    unsafe {
        let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = 0x0000_000C; // SEE_MASK_INVOKEIDLIST
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.nShow = 5; // SW_SHOW
        ShellExecuteExW(&mut info);
    }
}

/// Kabuğa açtırır: .msc gibi yönetici izni isteyen hedeflerde UAC penceresi de böyle çıkar.
/// UAC onayı beklerken arayüz donmasın diye ayrı iş parçacığında.
pub fn shell_open(target: &str) {
    let file = wide(OsStr::new(target));
    std::thread::spawn(move || unsafe {
        let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.lpFile = file.as_ptr();
        info.nShow = 5; // SW_SHOW
        ShellExecuteExW(&mut info);
    });
}

/// Explorer'ın "Ağ sürücüsüne bağlan" ya da "Bağlantıyı kes" penceresi
pub fn network_drive_dialog(disconnect: bool) {
    let which = if disconnect { "Disconnect" } else { "Connect" };
    let _ = Command::new("rundll32.exe")
        .raw_arg(format!("shell32.dll,SHHelpShortcuts_RunDLL {which}"))
        .spawn();
}

pub fn open_with(path: &Path) {
    let _ = Command::new("rundll32.exe")
        .raw_arg(format!("shell32.dll,OpenAs_RunDLL {}", path.display()))
        .spawn();
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

// ---------- Arka planda bekleme: Explorer gibi anında açılmak için ----------
//
// Pencere ve OpenGL sürücüsünün açılışı bu uygulamanın kendi açılışından katbekat uzun
// sürüyor; hibrit grafikli dizüstülerde bir saniyeyi geçiyor. Bu yüzden pencere kapanınca
// süreç gizlenip bekler, sonraki açılış ona devredilir ve pencere hemen görünür.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::FromRawHandle;
use std::sync::atomic::{AtomicIsize, Ordering::SeqCst};
use std::sync::Mutex;

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX};
use windows_sys::Win32::System::Pipes::{ConnectNamedPipe, CreateNamedPipeW, WaitNamedPipeW, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT};
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows_sys::Win32::UI::WindowsAndMessaging::{AllowSetForegroundWindow, IsIconic, SetForegroundWindow, ShowWindow, ASFW_ANY, SW_HIDE, SW_RESTORE, SW_SHOW};

/// Gizlenip bekleyen pencere; 0 ise pencere açık ya da bekleme kapalı
static PARKED: AtomicIsize = AtomicIsize::new(0);
/// Devralınan açılış isteği: açılacak klasör (yoksa son sekmeler)
static REOPEN: Mutex<Option<Option<PathBuf>>> = Mutex::new(None);

/// Aynı oturumdaki aynı kullanıcıya özel boru adı
fn pipe_name() -> Vec<u16> {
    let mut session = 0;
    unsafe { ProcessIdToSessionId(std::process::id(), &mut session) };
    let user = std::env::var("USERNAME").unwrap_or_default();
    wide(OsStr::new(&format!(r"\\.\pipe\SwiftExplorer-{session}-{user}")))
}

/// Gizlenip bekleyen bir süreç varsa açılışı ona devreder. `true` dönerse pencere
/// o süreçte açıldı, bu süreç çıkabilir.
pub fn hand_off(start: Option<&Path>) -> bool {
    let Some(mut pipe) = connect() else { return false };
    let dir: Vec<u16> = start.and_then(|p| std::path::absolute(p).ok()).map(|p| p.as_os_str().encode_wide().collect()).unwrap_or_default();
    let mut msg = ((dir.len() * 2) as u32).to_le_bytes().to_vec();
    msg.extend(dir.iter().flat_map(|c| c.to_le_bytes()));
    // Kullanıcının başlattığı süreç öne çıkma hakkına sahip; bekleyen sürece devreder
    unsafe { AllowSetForegroundWindow(ASFW_ANY) };
    let mut ok = [0u8];
    pipe.write_all(&msg).and_then(|_| pipe.read_exact(&mut ok)).is_ok() && ok[0] == 1
}

/// Sonraki açılışları dinleyen bir süreç var mı (pencere açık ya da gizli)
pub fn is_serving() -> bool {
    connect().is_some()
}

fn connect() -> Option<std::fs::File> {
    let name = pipe_name();
    let path = OsString::from_wide(&name[..name.len() - 1]);
    for _ in 0..3 {
        match std::fs::OpenOptions::new().read(true).write(true).open(&path) {
            Ok(f) => return Some(f),
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => unsafe {
                WaitNamedPipeW(name.as_ptr(), 200);
            },
            Err(_) => return None,
        }
    }
    None
}

fn create_pipe(name: &[u16], first: bool) -> HANDLE {
    let mode = PIPE_ACCESS_DUPLEX | if first { FILE_FLAG_FIRST_PIPE_INSTANCE } else { 0 };
    unsafe { CreateNamedPipeW(name.as_ptr(), mode, PIPE_TYPE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS, PIPE_UNLIMITED_INSTANCES, 512, 512, 0, std::ptr::null()) }
}

/// Sonraki açılışları dinler. Boruyu başka bir süreç tutuyorsa `false` döner;
/// o zaman bu pencere kapanınca süreç de kapanır.
pub fn serve(ctx: egui::Context) -> bool {
    let name = pipe_name();
    let first = create_pipe(&name, true);
    if first == INVALID_HANDLE_VALUE {
        return false;
    }
    // HANDLE ham işaretçi, iş parçacığına sayı olarak geçer
    let first = first as isize;
    std::thread::spawn(move || {
        let mut h = first as HANDLE;
        loop {
            let connected = unsafe { ConnectNamedPipe(h, std::ptr::null_mut()) } != 0 || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
            // Bu istek işlenirken gelen açılış "boru yok" görüp ayrı süreç başlatmasın
            let next = create_pipe(&name, false);
            if connected {
                answer(unsafe { std::fs::File::from_raw_handle(h) }, &ctx);
            } else {
                unsafe { CloseHandle(h) };
            }
            if next == INVALID_HANDLE_VALUE {
                return;
            }
            h = next;
        }
    });
    true
}

fn answer(mut pipe: std::fs::File, ctx: &egui::Context) {
    let mut len = [0u8; 4];
    if pipe.read_exact(&mut len).is_err() {
        return;
    }
    let len = u32::from_le_bytes(len) as usize;
    if len > 64 * 1024 {
        return;
    }
    let mut buf = vec![0u8; len];
    if pipe.read_exact(&mut buf).is_err() {
        return;
    }
    let dir: Vec<u16> = buf.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let hwnd = PARKED.swap(0, SeqCst);
    if hwnd != 0 {
        *REOPEN.lock().unwrap() = Some((!dir.is_empty()).then(|| PathBuf::from(OsString::from_wide(&dir))));
        // İlk kare eski içeriği göstermesin; arayüz yeni içerik hazır olunca açar
        crate::shell::set_cloak(hwnd, true);
        let h = hwnd as windows_sys::Win32::Foundation::HWND;
        unsafe {
            ShowWindow(h, if IsIconic(h) != 0 { SW_RESTORE } else { SW_SHOW });
            SetForegroundWindow(h);
        }
        ctx.request_repaint();
    }
    // Pencere açıksa reddedilir; yeni açılış kendi penceresini açar
    let _ = pipe.write_all(&[(hwnd != 0) as u8]);
    let _ = pipe.sync_all();
}

/// Pencereyi gizler ve sonraki açılışı bekler
pub fn park(hwnd: isize) {
    unsafe { ShowWindow(hwnd as windows_sys::Win32::Foundation::HWND, SW_HIDE) };
    PARKED.store(hwnd, SeqCst);
}

/// Pencere gizli bekliyorsa bekleme durumunu bırakır: sonraki açılış artık bu sürece devredilmez
pub fn take_parked() -> bool {
    PARKED.swap(0, SeqCst) != 0
}

/// Gizliyken gelen açılış isteği
pub fn take_reopen() -> Option<Option<PathBuf>> {
    REOPEN.lock().unwrap().take()
}

// ---------- Oturum açılınca arka planda başlama: bilgisayar açıldıktan sonraki ilk açılış da anında olsun

/// Bu bayrakla başlayan süreç pencere göstermeden bekler
pub const BACKGROUND_ARG: &str = "--background";

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "SwiftExplorer";

pub fn autostart() -> bool {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};
    let (key, value) = (wide(OsStr::new(RUN_KEY)), wide(OsStr::new(RUN_VALUE)));
    unsafe { RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) == 0 }
}

pub fn set_autostart(on: bool) {
    use windows_sys::Win32::System::Registry::{RegDeleteKeyValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ};
    let (key, value) = (wide(OsStr::new(RUN_KEY)), wide(OsStr::new(RUN_VALUE)));
    if !on {
        unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr()) };
        return;
    }
    let Ok(exe) = crate::update::stable_exe() else { return };
    let mut cmd = OsString::from("\"");
    cmd.push(exe.as_os_str());
    cmd.push(format!("\" {BACKGROUND_ARG}"));
    let data = wide(&cmd);
    unsafe { RegSetKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr(), REG_SZ, data.as_ptr().cast(), (data.len() * 2) as u32) };
}

// ---------- Varsayılan dosya gezgini: klasörler, sürücüler ve Win+E bu uygulamada açılır

const VERB: &str = "SwiftExplorer";
/// Klasör ve sürücülerin varsayılan eylemi bu anahtarların (Default) değeridir
const SHELL_KEYS: [&str; 2] = [r"Software\Classes\Directory\shell", r"Software\Classes\Drive\shell"];
/// Win+E'nin açtığı "Bu bilgisayar" penceresi
const WIN_E_KEY: &str = r"Software\Classes\CLSID\{52205fd8-5dfb-447d-801a-d0b52f2e83e1}";

fn reg_get(key: &str, value: Option<&str>) -> Option<String> {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};
    let key = wide(OsStr::new(key));
    let value = value.map(|v| wide(OsStr::new(v)));
    let value_ptr = value.as_ref().map_or(std::ptr::null(), |v| v.as_ptr());
    let mut buf = [0u16; 1024];
    let mut len = (buf.len() * 2) as u32;
    let r = unsafe { RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), value_ptr, RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut len) };
    (r == 0).then(|| String::from_utf16_lossy(&buf[..(len as usize / 2).saturating_sub(1)]))
}

fn reg_set(key: &str, value: Option<&str>, data: &OsStr) -> bool {
    use windows_sys::Win32::System::Registry::{RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ};
    let key = wide(OsStr::new(key));
    let value = value.map(|v| wide(OsStr::new(v)));
    let value_ptr = value.as_ref().map_or(std::ptr::null(), |v| v.as_ptr());
    let data = wide(data);
    unsafe { RegSetKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), value_ptr, REG_SZ, data.as_ptr().cast(), (data.len() * 2) as u32) == 0 }
}

fn reg_delete_value(key: &str) {
    use windows_sys::Win32::System::Registry::{RegDeleteKeyValueW, HKEY_CURRENT_USER};
    let key = wide(OsStr::new(key));
    unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), std::ptr::null()) };
}

fn reg_delete_tree(key: &str) {
    use windows_sys::Win32::System::Registry::{RegDeleteTreeW, HKEY_CURRENT_USER};
    let key = wide(OsStr::new(key));
    unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, key.as_ptr()) };
}

/// `"C:\...\SwiftExplorer.exe" "%1"`; argümansız hali Win+E içindir
fn launch_command(arg: &str) -> Option<OsString> {
    let exe = crate::update::stable_exe().ok()?;
    let mut cmd = OsString::from("\"");
    cmd.push(exe.as_os_str());
    cmd.push("\"");
    cmd.push(arg);
    Some(cmd)
}

/// Kayıtlı komut bu exe'yi gösteriyor mu (exe taşındıysa kayıt eskimiştir)
pub fn is_default_file_manager() -> bool {
    let Some(cmd) = launch_command(" \"%1\"") else { return false };
    SHELL_KEYS.iter().all(|k| reg_get(k, None).as_deref() == Some(VERB) && reg_get(&format!(r"{k}\{VERB}\command"), None).is_some_and(|c| OsStr::new(&c) == cmd))
}

/// `"C:\...\SwiftExplorer.exe" "%1"` komutundaki exe
fn command_exe(cmd: &str) -> Option<PathBuf> {
    Some(PathBuf::from(cmd.strip_prefix('"')?.split('"').next()?))
}

/// Uygulama varsayılan gezginse kaydı bu exe'ye ve geçerli dile göre yeniler: exe taşındıysa
/// ya da yeni sürümde menü adı değiştiyse klasörler yine bu uygulamada açılır. Kayıt hâlâ
/// duran başka bir kopyayı gösteriyorsa o kopya seçilmiştir, dokunmaz.
pub fn refresh_default_file_manager() {
    let Some(cmd) = reg_get(&format!(r"{}\{VERB}\command", SHELL_KEYS[0]), None) else { return };
    // Varsayılanı sonradan başka bir program aldıysa geri alma
    if SHELL_KEYS.iter().all(|k| reg_get(k, None).as_deref() != Some(VERB)) {
        return;
    }
    let (Some(open), Some(bare)) = (launch_command(" \"%1\""), launch_command("")) else { return };
    let label = crate::i18n::t("Swift Explorer ile aç");
    let fresh = is_default_file_manager()
        && SHELL_KEYS.iter().all(|k| reg_get(&format!(r"{k}\{VERB}"), None).as_deref() == Some(label))
        && reg_get(&format!(r"{WIN_E_KEY}\shell\opennewwindow\command"), None).is_some_and(|c| OsStr::new(&c) == bare);
    if fresh || OsStr::new(&cmd) != open && command_exe(&cmd).is_some_and(|e| e.exists()) {
        return;
    }
    set_default_file_manager(true, None);
}

/// Yalnızca bu kullanıcı için (HKCU) ayarlar; geri alınca yazdığı her şeyi siler.
/// `previous` ve dönüş değeri Linux içindir; burada eski değer silinerek geri gelir.
pub fn set_default_file_manager(on: bool, _previous: Option<&str>) -> Option<String> {
    use windows_sys::Win32::UI::Shell::{SHChangeNotify, SHCNE_ASSOCCHANGED, SHCNF_IDLIST};
    if on {
        let (Some(open), Some(bare)) = (launch_command(" \"%1\""), launch_command("")) else { return None };
        for k in SHELL_KEYS {
            reg_set(&format!(r"{k}\{VERB}"), None, OsStr::new(crate::i18n::t("Swift Explorer ile aç")));
            reg_set(&format!(r"{k}\{VERB}\command"), None, &open);
            reg_set(k, None, OsStr::new(VERB));
        }
        // Win+E: DelegateExecute boş olunca Explorer komutu çalıştırır
        reg_set(&format!(r"{WIN_E_KEY}\shell\opennewwindow\command"), None, &bare);
        reg_set(&format!(r"{WIN_E_KEY}\shell\opennewwindow\command"), Some("DelegateExecute"), OsStr::new(""));
    } else {
        for k in SHELL_KEYS {
            if reg_get(k, None).as_deref() == Some(VERB) {
                reg_delete_value(k);
            }
            reg_delete_tree(&format!(r"{k}\{VERB}"));
        }
        // Başka bir program yazdıysa dokunma
        let ours = reg_get(&format!(r"{WIN_E_KEY}\shell\opennewwindow\command"), None).is_some_and(|c| c.to_lowercase().contains("swiftexplorer") || c.to_lowercase().contains("swift-explorer"));
        if ours {
            reg_delete_tree(WIN_E_KEY);
        }
    }
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED as i32, SHCNF_IDLIST, std::ptr::null(), std::ptr::null()) };
    None
}

// ---------- Dosyayı kim kilitliyor: Restart Manager

/// Klasörlerde içindeki dosyalara bakılır; çok büyük klasörde sorgu uzamasın diye sınır var
const LOCK_SCAN_LIMIT: usize = 5000;

fn files_under(paths: &[PathBuf], out: &mut Vec<PathBuf>) {
    for p in paths {
        if out.len() >= LOCK_SCAN_LIMIT {
            return;
        }
        match std::fs::symlink_metadata(p) {
            Ok(md) if md.is_dir() => {
                let children: Vec<PathBuf> = std::fs::read_dir(p).into_iter().flatten().flatten().map(|e| e.path()).collect();
                files_under(&children, out);
            }
            Ok(_) => out.push(p.clone()),
            Err(_) => {}
        }
    }
}

fn filetime_u64(ft: FILETIME) -> u64 {
    (ft.dwHighDateTime as u64) << 32 | ft.dwLowDateTime as u64
}

fn utf16_until_nul(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

/// Aynı PID'yi sonradan başka bir süreç almış olabilir; başlama zamanı tutmazsa tutamaç verilmez
fn open_same(l: &Locker, access: u32) -> Option<HANDLE> {
    unsafe {
        let h = OpenProcess(access | PROCESS_QUERY_LIMITED_INFORMATION, 0, l.pid);
        if h.is_null() {
            return None;
        }
        let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut created, mut exit, mut kernel, mut user) = (zero, zero, zero, zero);
        if GetProcessTimes(h, &mut created, &mut exit, &mut kernel, &mut user) != 0 && filetime_u64(created) == l.started {
            Some(h)
        } else {
            CloseHandle(h);
            None
        }
    }
}

fn exe_path(pid: u32) -> Option<PathBuf> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len) != 0;
        CloseHandle(h);
        ok.then(|| PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
    }
}

/// Sürecin görünür, sahipsiz pencereleri: kapatılacak ana pencereler
fn top_windows(pid: u32) -> Vec<HWND> {
    struct Find {
        pid: u32,
        found: Vec<HWND>,
    }
    unsafe extern "system" fn each(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let f = &mut *(lparam as *mut Find);
        let mut owner_pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut owner_pid);
        if owner_pid == f.pid && IsWindowVisible(hwnd) != 0 && GetWindow(hwnd, GW_OWNER).is_null() {
            f.found.push(hwnd);
        }
        1
    }
    let mut f = Find { pid, found: Vec::new() };
    unsafe { EnumWindows(Some(each), &mut f as *mut Find as LPARAM) };
    f.found
}

/// Bu dosyaları açık tutan süreçler. Yavaş olabilir, arayüz iş parçacığında çağırma.
pub fn lockers(paths: &[PathBuf]) -> Vec<Locker> {
    let mut files = Vec::new();
    files_under(paths, &mut files);
    if files.is_empty() {
        return Vec::new();
    }
    let wides: Vec<Vec<u16>> = files.iter().map(|p| wide(p.as_os_str())).collect();
    let names: Vec<*const u16> = wides.iter().map(|w| w.as_ptr()).collect();
    let mut found = Vec::new();
    unsafe {
        let mut session = 0u32;
        let mut key = [0u16; CCH_RM_SESSION_KEY as usize + 1];
        if RmStartSession(&mut session, 0, key.as_mut_ptr()) != 0 {
            return found;
        }
        if RmRegisterResources(session, names.len() as u32, names.as_ptr(), 0, std::ptr::null(), 0, std::ptr::null()) == 0 {
            let mut info: Vec<RM_PROCESS_INFO> = Vec::new();
            // Sorgu arasında yeni süreç dosyayı açarsa liste büyür; birkaç kez denenir
            for _ in 0..3 {
                let (mut needed, mut count, mut reasons) = (0u32, info.capacity() as u32, 0u32);
                let r = RmGetList(session, &mut needed, &mut count, info.as_mut_ptr(), &mut reasons);
                if r == ERROR_MORE_DATA {
                    info.reserve(needed as usize);
                    continue;
                }
                if r == 0 {
                    info.set_len(count as usize);
                }
                break;
            }
            let me = std::process::id();
            for i in &info {
                let pid = i.Process.dwProcessId;
                found.push(Locker {
                    pid,
                    started: filetime_u64(i.Process.ProcessStartTime),
                    name: utf16_until_nul(&i.strAppName),
                    exe: exe_path(pid),
                    service: i.ApplicationType == RmService,
                    has_window: !top_windows(pid).is_empty(),
                    is_self: pid == me,
                });
            }
        }
        RmEndSession(session);
    }
    found
}

/// Pencerelerine kapat mesajı gönderir; program kaydedilmemiş iş için soru sorabilir
pub fn close_locker(l: &Locker) -> bool {
    let Some(h) = open_same(l, 0) else { return false };
    unsafe { CloseHandle(h) };
    let windows = top_windows(l.pid);
    for &w in &windows {
        unsafe { PostMessageW(w, WM_CLOSE, 0, 0) };
    }
    !windows.is_empty()
}

/// Süreci hemen sonlandırır; kaydedilmemiş değişiklikler kaybolur
pub fn kill_locker(l: &Locker) -> bool {
    let Some(h) = open_same(l, PROCESS_TERMINATE) else { return false };
    unsafe {
        let ok = TerminateProcess(h, 1) != 0;
        CloseHandle(h);
        ok
    }
}

#[cfg(test)]
mod tests {
    use super::command_exe;
    use std::path::PathBuf;

    #[test]
    fn exe_of_a_registered_command() {
        assert_eq!(command_exe(r#""C:\Apps\Swift Explorer\SwiftExplorer.exe" "%1""#), Some(PathBuf::from(r"C:\Apps\Swift Explorer\SwiftExplorer.exe")));
        assert_eq!(command_exe(r#""C:\SwiftExplorer.exe""#), Some(PathBuf::from(r"C:\SwiftExplorer.exe")));
        assert_eq!(command_exe(r"C:\SwiftExplorer.exe %1"), None);
    }
}
