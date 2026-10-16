//! Uygulamanın kendi Özellikler penceresi (Windows'taki Genel / Dijital İmzalar / Güvenlik /
//! Ayrıntılar / Önceki Sürümler sekmeli pencerenin karşılığı). Tüm sistemlerde aynı çalışır;
//! renkleri uygulamanın temasından alır. Bir sistemde karşılığı olmayan sekmeler bilgi şeridi gösterir.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use chrono::Datelike;
use egui::{Align, Key, Layout, RichText};

use crate::fsutil::{self, TreeStats};

/// Dosyayı açan varsayılan uygulama
pub struct AppInfo {
    pub name: String,
    /// Ikonu çizmek için programın yolu (bilinmiyorsa ikon çizilmez)
    pub exe: Option<PathBuf>,
}

/// Dijital imza satırı (Windows)
#[cfg_attr(not(windows), allow(dead_code))]
pub struct Signer {
    pub name: String,
    pub digest: String,
    pub message: String,
}

/// Arka planda okunan değer
enum Loaded<T> {
    Pending,
    Done(T),
}
type Shared<T> = Arc<Mutex<Loaded<T>>>;

fn spawn_load<T: Send + 'static>(ctx: &egui::Context, f: impl FnOnce() -> T + Send + 'static) -> Shared<T> {
    let cell: Shared<T> = Arc::new(Mutex::new(Loaded::Pending));
    let (c, ctx) = (cell.clone(), ctx.clone());
    std::thread::spawn(move || {
        let v = f();
        *c.lock().unwrap() = Loaded::Done(v);
        ctx.request_repaint();
    });
    cell
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    General,
    DigitalSignatures,
    Security,
    Details,
    PreviousVersions,
}

/// Diskten okunan, düzenlenmeyen değerler
struct Info {
    name: String,
    kind: String,
    is_dir: bool,
    location: String,
    link: Option<String>,
    created: Option<SystemTime>,
    modified: Option<SystemTime>,
    accessed: Option<SystemTime>,
    /// Tek bir dosyanın (boyut, diskteki boyut); klasör ve çoklu seçimde arka plandaki sayım kullanılır
    file_size: Option<(u64, u64)>,
    readonly: bool,
    hidden: bool,
    /// Unix izin bitleri (Windows'ta 0)
    mode: u32,
    owner: String,
    group: String,
    attrs: String,
    mime: String,
}

/// Pencerenin bir çizimden dönen sonucu
#[derive(Default)]
pub struct Outcome {
    pub close: bool,
    /// Yeniden adlandırma yapıldıysa (eski, yeni): çağıran geri alma kaydı tutar
    pub renamed: Option<(PathBuf, PathBuf)>,
    /// Dosya listesi yeniden okunmalı (öznitelikler değişti)
    pub refresh: bool,
    pub error: Option<String>,
}

/// Simge çizici: (ui, yol, klasör mü, uzantı, boyut)
pub type IconFn<'a> = dyn FnMut(&mut egui::Ui, &Path, bool, &str, f32) + 'a;

pub struct Properties {
    paths: Vec<PathBuf>,
    type_name: String,
    info: Info,
    stats: Option<Arc<TreeStats>>,
    opens_with: Option<Shared<Option<AppInfo>>>,
    #[cfg_attr(not(windows), allow(dead_code))]
    signatures: Option<Shared<Vec<Signer>>>,
    tab: Tab,
    /// Pencerenin ilk konumu (ana pencerenin ortası); sonradan değişmez ki kullanıcı taşıyabilsin
    pos: Option<egui::Pos2>,
    /// Varsayılan uygulama seçici (Linux)
    #[cfg(not(windows))]
    chooser: Option<Chooser>,
    // Düzenlenebilir alanlar; `info` ile farklıysa "Uygula" etkinleşir
    name: String,
    readonly: bool,
    hidden: bool,
    mode: u32,
}

impl Drop for Properties {
    fn drop(&mut self) {
        if let Some(s) = &self.stats {
            s.cancel.store(true, Relaxed);
        }
    }
}

fn start_scan(paths: &[PathBuf], ctx: &egui::Context) -> Arc<TreeStats> {
    #[cfg(windows)]
    let cluster = paths.first().and_then(|p| crate::win::cluster_size(p)).unwrap_or(0);
    #[cfg(not(windows))]
    let cluster = 0;
    let st = Arc::new(TreeStats::with_cluster(cluster));
    let (t, c) = (st.clone(), ctx.clone());
    let roots = paths.to_vec();
    let multi = roots.len() > 1;
    std::thread::spawn(move || {
        for p in &roots {
            // Çoklu seçimde seçilen klasörün kendisi de sayılır
            if multi && std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir()) {
                t.dirs.fetch_add(1, Relaxed);
            }
            fsutil::scan_tree(p, &t);
        }
        t.done.store(true, Relaxed);
        c.request_repaint();
    });
    st
}

fn load_info(paths: &[PathBuf], type_name: &str) -> Option<Info> {
    let first = paths.first()?;
    let single = paths.len() == 1;
    let lmd = std::fs::symlink_metadata(first).ok()?;
    let md = std::fs::metadata(first).unwrap_or_else(|_| lmd.clone());
    let is_dir = md.is_dir();
    let name = first.file_name().map_or_else(|| first.display().to_string(), |n| n.to_string_lossy().into_owned());
    let location = if single || paths.iter().all(|p| p.parent() == first.parent()) {
        first.parent().map(|p| p.display().to_string()).unwrap_or_default()
    } else {
        String::new()
    };
    let kind = if !single {
        crate::i18n::t("Birden çok tür").to_string()
    } else if is_dir || first.extension().is_none() {
        type_name.to_string()
    } else {
        format!("{} (.{})", type_name, first.extension().unwrap_or_default().to_string_lossy())
    };
    #[cfg(not(windows))]
    let (readonly, hidden, mode, owner, group, attrs) = {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let mode = md.permissions().mode();
        let kind_char = if lmd.file_type().is_symlink() { 'l' } else if is_dir { 'd' } else { '-' };
        (
            mode & 0o222 == 0,
            name.starts_with('.'),
            mode & 0o7777,
            fsutil::user_name(md.uid()).unwrap_or_else(|| md.uid().to_string()),
            fsutil::group_name(md.gid()).unwrap_or_else(|| md.gid().to_string()),
            format!("{kind_char}{}", fsutil::mode_string(mode)),
        )
    };
    #[cfg(windows)]
    let (readonly, hidden, mode, owner, group, attrs) = {
        use std::os::windows::fs::MetadataExt;
        let a = md.file_attributes();
        let letters: String = [(0x1, 'R'), (0x2, 'H'), (0x4, 'S'), (0x20, 'A')].iter().filter(|(b, _)| a & b != 0).map(|(_, c)| *c).collect();
        (a & 0x1 != 0, a & 0x2 != 0, 0, String::new(), String::new(), letters)
    };
    #[cfg(not(windows))]
    let mime = if is_dir { "inode/directory".to_string() } else { mime_guess::from_path(first).first_raw().unwrap_or("application/octet-stream").to_string() };
    #[cfg(windows)]
    let mime = String::new();

    let file_size = (single && !is_dir).then(|| {
        #[cfg(not(windows))]
        let on_disk = std::os::unix::fs::MetadataExt::blocks(&md) * 512;
        #[cfg(windows)]
        let on_disk = fsutil::round_to_cluster(md.len(), crate::win::cluster_size(first).unwrap_or(0));
        (md.len(), on_disk)
    });
    Some(Info {
        name,
        kind,
        is_dir,
        location,
        link: lmd.file_type().is_symlink().then(|| std::fs::read_link(first).ok().map(|t| t.display().to_string())).flatten(),
        created: md.created().ok(),
        modified: md.modified().ok(),
        accessed: md.accessed().ok(),
        file_size,
        readonly,
        hidden,
        mode,
        owner,
        group,
        attrs,
        mime,
    })
}

// ---------------------------------------------------------------- biçimlendirme (Windows gibi)

const WEEKDAYS: [&str; 7] = ["Pazartesi", "Salı", "Çarşamba", "Perşembe", "Cuma", "Cumartesi", "Pazar"];
const MONTHS: [&str; 12] = ["Ocak", "Şubat", "Mart", "Nisan", "Mayıs", "Haziran", "Temmuz", "Ağustos", "Eylül", "Ekim", "Kasım", "Aralık"];

/// "Monday, October 12, 2026, 9:20:57 AM" (Windows'un uzun tarih biçimi; dile göre)
fn long_date(t: Option<SystemTime>) -> String {
    let Some(t) = t else { return crate::i18n::t("Kullanılamıyor").to_string() };
    let dt = chrono::DateTime::<chrono::Local>::from(t);
    let time = dt.format(crate::i18n::t("%H:%M:%S")).to_string();
    crate::i18n::tf(
        "{2} {1} {3} {0}, {4}",
        &[&crate::i18n::t(WEEKDAYS[dt.weekday().num_days_from_monday() as usize]), &crate::i18n::t(MONTHS[dt.month0() as usize]), &dt.day(), &dt.year(), &time],
    )
}

/// Binlik ayırıcılı sayı: 12591 → "12,591" (ayırıcı dile göre)
fn group_digits(n: u64) -> String {
    group_with(n, crate::i18n::t("."))
}

fn group_with(n: u64, sep: &str) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push_str(sep);
        }
        out.push(c);
    }
    out
}

/// "12.2 KB": Explorer gibi üç anlamlı basamak, ondalık ayırıcı dile göre
fn size_short(n: u64) -> String {
    const U: [&str; 5] = ["bayt", "KB", "MB", "GB", "TB"];
    if n < 1024 {
        return crate::i18n::tf("{} bayt", &[&group_digits(n)]);
    }
    let (mut v, mut i) = (n as f64, 0);
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    let s = if v < 10.0 { format!("{v:.2}") } else if v < 100.0 { format!("{v:.1}") } else { format!("{v:.0}") };
    format!("{} {}", s.replace('.', crate::i18n::t(",")), U[i])
}

/// "12.2 KB (12,591 bytes)"
fn size_full(n: u64) -> String {
    if n < 1024 {
        size_short(n)
    } else {
        format!("{} ({})", size_short(n), crate::i18n::tf("{} bayt", &[&group_digits(n)]))
    }
}

/// Dijital imzası olabilecek dosya türleri (Windows bu sekmeyi yalnızca imzalı dosyalarda gösterir)
#[cfg_attr(not(windows), allow(dead_code))]
fn signable(path: &Path) -> bool {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    matches!(ext.as_str(), "exe" | "dll" | "sys" | "ocx" | "scr" | "msi" | "msp" | "cab" | "cat" | "ps1" | "psm1" | "psd1" | "vbs" | "js" | "wsf" | "msix" | "appx" | "msixbundle" | "appxbundle")
}

impl Properties {
    /// `type_name`: tek öğenin tür adı (çağıran önbellekten verir)
    pub fn new(paths: Vec<PathBuf>, type_name: String, ctx: &egui::Context) -> Option<Properties> {
        let info = load_info(&paths, &type_name)?;
        let single_file = paths.len() == 1 && !info.is_dir;
        let stats = (paths.len() > 1 || info.is_dir).then(|| start_scan(&paths, ctx));
        let opens_with = single_file.then(|| {
            let p = paths[0].clone();
            spawn_load(ctx, move || crate::win::default_app(&p))
        });
        #[cfg(windows)]
        let signatures = (single_file && signable(&paths[0])).then(|| {
            let p = paths[0].clone();
            spawn_load(ctx, move || crate::win::signatures(&p))
        });
        #[cfg(not(windows))]
        let signatures = None;
        Some(Properties {
            name: info.name.clone(),
            readonly: info.readonly,
            hidden: info.hidden,
            mode: info.mode,
            paths,
            type_name,
            info,
            stats,
            opens_with,
            signatures,
            tab: Tab::General,
            pos: None,
            #[cfg(not(windows))]
            chooser: None,
        })
    }

    fn single(&self) -> bool {
        self.paths.len() == 1
    }

    /// Unix'te "gizli" adın başındaki noktadır; Windows'ta ayrı bir öznitelik
    fn hidden_is_name(&self) -> bool {
        cfg!(not(windows))
    }

    fn dirty(&self) -> bool {
        let hidden_changed = !self.hidden_is_name() && self.hidden != self.info.hidden;
        (self.single() && self.name.trim() != self.info.name) || self.readonly != self.info.readonly || hidden_changed || self.mode != self.info.mode
    }

    /// İkon için (yol, klasör mü, uzantı)
    fn icon_target(&self) -> (&Path, bool, String) {
        let p = &self.paths[0];
        let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        (p, self.info.is_dir, ext)
    }

    /// Görünen sekmeler, Windows'taki sırayla. Windows imzasız dosyada İmzalar sekmesini göstermez.
    fn tabs(&self) -> Vec<(Tab, &'static str)> {
        let mut v = vec![(Tab::General, "Genel")];
        if self.single() {
            let show_signatures = if cfg!(windows) {
                self.signatures.as_ref().is_some_and(|s| matches!(&*s.lock().unwrap(), Loaded::Done(l) if !l.is_empty()))
            } else {
                !self.info.is_dir
            };
            if show_signatures {
                v.push((Tab::DigitalSignatures, "Dijital imzalar"));
            }
            v.push((Tab::Security, "Güvenlik"));
            v.push((Tab::Details, "Ayrıntılar"));
            v.push((Tab::PreviousVersions, "Önceki sürümler"));
        }
        v
    }

    fn apply(&mut self) -> Outcome {
        let mut out = Outcome::default();
        let mut errors: Vec<String> = Vec::new();
        let ro_changed = self.readonly != self.info.readonly;

        #[cfg(windows)]
        {
            let hidden_changed = self.hidden != self.info.hidden;
            if ro_changed || hidden_changed {
                for p in &self.paths {
                    let r = crate::win::set_attributes(p, ro_changed.then_some(self.readonly), hidden_changed.then_some(self.hidden));
                    if let Err(e) = r {
                        errors.push(e.to_string());
                    }
                }
                out.refresh = true;
            }
        }
        #[cfg(not(windows))]
        if self.mode != self.info.mode || ro_changed {
            use std::os::unix::fs::PermissionsExt;
            for p in &self.paths {
                let mode = if self.single() {
                    self.mode
                } else {
                    // Çoklu seçimde yalnızca yazma bitleri değişir; diğer izinler olduğu gibi kalır
                    let cur = std::fs::metadata(p).map_or(0o644, |m| m.permissions().mode() & 0o7777);
                    if self.readonly { cur & !0o222 } else { cur | 0o200 }
                };
                if let Err(e) = std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)) {
                    errors.push(crate::i18n::tf("İzinler ayarlanamadı: {}", &[&e]));
                }
            }
            out.refresh = true;
        }

        if self.single() {
            let new_name = self.name.trim().to_string();
            if new_name != self.info.name {
                match self.rename(&new_name) {
                    Ok(to) => out.renamed = Some((self.paths[0].clone(), to)),
                    Err(e) => errors.push(e),
                }
            }
        }
        if let Some((_, to)) = &out.renamed {
            self.paths[0] = to.clone();
        }
        // Başarısız alanlar da dahil her şey diskten yeniden okunur: pencere gerçeği gösterir
        if let Some(info) = load_info(&self.paths, &self.type_name) {
            self.name = info.name.clone();
            self.readonly = info.readonly;
            self.hidden = info.hidden;
            self.mode = info.mode;
            self.info = info;
        }
        out.error = errors.into_iter().next();
        out
    }

    fn rename(&self, new_name: &str) -> Result<PathBuf, String> {
        let from = &self.paths[0];
        if !crate::app::valid_name(new_name) {
            return Err(crate::i18n::t("Ad şu karakterleri içeremez: \\ / : * ? \" < > |").to_string());
        }
        let to = from.with_file_name(new_name);
        // Yalnızca büyük/küçük harf farkı Windows'ta da çalışsın diye ayrı kontrol
        if to.exists() && !new_name.eq_ignore_ascii_case(&self.info.name) {
            return Err(crate::i18n::tf("\"{}\" adında bir öğe zaten var", &[&new_name]));
        }
        std::fs::rename(from, &to).map_err(|e| crate::i18n::tf("Yeniden adlandırılamadı: {}", &[&e]))?;
        Ok(to)
    }

    /// Özellikler penceresini ayrı bir işletim sistemi penceresi olarak çizer.
    /// `icon` verilen yerde sistem ikonunu çizer.
    pub fn show(&mut self, ctx: &egui::Context, icon: &mut IconFn) -> Outcome {
        let busy = self.stats.as_ref().is_some_and(|s| !s.done.load(Relaxed));
        if busy {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        let title = crate::i18n::tf("{} özellikleri", &[&self.info.name]);
        let pos = *self.pos.get_or_insert_with(|| centered_on_parent(ctx, WINDOW_SIZE));
        let builder = dialog_builder(&title, WINDOW_SIZE, pos);
        let mut out = Outcome::default();
        ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("swift_explorer_properties"), builder, |ctx, class| {
            out = self.viewport_ui(ctx, class, &title, busy, icon);
        });
        #[cfg(not(windows))]
        if self.chooser.is_some() {
            self.run_chooser(ctx);
        }
        out
    }

    fn viewport_ui(&mut self, ctx: &egui::Context, class: egui::ViewportClass, title: &str, busy: bool, icon: &mut IconFn) -> Outcome {
        let (mut ok, mut cancel, mut apply) = (false, false, false);
        let esc = ctx.input(|i| i.key_pressed(Key::Escape));
        cancel |= esc || ctx.input(|i| i.viewport().close_requested());

        let mut layout = |ui: &mut egui::Ui| {
            let dirty = self.dirty();
            egui::TopBottomPanel::bottom("props_buttons").frame(panel_frame(ui, 10)).show_inside(ui, |ui| {
                ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), BUTTON_H), Layout::right_to_left(Align::Center), |ui| {
                    let size = egui::vec2(80.0, BUTTON_H);
                    apply = ui.add_enabled(dirty, egui::Button::new(crate::i18n::t("Uygula")).min_size(size)).clicked();
                    cancel |= ui.add(egui::Button::new(crate::i18n::t("İptal")).min_size(size)).clicked();
                    ok |= ui.add(egui::Button::new(crate::i18n::t("Tamam")).min_size(size)).clicked();
                });
            });
            let tabs = self.tabs();
            if !tabs.iter().any(|(t, _)| *t == self.tab) {
                self.tab = Tab::General;
            }
            egui::TopBottomPanel::top("props_tabs").frame(panel_frame(ui, 8)).show_inside(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    for (tab, label) in &tabs {
                        ui.selectable_value(&mut self.tab, *tab, RichText::new(crate::i18n::t(label)).size(13.0));
                    }
                });
            });
            egui::CentralPanel::default().frame(panel_frame(ui, 12)).show_inside(ui, |ui| match self.tab {
                Tab::General => ok |= self.general_tab(ui, busy, icon),
                Tab::DigitalSignatures => self.signatures_tab(ui),
                Tab::Security => self.security_tab(ui),
                Tab::Details => self.details_tab(ui, busy),
                Tab::PreviousVersions => self.previous_versions_tab(ui),
            });
        };
        if class == egui::ViewportClass::Embedded {
            // Birden çok pencereyi desteklemeyen arka uçlarda uygulama penceresinin içinde açılır
            egui::Window::new(title).id(egui::Id::new("properties")).collapsible(false).resizable(false).show(ctx, |ui| {
                ui.set_min_size(WINDOW_SIZE);
                layout(ui);
            });
        } else {
            egui::CentralPanel::default().frame(egui::Frame::NONE).show(ctx, |ui| layout(ui));
        }

        let mut out = Outcome::default();
        if ok || apply {
            if self.dirty() {
                out = self.apply();
            }
            out.close = ok && out.error.is_none();
        }
        if cancel {
            out.close = true;
        }
        out
    }

    fn general_tab(&mut self, ui: &mut egui::Ui, busy: bool, icon: &mut IconFn) -> bool {
        let mut enter = false;
        ui.horizontal(|ui| {
            let (p, is_dir, ext) = self.icon_target();
            let p = p.to_path_buf();
            icon(ui, &p, is_dir, &ext, 32.0);
            ui.add_space(6.0);
            if self.single() {
                let r = ui.add(egui::TextEdit::singleline(&mut self.name).desired_width(f32::INFINITY));
                enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            } else {
                ui.label(crate::i18n::tf("{} öğe", &[&self.paths.len()]));
            }
        });
        ui.add_space(6.0);
        ui.separator();
        self.grid(ui, "props_type", |ui, row| {
            row(ui, "Tür", self.info.kind.clone());
        });
        if self.opens_with.is_some() && self.opens_with_row(ui, icon) {
            self.change_default_app(ui.ctx());
        }
        self.grid(ui, "props_loc", |ui, row| {
            if !self.info.location.is_empty() {
                row(ui, "Konum", self.info.location.clone());
            }
            if let Some(l) = &self.info.link {
                row(ui, "Bağlantı hedefi", l.clone());
            }
        });
        ui.separator();
        self.grid(ui, "props_size", |ui, row| {
            if let Some((size, on_disk)) = self.info.file_size {
                row(ui, "Boyut", size_full(size));
                row(ui, "Disk üzerindeki boyut", size_full(on_disk));
            } else if let Some(s) = &self.stats {
                let wait = if busy { format!("  {}", crate::i18n::t("Hesaplanıyor…")) } else { String::new() };
                row(ui, "Boyut", format!("{}{wait}", size_full(s.size.load(Relaxed))));
                row(ui, "Disk üzerindeki boyut", format!("{}{wait}", size_full(s.on_disk.load(Relaxed))));
                row(ui, "İçerik", crate::i18n::tf("{} Dosya, {} Klasör", &[&s.files.load(Relaxed), &s.dirs.load(Relaxed)]));
            }
        });
        if self.single() {
            ui.separator();
            self.grid(ui, "props_time", |ui, row| {
                row(ui, "Oluşturma", long_date(self.info.created));
                // Explorer klasörlerde yalnızca oluşturma tarihini gösterir
                if !self.info.is_dir {
                    row(ui, "Değiştirme", long_date(self.info.modified));
                    row(ui, "Erişim", long_date(self.info.accessed));
                }
            });
        } else {
            ui.separator();
        }
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(crate::i18n::t("Öznitelikler")).weak());
            ui.add_space(8.0);
            let mut ro = self.readonly;
            if ui.checkbox(&mut ro, crate::i18n::t("Salt okunur")).changed() {
                self.set_readonly(ro);
            }
            // Unix'te gizli olmak adın noktayla başlamasıdır; çoklu seçimde birden çok adı değiştirmeyiz
            let mut hidden = if self.hidden_is_name() { self.name.starts_with('.') } else { self.hidden };
            let can_hide = !self.hidden_is_name() || self.single();
            if ui.add_enabled(can_hide, egui::Checkbox::new(&mut hidden, crate::i18n::t("Gizli"))).changed() {
                if self.hidden_is_name() {
                    self.name = if hidden { format!(".{}", self.name) } else { self.name.trim_start_matches('.').to_string() };
                } else {
                    self.hidden = hidden;
                }
            }
        });
        enter
    }

    /// "Opens with: [simge] Notepad   [Değiştir…]"; Değiştir'e basıldıysa `true` döner
    fn opens_with_row(&self, ui: &mut egui::Ui, icon: &mut IconFn) -> bool {
        let Some(cell) = &self.opens_with else { return false };
        let guard = cell.lock().unwrap();
        let mut change = false;
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_sized([KEY_WIDTH, 20.0], egui::Label::new(RichText::new(crate::i18n::t("Açan uygulama")).weak()));
            match &*guard {
                Loaded::Pending => {
                    ui.label(RichText::new(crate::i18n::t("Yükleniyor…")).weak());
                }
                Loaded::Done(app) => {
                    if let Some(exe) = app.as_ref().and_then(|a| a.exe.as_deref()) {
                        icon(ui, exe, false, "exe", 18.0);
                    }
                    ui.label(app.as_ref().map_or_else(|| crate::i18n::t("Bilinmiyor").to_string(), |a| a.name.clone()));
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let pending = matches!(&*guard, Loaded::Pending);
                change = ui.add_enabled(!pending, egui::Button::new(crate::i18n::t("Değiştir…")).min_size(egui::vec2(90.0, 24.0))).clicked();
            });
        });
        ui.add_space(4.0);
        change
    }

    /// Windows: sistemin "Birlikte aç" penceresi. Linux: kurulu uygulamaların listesi (`xdg-mime` ile ayarlanır).
    fn change_default_app(&mut self, ctx: &egui::Context) {
        let p = self.paths[0].clone();
        #[cfg(windows)]
        {
            self.opens_with = Some(spawn_load(ctx, move || {
                crate::win::choose_default_app(&p);
                crate::win::default_app(&p)
            }));
        }
        #[cfg(not(windows))]
        {
            let data = spawn_load(ctx, move || {
                let mime = crate::win::mime_of(&p).unwrap_or_default();
                let current = crate::win::default_app_id(&mime);
                let (recommended, other) = crate::desktop_apps::installed_apps().into_iter().partition(|a| a.mimes.contains(&mime));
                ChooserData { mime, current, recommended, other }
            });
            self.chooser = Some(Chooser { data, selected: None, pos: None, synced: false, scroll_pending: false, icons: Default::default() });
        }
    }

    #[cfg(not(windows))]
    fn run_chooser(&mut self, ctx: &egui::Context) {
        let ext = self.paths[0].extension().map_or_else(|| self.info.name.clone(), |e| format!(".{}", e.to_string_lossy()));
        let Some(ch) = self.chooser.as_mut() else { return };
        match show_chooser(ch, ctx, &ext) {
            ChooserEvent::None => {}
            ChooserEvent::Cancel => self.chooser = None,
            ChooserEvent::Pick { id, mime } => {
                self.chooser = None;
                let p = self.paths[0].clone();
                self.opens_with = Some(spawn_load(ctx, move || {
                    crate::desktop_apps::set_default(&id, &mime);
                    crate::win::default_app(&p)
                }));
            }
        }
    }

    /// Salt okunur: Unix'te yazma izinlerini açıp kapatır (Güvenlik sekmesiyle tutarlı kalır)
    fn set_readonly(&mut self, ro: bool) {
        self.readonly = ro;
        if self.single() && cfg!(not(windows)) {
            self.mode = if ro { self.mode & !0o222 } else { self.mode | 0o200 };
        }
    }

    fn signatures_tab(&self, ui: &mut egui::Ui) {
        #[cfg(not(windows))]
        banner(ui, "Dijital imzalar bu sistemde desteklenmiyor.");
        #[cfg(windows)]
        {
            let Some(cell) = &self.signatures else { return };
            let guard = cell.lock().unwrap();
            let Loaded::Done(list) = &*guard else { return };
            ui.label(crate::i18n::t("İmza listesi"));
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_min_height(120.0);
                egui::Grid::new("props_sigs").num_columns(3).striped(true).spacing([14.0, 6.0]).min_col_width((ui.available_width() - 28.0) / 3.0).show(ui, |ui| {
                    for h in ["İmzalayanın adı", "Özet algoritması", "Zaman damgası"] {
                        ui.label(RichText::new(crate::i18n::t(h)).strong());
                    }
                    ui.end_row();
                    for s in list {
                        ui.label(&s.name);
                        ui.label(&s.digest);
                        ui.label(RichText::new(crate::i18n::t("Kullanılamıyor")).weak());
                        ui.end_row();
                    }
                });
            });
            if let Some(s) = list.first() {
                ui.add_space(6.0);
                ui.label(RichText::new(&s.message).weak());
            }
            ui.add_space(8.0);
            ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                if ui.add(egui::Button::new(crate::i18n::t("Ayrıntılar")).min_size(egui::vec2(90.0, 26.0))).clicked() {
                    crate::win::show_properties(&self.paths[0], Some("Digital Signatures"));
                }
            });
        }
    }

    fn previous_versions_tab(&self, ui: &mut egui::Ui) {
        #[cfg(not(windows))]
        banner(ui, "Önceki sürümler bu sistemde desteklenmiyor.");
        #[cfg(windows)]
        {
            ui.label(crate::i18n::t("Önceki sürümler Windows tarafından yönetilir (geri yükleme noktaları ve Dosya Geçmişi)."));
            ui.add_space(8.0);
            if ui.button(crate::i18n::t("Windows önceki sürümlerini aç")).clicked() {
                crate::win::show_properties(&self.paths[0], Some("Previous Versions"));
            }
        }
    }

    fn security_tab(&mut self, ui: &mut egui::Ui) {
        #[cfg(windows)]
        {
            ui.label(crate::i18n::t("Windows dosya izinleri sistem tarafından yönetilir."));
            ui.add_space(8.0);
            if ui.button(crate::i18n::t("Windows güvenlik ayarlarını aç")).clicked() {
                crate::win::show_properties(&self.paths[0], Some("Security"));
            }
        }
        #[cfg(not(windows))]
        {
            self.grid(ui, "props_owner", |ui, row| {
                row(ui, "Sahip", self.info.owner.clone());
                row(ui, "Grup", self.info.group.clone());
            });
            ui.separator();
            ui.label(RichText::new(crate::i18n::t("İzinler")).weak());
            ui.add_space(4.0);
            let mut mode = self.mode;
            egui::Grid::new("props_perm").num_columns(4).spacing([18.0, 8.0]).min_col_width((ui.available_width() - 54.0) / 4.0).show(ui, |ui| {
                ui.label("");
                for h in ["Okuma", "Yazma", "Çalıştırma"] {
                    ui.label(RichText::new(crate::i18n::t(h)).weak());
                }
                ui.end_row();
                for (who, shift) in [("Sahip", 6), ("Grup", 3), ("Diğerleri", 0)] {
                    ui.label(crate::i18n::t(who));
                    for bit in [0o4, 0o2, 0o1] {
                        let mask = bit << shift;
                        let mut on = mode & mask != 0;
                        if ui.checkbox(&mut on, "").changed() {
                            mode = if on { mode | mask } else { mode & !mask };
                        }
                    }
                    ui.end_row();
                }
            });
            if mode != self.mode {
                self.mode = mode;
                self.readonly = mode & 0o222 == 0;
            }
            ui.add_space(8.0);
            ui.label(RichText::new(format!("{}  ({:03o})", fsutil::mode_string(self.mode), self.mode)).monospace());
        }
    }

    fn details_tab(&self, ui: &mut egui::Ui, busy: bool) {
        let size = match (self.info.file_size, &self.stats) {
            (Some((s, _)), _) => size_short(s),
            (None, Some(st)) => format!("{}{}", size_short(st.size.load(Relaxed)), if busy { format!("  {}", crate::i18n::t("Hesaplanıyor…")) } else { String::new() }),
            _ => String::new(),
        };
        let i = &self.info;
        let mut rows: Vec<(&str, String)> = vec![
            ("Ad", i.name.clone()),
            ("Tür", i.kind.clone()),
            ("Konum", i.location.clone()),
            ("Boyut", size),
            ("Oluşturma", fsutil::format_time(i.created)),
            ("Değiştirme", fsutil::format_time(i.modified)),
            ("Erişim", fsutil::format_time(i.accessed)),
            ("Öznitelikler", i.attrs.clone()),
        ];
        if !i.owner.is_empty() {
            rows.push(("Sahip", format!("{}:{}", i.owner, i.group)));
        }
        if !i.mime.is_empty() {
            rows.push(("MIME türü", i.mime.clone()));
        }
        if let Some(l) = &i.link {
            rows.push(("Bağlantı hedefi", l.clone()));
        }
        egui::ScrollArea::vertical().show(ui, |ui| {
            let w = ui.available_width();
            let head = |ui: &mut egui::Ui, k: &str, v: &str| {
                ui.horizontal(|ui| {
                    ui.add_sized([KEY_WIDTH, 18.0], egui::Label::new(RichText::new(k).strong()));
                    ui.label(RichText::new(v).strong());
                });
            };
            head(ui, crate::i18n::t("Özellik"), crate::i18n::t("Değer"));
            ui.separator();
            for (n, (k, v)) in rows.into_iter().enumerate() {
                let fill = if n % 2 == 0 { ui.visuals().faint_bg_color } else { egui::Color32::TRANSPARENT };
                egui::Frame::new().fill(fill).inner_margin(egui::Margin::symmetric(4, 3)).show(ui, |ui| {
                    ui.set_width(w - 8.0);
                    ui.horizontal_top(|ui| {
                        ui.add_sized([KEY_WIDTH - 4.0, 18.0], egui::Label::new(RichText::new(crate::i18n::t(k)).weak()));
                        ui.add(egui::Label::new(v).wrap().selectable(true));
                    });
                });
            }
        });
    }

    /// Başlık ve değer satırları; değer kalan genişliği doldurur
    fn grid(&self, ui: &mut egui::Ui, _id: &str, add: impl FnOnce(&mut egui::Ui, &dyn Fn(&mut egui::Ui, &str, String))) {
        let row = |ui: &mut egui::Ui, k: &str, v: String| kv_row(ui, k, v);
        ui.add_space(2.0);
        add(ui, &row);
        ui.add_space(2.0);
    }
}

const KEY_WIDTH: f32 = 130.0;

fn kv_row(ui: &mut egui::Ui, key: &str, value: String) {
    ui.horizontal_top(|ui| {
        ui.add_sized([KEY_WIDTH, 18.0], egui::Label::new(RichText::new(crate::i18n::t(key)).weak()));
        ui.add(egui::Label::new(value).wrap().selectable(true));
    });
    ui.add_space(3.0);
}

/// Bu sistemde karşılığı olmayan sekmelerde gösterilen bilgi şeridi
#[cfg_attr(windows, allow(dead_code))]
fn banner(ui: &mut egui::Ui, text: &str) {
    egui::Frame::group(ui.style()).fill(ui.visuals().faint_bg_color).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(RichText::new("ℹ").size(18.0).weak());
            ui.add(egui::Label::new(RichText::new(crate::i18n::t(text)).weak()).wrap());
        });
    });
}

const BUTTON_H: f32 = 28.0;
const WINDOW_SIZE: egui::Vec2 = egui::vec2(470.0, 540.0);
#[cfg(not(windows))]
const ICON_SIZE: f32 = 22.0;
#[cfg(not(windows))]
const CHOOSER_SIZE: egui::Vec2 = egui::vec2(420.0, 500.0);

/// Ana pencerenin ortasına yerleşen bir pencerenin sol üst köşesi
fn centered_on_parent(ctx: &egui::Context, size: egui::Vec2) -> egui::Pos2 {
    let parent = ctx.input(|i| i.viewport().outer_rect).unwrap_or_else(|| ctx.screen_rect());
    parent.center() - size / 2.0
}

/// Ayrı, sistem başlık çubuklu iletişim penceresi
fn dialog_builder(title: &str, size: egui::Vec2, pos: egui::Pos2) -> egui::ViewportBuilder {
    egui::ViewportBuilder::default()
        .with_title(title)
        .with_app_id(crate::APP_ID)
        .with_inner_size(size)
        .with_position(pos)
        .with_resizable(false)
        .with_minimize_button(false)
        .with_maximize_button(false)
        .with_icon(eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")).unwrap_or_default())
}

/// Pencere içi bölmelerin çerçevesi: uygulamanın tema rengi ve eşit dolgu
fn panel_frame(ui: &egui::Ui, margin: i8) -> egui::Frame {
    egui::Frame::NONE.fill(ui.visuals().panel_fill).inner_margin(egui::Margin::symmetric(margin + 2, margin))
}

#[cfg(not(windows))]
struct ChooserData {
    mime: String,
    /// Şu an varsayılan olan uygulamanın kimliği
    current: Option<String>,
    recommended: Vec<crate::desktop_apps::DesktopApp>,
    other: Vec<crate::desktop_apps::DesktopApp>,
}

#[cfg(not(windows))]
struct Chooser {
    data: Shared<ChooserData>,
    selected: Option<String>,
    pos: Option<egui::Pos2>,
    /// Açılışta geçerli varsayılan uygulama seçilip görünür kılındı mı
    synced: bool,
    scroll_pending: bool,
    /// Uygulama kimliğine göre ikon dokuları; görünür satırlar için tembelce yüklenir
    icons: std::collections::HashMap<String, Option<egui::TextureHandle>>,
}

#[cfg(not(windows))]
enum ChooserEvent {
    None,
    Cancel,
    Pick { id: String, mime: String },
}

/// "Birlikte aç…" penceresi: dosya türü için varsayılan uygulamayı seçtirir (ayrı pencere)
#[cfg(not(windows))]
fn show_chooser(ch: &mut Chooser, ctx: &egui::Context, ext: &str) -> ChooserEvent {
    let mut event = ChooserEvent::None;
    let pos = *ch.pos.get_or_insert_with(|| centered_on_parent(ctx, CHOOSER_SIZE));
    let builder = dialog_builder(crate::i18n::t("Birlikte aç…"), CHOOSER_SIZE, pos);
    let guard = ch.data.clone();
    ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("swift_explorer_chooser"), builder, |ctx, class| {
        if ctx.input(|i| i.viewport().close_requested() || i.key_pressed(Key::Escape)) {
            event = ChooserEvent::Cancel;
        }
        let guard = guard.lock().unwrap();
        // Liste yüklenince geçerli varsayılan uygulama seçili gelir ve görünür kılınır
        if let (false, Loaded::Done(d)) = (ch.synced, &*guard) {
            ch.synced = true;
            ch.scroll_pending = d.current.is_some();
            ch.selected = d.current.clone();
        }
        let mut scroll = ch.scroll_pending;
        let mut layout = |ui: &mut egui::Ui| {
            egui::TopBottomPanel::bottom("chooser_buttons").frame(panel_frame(ui, 10)).show_inside(ui, |ui| {
                ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), BUTTON_H), Layout::right_to_left(Align::Center), |ui| {
                    let size = egui::vec2(80.0, BUTTON_H);
                    if ui.add(egui::Button::new(crate::i18n::t("İptal")).min_size(size)).clicked() {
                        event = ChooserEvent::Cancel;
                    }
                    let mime = match &*guard {
                        Loaded::Done(d) => d.mime.clone(),
                        Loaded::Pending => String::new(),
                    };
                    if let Some(id) = ch.selected.clone().filter(|_| !mime.is_empty()) {
                        if ui.add(egui::Button::new(crate::i18n::t("Tamam")).min_size(size)).clicked() {
                            event = ChooserEvent::Pick { id, mime };
                        }
                    } else {
                        ui.add_enabled(false, egui::Button::new(crate::i18n::t("Tamam")).min_size(size));
                    }
                });
            });
            egui::CentralPanel::default().frame(panel_frame(ui, 12)).show_inside(ui, |ui| {
                ui.label(crate::i18n::tf("Bu dosya türü için varsayılan uygulamayı seçin: {}", &[&ext]));
                ui.add_space(6.0);
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        let Loaded::Done(d) = &*guard else {
                            ui.label(RichText::new(crate::i18n::t("Yükleniyor…")).weak());
                            return;
                        };
                        // Kare başına yüklenen ikon sayısı sınırlıdır ki uzun liste takılmasın
                        let mut budget = 6;
                        let px = (ICON_SIZE * ctx.pixels_per_point()).ceil() as i32;
                        let mut section = |ui: &mut egui::Ui, title: &str, apps: &[crate::desktop_apps::DesktopApp]| {
                            if apps.is_empty() {
                                return;
                            }
                            ui.label(RichText::new(crate::i18n::t(title)).strong());
                            for a in apps {
                                let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 30.0), egui::Sense::click());
                                let sel = ch.selected.as_deref() == Some(a.id.as_str());
                                let v = ui.visuals();
                                if sel {
                                    ui.painter().rect_filled(rect, 4.0, v.selection.bg_fill);
                                } else if resp.hovered() {
                                    ui.painter().rect_filled(rect, 4.0, v.widgets.hovered.weak_bg_fill);
                                }
                                let text_color = if sel { v.selection.stroke.color } else { v.text_color() };
                                if ui.is_rect_visible(rect) {
                                    if !ch.icons.contains_key(&a.id) {
                                        if budget > 0 {
                                            budget -= 1;
                                            let tex = crate::shell::app_icon(a.icon.as_deref(), px).map(|img| ctx.load_texture(format!("app-icon-{}", a.id), img, egui::TextureOptions::LINEAR));
                                            ch.icons.insert(a.id.clone(), tex);
                                        } else {
                                            ctx.request_repaint();
                                        }
                                    }
                                    if let Some(Some(tex)) = ch.icons.get(&a.id) {
                                        let r = egui::Rect::from_min_size(rect.left_center() + egui::vec2(8.0, -ICON_SIZE / 2.0), egui::vec2(ICON_SIZE, ICON_SIZE));
                                        ui.painter().image(tex.id(), r, egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
                                    }
                                }
                                ui.painter().text(rect.left_center() + egui::vec2(8.0 + ICON_SIZE + 10.0, 0.0), egui::Align2::LEFT_CENTER, &a.name, egui::FontId::proportional(14.0), text_color);
                                if sel && scroll {
                                    resp.scroll_to_me(Some(Align::Center));
                                    scroll = false;
                                }
                                if resp.clicked() || resp.double_clicked() {
                                    ch.selected = Some(a.id.clone());
                                }
                                if resp.double_clicked() {
                                    event = ChooserEvent::Pick { id: a.id.clone(), mime: d.mime.clone() };
                                }
                            }
                            ui.add_space(6.0);
                        };
                        section(ui, "Önerilen uygulamalar", &d.recommended);
                        section(ui, "Diğer uygulamalar", &d.other);
                    });
                });
            });
        };
        if class == egui::ViewportClass::Embedded {
            egui::Window::new(crate::i18n::t("Birlikte aç…")).id(egui::Id::new("props_chooser")).collapsible(false).resizable(false).show(ctx, |ui| {
                ui.set_min_size(CHOOSER_SIZE);
                layout(ui);
            });
        } else {
            egui::CentralPanel::default().frame(egui::Frame::NONE).show(ctx, |ui| layout(ui));
        }
        ch.scroll_pending = false;
    });
    event
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digit_grouping() {
        assert_eq!(group_with(0, ","), "0");
        assert_eq!(group_with(999, ","), "999");
        assert_eq!(group_with(1000, ","), "1,000");
        assert_eq!(group_with(12_591, ","), "12,591");
        assert_eq!(group_with(1_234_567_890, "."), "1.234.567.890");
    }

    #[test]
    fn signable_types() {
        assert!(signable(Path::new("C:/x/ETL Drive Mapper.PS1")));
        assert!(signable(Path::new("setup.exe")));
        assert!(!signable(Path::new("notes.txt")));
        assert!(!signable(Path::new("Makefile")));
    }
}
