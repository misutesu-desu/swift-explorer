//! Windows Shell entegrasyonu: sistem ikonları/küçük resimler, tür adları,
//! yerel sağ tık menüsü, uygulamadan dışarı sürükle-bırak ve vurgu rengi.

use std::cell::RefCell;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicIsize, Ordering};

use windows::core::{implement, Interface, HRESULT, HSTRING, PCSTR, PCWSTR, PSTR};
use windows::Win32::Foundation::{
    BOOL, DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, HWND, LPARAM, LRESULT, POINT, RECT,
    SIZE, S_OK, WPARAM,
};
use windows::Win32::Graphics::Dwm::{DwmGetColorizationColor, DwmSetWindowAttribute, DWMWA_CLOAK};
use windows::Win32::Graphics::Gdi::{
    DeleteObject, GetDC, GetDIBits, GetObjectW, ReleaseDC, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DIB_RGB_COLORS, HBITMAP,
};
use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL;
use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, IDataObject, COINIT_APARTMENTTHREADED};
use windows::Win32::System::Ole::{DoDragDrop, IDropSource, IDropSource_Impl, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE, DROPEFFECT_NONE};
use windows::Win32::System::SystemServices::{MK_LBUTTON, MODIFIERKEYS_FLAGS};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    BHID_DataObject, BHID_SFUIObject, DefSubclassProc, IContextMenu, IContextMenu2, IContextMenu3, IShellItemArray,
    IShellItemImageFactory, SHCreateItemFromParsingName, SHCreateShellItemArrayFromIDLists, SHGetFileInfoW,
    SHParseDisplayName, RemoveWindowSubclass, SetWindowSubclass, CMF_CANRENAME, CMF_EXTENDEDVERBS, CMF_NORMAL,
    CMINVOKECOMMANDINFO, CMINVOKECOMMANDINFOEX, GCS_VERBW, SHFILEINFOW, SHGFI_TYPENAME,
    SHGFI_USEFILEATTRIBUTES, SIIGBF, SIIGBF_BIGGERSIZEOK, SIIGBF_ICONONLY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreatePopupMenu, DestroyMenu, GetCursorPos, GetWindowRect, IsZoomed, TrackPopupMenuEx, HTBOTTOM, HTBOTTOMLEFT,
    HTBOTTOMRIGHT, HTCLIENT, HTLEFT, HTRIGHT, HTTOP, HTTOPLEFT, HTTOPRIGHT, SW_SHOWNORMAL, TPM_RETURNCMD,
    TPM_RIGHTBUTTON, WM_NCHITTEST, WM_DRAWITEM, WM_INITMENUPOPUP, WM_MEASUREITEM, WM_MENUCHAR, WM_MENUSELECT,
    CallNextHookEx, SetWindowsHookExW, UnhookWindowsHookEx, CBT_CREATEWNDW, HCBT_CREATEWND, HHOOK, WH_CBT, WS_CHILD,
    WM_CREATE, WM_SHOWWINDOW,
};
use windows::Win32::System::Threading::GetCurrentThreadId;

pub use crate::images::{ImageCache, Img};

fn hs(p: &Path) -> HSTRING {
    HSTRING::from(p.as_os_str())
}

// ---------------------------------------------------------------- görüntüler

/// Shell'den ikon ya da küçük resim alır (Explorer'ın kendi önbelleğini kullanır).
pub fn shell_image(path: &Path, size: i32, icon_only: bool) -> Option<egui::ColorImage> {
    unsafe {
        let factory: IShellItemImageFactory = SHCreateItemFromParsingName(&hs(path), None).ok()?;
        let mut flags: SIIGBF = SIIGBF_BIGGERSIZEOK;
        if icon_only {
            flags |= SIIGBF_ICONONLY;
        }
        let hbm = factory.GetImage(SIZE { cx: size, cy: size }, flags).ok()?;
        let img = hbitmap_to_image(hbm);
        let _ = DeleteObject(hbm);
        img
    }
}

unsafe fn hbitmap_to_image(hbm: HBITMAP) -> Option<egui::ColorImage> {
    let (buf, w, h) = hbitmap_bgra(hbm)?;
    Some(bgra_to_image(buf, w, h))
}

/// HBITMAP'in ham 32 bit BGRA pikselleri (yukarıdan aşağı)
unsafe fn hbitmap_bgra(hbm: HBITMAP) -> Option<(Vec<u8>, usize, usize)> {
    let mut bm = BITMAP::default();
    if GetObjectW(hbm, std::mem::size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut c_void)) == 0 {
        return None;
    }
    let (w, h) = (bm.bmWidth, bm.bmHeight.abs());
    if w <= 0 || h <= 0 {
        return None;
    }
    let mut bi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h, // yukarıdan aşağı
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut buf = vec![0u8; (w * h * 4) as usize];
    let dc = GetDC(HWND::default());
    let lines = GetDIBits(dc, hbm, 0, h as u32, Some(buf.as_mut_ptr() as *mut c_void), &mut bi, DIB_RGB_COLORS);
    ReleaseDC(HWND::default(), dc);
    if lines == 0 {
        return None;
    }
    Some((buf, w as usize, h as usize))
}

fn bgra_to_image(mut buf: Vec<u8>, w: usize, h: usize) -> egui::ColorImage {
    // JPEG küçük resimlerinde alfa kanalı boş gelir
    let no_alpha = buf.chunks_exact(4).all(|p| p[3] == 0);
    // Shell ikonları çoğu zaman ön-çarpılmamış alfa ile gelir; öyleyse ön-çarpılmış
    // gibi çizmek yarı saydam kenarlarda beyaz hale bırakır. Rengi alfadan büyük
    // bir piksel varsa veri ön-çarpılmamıştır.
    let straight = !no_alpha && buf.chunks_exact(4).any(|p| p[0] > p[3] || p[1] > p[3] || p[2] > p[3]);
    for p in buf.chunks_exact_mut(4) {
        p.swap(0, 2); // BGRA → RGBA
        if no_alpha {
            p[3] = 255;
        } else if straight {
            // Windows gibi gamma uzayında çarp. egui 0.31'in from_rgba_unmultiplied'ı
            // lineer uzayda çarpar; yarı saydam kenarlar yine fazla parlak çıkar.
            let a = p[3] as u32;
            for c in &mut p[..3] {
                *c = ((*c as u32 * a + 127) / 255) as u8;
            }
        }
    }
    egui::ColorImage::from_rgba_premultiplied([w, h], &buf)
}

pub fn type_name(ext: &str) -> String {
    unsafe {
        let mut info = SHFILEINFOW::default();
        let name = HSTRING::from(format!("x.{ext}"));
        SHGetFileInfoW(
            &name,
            FILE_ATTRIBUTE_NORMAL,
            Some(&mut info),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_TYPENAME | SHGFI_USEFILEATTRIBUTES,
        );
        // 32 bit Windows'ta yapı "packed"; alana referans almak yerine değeri kopyalarız
        let type_name = info.szTypeName;
        let len = type_name.iter().position(|&c| c == 0).unwrap_or(0);
        String::from_utf16_lossy(&type_name[..len])
    }
}

/// Görüntü iş parçacığı başlarken bir kez çağrılır (Shell, COM ister).
pub fn init_worker_thread() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
}

// ---------------------------------------------------------------- PIDL yardımcıları

struct Pidls(Vec<*mut ITEMIDLIST>);

impl Drop for Pidls {
    fn drop(&mut self) {
        for p in &self.0 {
            unsafe { CoTaskMemFree(Some(*p as *const c_void)) };
        }
    }
}

fn item_array(paths: &[PathBuf]) -> Option<(IShellItemArray, Pidls)> {
    unsafe {
        let mut pidls = Pidls(Vec::new());
        for p in paths {
            let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
            if SHParseDisplayName(&hs(p), None, &mut pidl, 0, None).is_ok() && !pidl.is_null() {
                pidls.0.push(pidl);
            }
        }
        if pidls.0.is_empty() {
            return None;
        }
        let consts: Vec<*const ITEMIDLIST> = pidls.0.iter().map(|p| *p as *const _).collect();
        let arr = SHCreateShellItemArrayFromIDLists(&consts).ok()?;
        Some((arr, pidls))
    }
}

// ---------------------------------------------------------------- yerel sağ tık menüsü

thread_local! {
    static ACTIVE_MENU: RefCell<Option<IContextMenu>> = const { RefCell::new(None) };
}

static STARTUP_HOOK: AtomicIsize = AtomicIsize::new(0);
static STARTUP_HWND: AtomicIsize = AtomicIsize::new(0);
const STARTUP_SUBCLASS: usize = 0x5345;

/// Ana pencereyi daha gösterilmeden gizler. winit pencereyi GL yüzeyi hazır olmadan
/// (~250 ms önce) görünür yapabiliyor; o sürede boş/beyaz yüzey ekrana yansır.
/// Oluşturulan pencere alt sınıflanır ve gösterilmeden hemen önce DWM ile gizlenir.
pub fn begin_startup_cloak() {
    unsafe {
        if let Ok(h) = SetWindowsHookExW(WH_CBT, Some(cbt_proc), None, GetCurrentThreadId()) {
            STARTUP_HOOK.store(h.0 as isize, Ordering::Relaxed);
        }
    }
}

/// Açılış gizlemesini kaldırır; pencere bundan sonra normal davranır.
pub fn end_startup_cloak() {
    unhook_startup();
    let hwnd = STARTUP_HWND.swap(0, Ordering::Relaxed);
    if hwnd != 0 {
        unsafe {
            let _ = RemoveWindowSubclass(HWND(hwnd as *mut c_void), Some(startup_subclass), STARTUP_SUBCLASS);
        }
    }
}

fn unhook_startup() {
    let h = STARTUP_HOOK.swap(0, Ordering::Relaxed);
    if h != 0 {
        unsafe {
            let _ = UnhookWindowsHookEx(HHOOK(h as *mut c_void));
        }
    }
}

unsafe extern "system" fn cbt_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code == HCBT_CREATEWND as i32 && STARTUP_HWND.load(Ordering::Relaxed) == 0 {
        let cs = &*(*(lp.0 as *const CBT_CREATEWNDW)).lpcs;
        let top_level = cs.hwndParent.is_invalid() && (cs.style & WS_CHILD.0 as i32) == 0;
        if top_level && !cs.lpszName.is_null() && cs.lpszName.to_string().is_ok_and(|t| t == crate::APP_NAME) {
            let hwnd = HWND(wp.0 as *mut c_void);
            if SetWindowSubclass(hwnd, Some(startup_subclass), STARTUP_SUBCLASS, 0).as_bool() {
                STARTUP_HWND.store(wp.0 as isize, Ordering::Relaxed);
            }
        }
    }
    let r = CallNextHookEx(None, code, wp, lp);
    if STARTUP_HWND.load(Ordering::Relaxed) != 0 {
        unhook_startup(); // ana pencere bulundu, kancaya artık gerek yok
    }
    r
}

unsafe extern "system" fn startup_subclass(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, _data: usize) -> LRESULT {
    match msg {
        WM_CREATE => {
            let r = DefSubclassProc(hwnd, msg, wp, lp);
            set_cloak(hwnd.0 as isize, true);
            r
        }
        WM_SHOWWINDOW if wp.0 != 0 => {
            set_cloak(hwnd.0 as isize, true);
            DefSubclassProc(hwnd, msg, wp, lp)
        }
        _ => DefSubclassProc(hwnd, msg, wp, lp),
    }
}

/// Sistem ölçeği (ör. %150 → 1.5). Süreci, winit'in de yaptığı gibi monitör başına
/// DPI farkında yapar; aksi halde Windows her zaman 96 DPI bildirir.
pub fn system_scale() -> f32 {
    use windows::Win32::UI::HiDpi::{GetDpiForSystem, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        GetDpiForSystem().max(96) as f32 / 96.0
    }
}

/// Pencereyi DWM düzeyinde gizler/gösterir. Gizliyken çizim sürer ama ekrana
/// hiçbir şey (ilk karedeki beyaz yüzey dahil) yansımaz.
pub fn set_cloak(hwnd: isize, cloak: bool) {
    unsafe {
        let v = BOOL::from(cloak);
        let _ = DwmSetWindowAttribute(HWND(hwnd as *mut c_void), DWMWA_CLOAK, &v as *const BOOL as *const c_void, std::mem::size_of::<BOOL>() as u32);
    }
}

/// Ana pencereyi alt sınıflar: "Gönder", "Birlikte aç" gibi alt menüler
/// IContextMenu2/3'e iletilen mesajlarla doldurulur.
pub fn install_subclass(hwnd: isize) {
    unsafe {
        let _ = SetWindowSubclass(HWND(hwnd as *mut c_void), Some(subclass_proc), 0x4847, 0);
    }
}

unsafe extern "system" fn subclass_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, _data: usize) -> LRESULT {
    if msg == WM_NCHITTEST {
        let hit = DefSubclassProc(hwnd, msg, wp, lp);
        if hit.0 == HTCLIENT as isize {
            if let Some(edge) = resize_edge(hwnd, lp) {
                return LRESULT(edge as isize);
            }
        }
        return hit;
    }
    if matches!(msg, WM_INITMENUPOPUP | WM_DRAWITEM | WM_MEASUREITEM | WM_MENUCHAR | WM_MENUSELECT) {
        let handled = ACTIVE_MENU.with(|m| {
            let m = m.borrow();
            let cm = m.as_ref()?;
            if let Ok(cm3) = cm.cast::<IContextMenu3>() {
                let mut res = LRESULT(0);
                if cm3.HandleMenuMsg2(msg, wp, lp, Some(&mut res)).is_ok() {
                    return Some(res);
                }
            } else if let Ok(cm2) = cm.cast::<IContextMenu2>() {
                if cm2.HandleMenuMsg(msg, wp, lp).is_ok() {
                    return Some(LRESULT(0));
                }
            }
            None
        });
        if let Some(r) = handled {
            return r;
        }
    }
    DefSubclassProc(hwnd, msg, wp, lp)
}

/// Kenarlıksız pencerede kenarlardan boyutlandırma: imleç kenara yakınsa ilgili HT kodu.
unsafe fn resize_edge(hwnd: HWND, lp: LPARAM) -> Option<u32> {
    if IsZoomed(hwnd).as_bool() {
        return None;
    }
    let mut r = RECT::default();
    GetWindowRect(hwnd, &mut r).ok()?;
    let (x, y) = ((lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32);
    let dpi = windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd).max(96) as i32;
    let b = 6 * dpi / 96;
    let (l, rt, t, bt) = (x < r.left + b, x >= r.right - b, y < r.top + b, y >= r.bottom - b);
    Some(match (l, rt, t, bt) {
        (true, _, true, _) => HTTOPLEFT,
        (_, true, true, _) => HTTOPRIGHT,
        (true, _, _, true) => HTBOTTOMLEFT,
        (_, true, _, true) => HTBOTTOMRIGHT,
        (true, ..) => HTLEFT,
        (_, true, ..) => HTRIGHT,
        (_, _, true, _) => HTTOP,
        (.., true) => HTBOTTOM,
        _ => return None,
    })
}

pub enum MenuResult {
    None,
    Rename,
    Invoked,
}

/// Explorer'ın gerçek sağ tık menüsünü imleç konumunda gösterir.
pub fn context_menu(hwnd: isize, paths: &[PathBuf], extended: bool) -> MenuResult {
    unsafe {
        let Some((arr, _pidls)) = item_array(paths) else { return MenuResult::None };
        let Ok(cm) = arr.BindToHandler::<_, IContextMenu>(None, &BHID_SFUIObject) else { return MenuResult::None };
        let Ok(menu) = CreatePopupMenu() else { return MenuResult::None };
        let mut flags = CMF_NORMAL | CMF_CANRENAME;
        if extended {
            flags |= CMF_EXTENDEDVERBS;
        }
        if cm.QueryContextMenu(menu, 0, 1, 0x7FFF, flags).is_err() {
            let _ = DestroyMenu(menu);
            return MenuResult::None;
        }
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let hwnd = HWND(hwnd as *mut c_void);
        ACTIVE_MENU.with(|m| *m.borrow_mut() = Some(cm.clone()));
        let cmd = TrackPopupMenuEx(menu, (TPM_RETURNCMD | TPM_RIGHTBUTTON).0, pt.x, pt.y, hwnd, None);
        ACTIVE_MENU.with(|m| *m.borrow_mut() = None);
        let _ = DestroyMenu(menu);
        if cmd.0 <= 0 {
            return MenuResult::None;
        }
        let id = (cmd.0 - 1) as usize;
        let mut verb = [0u16; 64];
        if cm.GetCommandString(id, GCS_VERBW, None, PSTR(verb.as_mut_ptr() as *mut u8), verb.len() as u32).is_ok() {
            let len = verb.iter().position(|&c| c == 0).unwrap_or(0);
            if String::from_utf16_lossy(&verb[..len]).eq_ignore_ascii_case("rename") {
                return MenuResult::Rename;
            }
        }
        let info = CMINVOKECOMMANDINFOEX {
            cbSize: std::mem::size_of::<CMINVOKECOMMANDINFOEX>() as u32,
            fMask: 0x4000 | 0x2000_0000, // CMIC_MASK_UNICODE | CMIC_MASK_PTINVOKE
            hwnd,
            lpVerb: PCSTR(id as *const u8),
            lpVerbW: PCWSTR(id as *const u16),
            nShow: SW_SHOWNORMAL.0,
            ptInvoke: pt,
            ..Default::default()
        };
        let _ = cm.InvokeCommand(&info as *const _ as *const CMINVOKECOMMANDINFO);
        MenuResult::Invoked
    }
}

// ---------------------------------------------------------------- dışarı sürükle-bırak

#[implement(IDropSource)]
struct DropSource;

impl IDropSource_Impl for DropSource_Impl {
    fn QueryContinueDrag(&self, escape: BOOL, keys: MODIFIERKEYS_FLAGS) -> HRESULT {
        if escape.as_bool() {
            DRAGDROP_S_CANCEL
        } else if keys.0 & MK_LBUTTON.0 == 0 {
            DRAGDROP_S_DROP
        } else {
            S_OK
        }
    }

    fn GiveFeedback(&self, _effect: DROPEFFECT) -> HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

/// Seçili öğeleri OLE sürükle-bırak ile masaüstüne/Explorer'a/başka uygulamalara taşır.
/// Bırakılana kadar engeller (Windows'un kendi sürükleme döngüsü).
pub fn drag_out(paths: &[PathBuf]) -> bool {
    unsafe {
        let Some((arr, _pidls)) = item_array(paths) else { return false };
        let Ok(data) = arr.BindToHandler::<_, IDataObject>(None, &BHID_DataObject) else { return false };
        let source: IDropSource = DropSource.into();
        let mut effect = DROPEFFECT_NONE;
        let hr = DoDragDrop(&data, &source, DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK, &mut effect);
        hr == DRAGDROP_S_DROP
    }
}

pub fn cursor_outside_window(hwnd: isize) -> bool {
    unsafe {
        let mut pt = POINT::default();
        let mut r = RECT::default();
        if GetCursorPos(&mut pt).is_err() || GetWindowRect(HWND(hwnd as *mut c_void), &mut r).is_err() {
            return false;
        }
        pt.x < r.left || pt.x >= r.right || pt.y < r.top || pt.y >= r.bottom
    }
}

// ---------------------------------------------------------------- tema

pub fn accent_color() -> Option<egui::Color32> {
    unsafe {
        let mut c = 0u32;
        let mut opaque = BOOL(0);
        DwmGetColorizationColor(&mut c, &mut opaque).ok()?;
        Some(egui::Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ön-çarpılmış pikselde hiçbir renk kanalı alfadan büyük olamaz; olursa
    /// çizimde yarı saydam kenarlar beyaza çalar.
    fn bad_pixels(img: &egui::ColorImage) -> usize {
        img.pixels.iter().filter(|c| c.r() > c.a() || c.g() > c.a() || c.b() > c.a()).count()
    }

    #[test]
    fn shell_icons_have_valid_premultiplied_alpha() {
        init_worker_thread();
        let win = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
        let files = [
            format!(r"{win}\explorer.exe"),
            format!(r"{win}\notepad.exe"),
            format!(r"{win}\regedit.exe"),
            format!(r"{win}\System32"),
            format!(r"{win}\win.ini"),
            format!(r"{win}\System32\cmd.exe"),
        ];
        let mut straight_inputs = 0;
        for f in &files {
            for size in [16, 32, 48, 96, 256] {
                let raw = unsafe {
                    let factory: IShellItemImageFactory = SHCreateItemFromParsingName(&hs(Path::new(f)), None).unwrap();
                    let hbm = factory.GetImage(SIZE { cx: size, cy: size }, SIIGBF_BIGGERSIZEOK | SIIGBF_ICONONLY).unwrap();
                    let r = hbitmap_bgra(hbm).unwrap();
                    let _ = DeleteObject(hbm);
                    r
                };
                let raw_bad = raw.0.chunks_exact(4).filter(|p| p[0] > p[3] || p[1] > p[3] || p[2] > p[3]).count();
                if raw_bad > 0 {
                    straight_inputs += 1;
                }
                let img = bgra_to_image(raw.0, raw.1, raw.2);
                println!("{f} @{size}: ham geçersiz piksel {raw_bad} → dönüşümden sonra {}", bad_pixels(&img));
                assert_eq!(bad_pixels(&img), 0, "{f} @{size}");
            }
        }
        println!("ön-çarpılmamış gelen ikon sayısı: {straight_inputs}");
    }
}
