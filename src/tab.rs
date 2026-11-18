//! Tek bir sekme: konum, geçmiş, girdiler, seçim, canlı izleme ve arama durumu.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering::Relaxed;
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use notify::{RecursiveMode, Watcher};

use crate::folder_lock::FolderLock;
use crate::fsutil::{self, Entry, LoadMsg, Pattern, SearchHandle, SearchMsg, SortKey};

/// Explorer'daki "Bu bilgisayar" sanal konumu (Shell ayrıştırma adı; ikonu da bununla alınır)
pub const THIS_PC: &str = "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}";

/// Klasör olmayan konumlar: Giriş (boş yol) ve Bu bilgisayar
pub fn is_virtual(p: &Path) -> bool {
    p.as_os_str().is_empty() || p.as_os_str() == THIS_PC
}

/// Görünümdeki bir grup ("Dün", "Geçen hafta"…): `view[start..start + len]`
pub struct Group {
    pub label: String,
    pub start: usize,
    pub len: usize,
    pub collapsed: bool,
}

pub struct Search {
    pub query: String,
    pub handle: SearchHandle,
    pub done: bool,
    pub started: Instant,
    pub elapsed: Duration,
    /// Yalnız bu uzantıdaki sonuçlar gösterilir
    pub ext: Option<String>,
    /// Sonuçlardaki uzantılar; sonuçlar gelirken en fazla yarım saniyede bir sayılır
    pub ext_counts: Vec<(String, usize)>,
    counted: usize,
    counted_at: Instant,
}

pub struct Tab {
    pub path: PathBuf,
    pub entries: Vec<Entry>,
    pending: Vec<Entry>,
    /// Filtrelenmiş + sıralanmış `entries` indeksleri
    pub view: Vec<usize>,
    pub view_dirty: bool,
    /// Seçim, imleç ve çapa `entries` indeksleridir (sıralamadan bağımsız).
    pub selection: HashSet<usize>,
    pub cursor: Option<usize>,
    pub anchor: Option<usize>,
    pub scroll_to_cursor: bool,
    pub back: Vec<PathBuf>,
    pub forward: Vec<PathBuf>,
    pub filter: String,
    /// Kayıtlı ama henüz okunmamış sekme
    lazy: bool,
    load_requested: bool,
    pub folder_access: Vec<FolderLock>,
    loader: Option<Receiver<LoadMsg>>,
    progressive: bool,
    load_started: Instant,
    pub load_time: Option<Duration>,
    pub error: Option<String>,
    pub select_after_load: Option<PathBuf>,
    /// Yükleme bitince yerinde yeniden adlandırmaya geçilecek öğe (yeni klasör vb.)
    pub rename_after_load: Option<PathBuf>,
    watcher: Option<notify::RecommendedWatcher>,
    watch_rx: Option<Receiver<()>>,
    refresh_at: Option<Instant>,
    pub search: Option<Search>,
    pub free_space: Option<(u64, u64)>,
    /// Boş alan arka planda sorgulanır (ağ sürücülerinde saniyeler sürebilir)
    free_rx: Option<Receiver<Option<(u64, u64)>>>,
    /// Boşsa gruplama yok
    pub groups: Vec<Group>,
    pub collapsed_groups: HashSet<String>,
    /// Bu bilgisayar sayfasında seçili sürücü
    pub cursor_path: Option<PathBuf>,
}

/// Sondaki "/" işaretlerini kırp, "~" ile başlayan yolu ev klasörüne çevir.
#[cfg(not(windows))]
pub fn normalize(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix('~') {
        if rest.is_empty() || rest.starts_with('/') {
            if let Some(home) = dirs::home_dir() {
                return normalize(&home.join(rest.trim_start_matches('/')));
            }
        }
    }
    let t = s.trim_end_matches('/');
    PathBuf::from(if t.is_empty() && s.starts_with('/') { "/" } else { t })
}

/// "C:" → "C:\", eğik çizgileri düzelt, sondaki ayırıcıyı kırp.
#[cfg(windows)]
pub fn normalize(p: &Path) -> PathBuf {
    let mut s = p.to_string_lossy().replace('/', "\\");
    if s.len() == 2 && s.ends_with(':') {
        s.push('\\');
    }
    while s.len() > 3 && s.ends_with('\\') {
        s.pop();
    }
    PathBuf::from(s)
}

impl Tab {
    pub fn new(path: PathBuf, ctx: &egui::Context) -> Tab {
        let mut t = Tab::blank();
        t.navigate(path, false, ctx);
        t
    }

    fn blank() -> Tab {
        Tab {
            path: PathBuf::new(),
            entries: vec![],
            pending: vec![],
            view: vec![],
            view_dirty: false,
            selection: HashSet::new(),
            cursor: None,
            anchor: None,
            scroll_to_cursor: false,
            back: vec![],
            forward: vec![],
            filter: String::new(),
            loader: None,
            progressive: true,
            load_started: Instant::now(),
            load_time: None,
            error: None,
            select_after_load: None,
            rename_after_load: None,
            watcher: None,
            watch_rx: None,
            refresh_at: None,
            search: None,
            free_space: None,
            free_rx: None,
            groups: vec![],
            collapsed_groups: HashSet::new(),
            cursor_path: None,
            lazy: false,
            load_requested: false,
            folder_access: vec![],
        }
    }

    /// Henüz yüklenmemiş sekme: klasör ilk görüntülendiğinde okunur (`ensure_loaded`)
    pub fn new_lazy(path: PathBuf) -> Tab {
        Tab { path: normalize(&path), lazy: true, ..Tab::blank() }
    }

    pub fn ensure_loaded(&mut self, ctx: &egui::Context) {
        if std::mem::take(&mut self.lazy) {
            let p = std::mem::take(&mut self.path);
            self.navigate(p, false, ctx);
        }
    }

    pub fn is_lazy(&self) -> bool {
        self.lazy
    }

    pub fn request_load(&mut self) -> bool {
        self.lazy && !std::mem::replace(&mut self.load_requested, true)
    }

    pub fn relock(&mut self) {
        self.cancel_search();
        let mut tab = Self::new_lazy(std::mem::take(&mut self.path));
        tab.back = std::mem::take(&mut self.back);
        tab.forward = std::mem::take(&mut self.forward);
        *self = tab;
    }

    pub fn title(&self) -> String {
        if let Some(s) = &self.search {
            return format!("🔍 {}", s.query);
        }
        if self.is_pc() {
            return crate::i18n::t("Bu bilgisayar").into();
        }
        if self.is_home() {
            return crate::i18n::t("Giriş").into();
        }
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.to_string_lossy().into_owned())
    }

    /// Gerçek bir klasör değil (Giriş ya da Bu bilgisayar)
    pub fn is_home(&self) -> bool {
        is_virtual(&self.path)
    }

    pub fn is_pc(&self) -> bool {
        self.path.as_os_str() == THIS_PC
    }

    /// Dosya listesi gösteriliyor: gerçek klasör ya da arama sonuçları
    pub fn lists_files(&self) -> bool {
        !self.is_home() || self.search.is_some()
    }

    pub fn is_loading(&self) -> bool {
        self.loader.is_some()
    }

    pub fn navigate(&mut self, path: PathBuf, push_history: bool, ctx: &egui::Context) {
        let path = normalize(&path);
        if push_history && path != self.path && !self.path.as_os_str().is_empty() {
            self.back.push(self.path.clone());
            self.forward.clear();
        }
        self.cancel_search();
        self.lazy = false;
        self.load_requested = false;
        self.folder_access.clear();
        self.path = path;
        self.filter.clear();
        self.entries.clear();
        self.pending.clear();
        self.view.clear();
        self.selection.clear();
        self.cursor = None;
        self.anchor = None;
        self.error = None;
        self.groups.clear();
        self.collapsed_groups.clear();
        self.cursor_path = None;
        if self.is_home() {
            self.loader = None;
            self.watcher = None;
            self.watch_rx = None;
            self.free_space = None;
            self.load_time = None;
            return;
        }
        self.free_space = None;
        self.refresh_free_space(ctx);
        self.load(true, ctx);
        self.watch(ctx);
    }

    pub fn go_back(&mut self, ctx: &egui::Context) {
        if let Some(p) = self.back.pop() {
            self.forward.push(self.path.clone());
            self.select_after_load = None;
            self.navigate(p, false, ctx);
        }
    }

    pub fn go_forward(&mut self, ctx: &egui::Context) {
        if let Some(p) = self.forward.pop() {
            self.back.push(self.path.clone());
            self.navigate(p, false, ctx);
        }
    }

    pub fn go_up(&mut self, ctx: &egui::Context) {
        if self.search.is_some() {
            self.exit_search(ctx);
            return;
        }
        if let Some(parent) = self.up_target() {
            if !self.is_home() {
                self.select_after_load = Some(self.path.clone());
            }
            self.navigate(parent, true, ctx);
        }
    }

    pub fn up_target(&self) -> Option<PathBuf> {
        if self.search.is_some() || self.path.as_os_str().is_empty() {
            return None;
        }
        if self.is_pc() {
            return Some(PathBuf::new());
        }
        // Sürücü kökünün üstü Explorer'daki gibi "Bu bilgisayar"
        Some(self.path.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from(THIS_PC)))
    }

    /// `progressive`: girdileri geldikçe göster (ilk açılış). Aksi halde eski liste
    /// yeni liste hazır olana kadar ekranda kalır — yenilemede titreme olmaz.
    pub fn load(&mut self, progressive: bool, ctx: &egui::Context) {
        if self.is_home() || self.lazy {
            return;
        }
        self.progressive = progressive;
        if progressive {
            self.entries.clear();
            self.selection.clear();
            self.cursor = None;
            self.anchor = None;
            self.view_dirty = true;
        }
        self.pending.clear();
        self.load_started = Instant::now();
        self.loader = Some(fsutil::spawn_load(self.path.clone(), ctx.clone()));
    }

    fn watch(&mut self, ctx: &egui::Context) {
        self.watcher = None;
        self.watch_rx = None;
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        let w = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(ev) = res {
                if !matches!(ev.kind, notify::EventKind::Access(_)) {
                    let _ = tx.send(());
                    ctx.request_repaint();
                }
            }
        });
        if let Ok(mut w) = w {
            if w.watch(&self.path, RecursiveMode::NonRecursive).is_ok() {
                self.watcher = Some(w);
                self.watch_rx = Some(rx);
            }
        }
    }

    /// `roots`: aranacak klasörler. Bu bilgisayar'da bunlar sürücü kökleridir.
    pub fn start_search(&mut self, roots: Vec<PathBuf>, ctx: &egui::Context) {
        let pattern = Pattern::new(&self.filter);
        if pattern.is_empty() || self.lazy {
            return;
        }
        self.cancel_search();
        self.loader = None;
        self.entries.clear();
        self.selection.clear();
        self.cursor = None;
        self.anchor = None;
        self.view_dirty = true;
        self.search = Some(Search {
            query: self.filter.trim().to_string(),
            handle: fsutil::spawn_search(roots, pattern, !self.is_home(), ctx.clone()),
            done: false,
            started: Instant::now(),
            elapsed: Duration::ZERO,
            ext: None,
            ext_counts: Vec::new(),
            counted: 0,
            counted_at: Instant::now(),
        });
        // Sonuçlar zaten eşleşiyor; liste filtresini temizle
        self.filter.clear();
    }

    pub fn cancel_search(&mut self) {
        if let Some(s) = self.search.take() {
            s.handle.cancel.store(true, Relaxed);
        }
    }

    pub fn exit_search(&mut self, ctx: &egui::Context) {
        if self.search.is_some() {
            self.cancel_search();
            if self.is_home() {
                // Bu bilgisayar'da yüklenecek klasör yok, sonuçları elle temizle
                self.entries.clear();
                self.selection.clear();
                self.cursor = None;
                self.view_dirty = true;
            } else {
                self.load(true, ctx);
            }
        }
    }

    fn swap_in(&mut self, new: Vec<Entry>) {
        let name_of = |i: Option<usize>, es: &Vec<Entry>| i.and_then(|i| es.get(i)).map(|e| e.name.clone());
        let sel: HashSet<String> = self.selection.iter().filter_map(|&i| self.entries.get(i)).map(|e| e.name.clone()).collect();
        let cur = name_of(self.cursor, &self.entries);
        let anc = name_of(self.anchor, &self.entries);
        self.entries = new;
        self.selection.clear();
        self.cursor = None;
        self.anchor = None;
        for (i, e) in self.entries.iter().enumerate() {
            if sel.contains(&e.name) {
                self.selection.insert(i);
            }
            if cur.as_deref() == Some(e.name.as_str()) {
                self.cursor = Some(i);
            }
            if anc.as_deref() == Some(e.name.as_str()) {
                self.anchor = Some(i);
            }
        }
        self.view_dirty = true;
    }

    pub fn refresh_free_space(&mut self, ctx: &egui::Context) {
        let (tx, rx) = channel();
        let (path, ctx) = (self.path.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(crate::win::disk_space(&path));
            ctx.request_repaint();
        });
        self.free_rx = Some(rx);
    }

    /// Arka plan kanallarını boşaltır. Her karede tüm sekmeler için çağrılır.
    pub fn poll(&mut self, ctx: &egui::Context) {
        if let Some(Ok(v)) = self.free_rx.as_ref().map(|rx| rx.try_recv()) {
            self.free_space = v;
            self.free_rx = None;
        }
        let mut finished = false;
        if let Some(rx) = &self.loader {
            loop {
                match rx.try_recv() {
                    Ok(LoadMsg::Batch(v)) => {
                        if self.progressive {
                            self.entries.extend(v);
                            self.view_dirty = true;
                        } else {
                            self.pending.extend(v);
                        }
                    }
                    Ok(LoadMsg::Done) => {
                        finished = true;
                        break;
                    }
                    Ok(LoadMsg::Error(e)) => {
                        self.error = Some(e);
                        finished = true;
                        break;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        finished = true;
                        break;
                    }
                }
            }
        }
        if finished {
            self.loader = None;
            self.load_time = Some(self.load_started.elapsed());
            if !self.progressive && self.error.is_none() {
                let new = std::mem::take(&mut self.pending);
                self.swap_in(new);
            }
            self.view_dirty = true;
        }

        // Canlı izleme: olayları toparla, kısa gecikmeyle sessizce yenile
        if let Some(rx) = &self.watch_rx {
            let mut got = false;
            while rx.try_recv().is_ok() {
                got = true;
            }
            if got && self.search.is_none() && self.refresh_at.is_none() {
                self.refresh_at = Some(Instant::now() + Duration::from_millis(120));
            }
        }
        if let Some(t) = self.refresh_at {
            let now = Instant::now();
            if now >= t && self.loader.is_none() {
                self.refresh_at = None;
                self.refresh_free_space(ctx);
                self.load(false, ctx);
            } else {
                ctx.request_repaint_after(t.saturating_duration_since(now).max(Duration::from_millis(16)));
            }
        }

        if let Some(s) = &mut self.search {
            loop {
                match s.handle.rx.try_recv() {
                    Ok(SearchMsg::Batch(v)) => {
                        self.entries.extend(v);
                        self.view_dirty = true;
                    }
                    Ok(SearchMsg::Done) | Err(TryRecvError::Disconnected) => {
                        if !s.done {
                            s.done = true;
                            s.elapsed = s.started.elapsed();
                        }
                        break;
                    }
                    Err(TryRecvError::Empty) => break,
                }
            }
            if !s.done {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }
    }

    /// Arama sonuçlarındaki uzantıları gerekirse yeniden sayar
    pub fn count_search_extensions(&mut self) {
        let Some(s) = &mut self.search else { return };
        let n = self.entries.len();
        if s.counted != n && (s.done || s.counted_at.elapsed() > Duration::from_millis(500)) {
            s.ext_counts = fsutil::top_extensions(&self.entries, 8);
            s.counted = n;
            s.counted_at = Instant::now();
        }
    }

    pub fn rebuild_view(&mut self, key: SortKey, asc: bool, show_hidden: bool, group_by_date: bool) {
        let pat = Pattern::new(&self.filter);
        let ext = self.search.as_ref().and_then(|s| s.ext.as_deref());
        let entries = &self.entries;
        self.view = (0..entries.len())
            .filter(|&i| {
                let e = &entries[i];
                (show_hidden || !e.hidden) && (pat.is_empty() || pat.matches(&e.name_lower)) && ext.is_none_or(|x| !e.is_dir && e.ext == x)
            })
            .collect();
        let cmp = |&a: &usize, &b: &usize| fsutil::compare(&entries[a], &entries[b], key, asc);
        // Büyük klasörlerde (ör. WinSxS) tüm çekirdeklerle sırala; küçüklerde iş parçacığı maliyetine değmez
        if self.view.len() > 4000 {
            use rayon::slice::ParallelSliceMut;
            self.view.par_sort_by(cmp);
        } else {
            self.view.sort_by(cmp);
        }
        self.groups.clear();
        if group_by_date && self.search.is_none() {
            // Explorer gibi: en yeni grup üstte, grup içinde seçili sıralama korunur (kararlı sıralama)
            let now = chrono::Local::now();
            let bucket = |i: usize| fsutil::date_group(entries[i].modified, now);
            self.view.sort_by_key(|&i| bucket(i));
            let mut all = std::mem::take(&mut self.view);
            let mut k = 0;
            while k < all.len() {
                let b = bucket(all[k]);
                let end = k + all[k..].iter().take_while(|&&i| bucket(i) == b).count();
                let label = fsutil::DATE_GROUPS[b as usize].to_string();
                let collapsed = self.collapsed_groups.contains(&label);
                let start = self.view.len();
                if !collapsed {
                    self.view.extend_from_slice(&all[k..end]);
                }
                self.groups.push(Group { label, start, len: self.view.len() - start, collapsed });
                k = end;
            }
            all.clear();
        }
        self.view_dirty = false;

        if let Some(p) = &self.select_after_load {
            if let Some(i) = entries.iter().position(|e| &e.path == p) {
                self.selection.clear();
                self.selection.insert(i);
                self.cursor = Some(i);
                self.anchor = Some(i);
                self.scroll_to_cursor = true;
                self.select_after_load = None;
            } else if self.loader.is_none() {
                self.select_after_load = None;
            }
        }
    }

    pub fn cursor_row(&self) -> Option<usize> {
        let c = self.cursor?;
        self.view.iter().position(|&i| i == c)
    }

    pub fn selected_entries(&self) -> Vec<&Entry> {
        let mut v: Vec<&Entry> = self.view.iter().filter(|i| self.selection.contains(i)).map(|&i| &self.entries[i]).collect();
        if v.is_empty() {
            if let Some(e) = self.cursor.and_then(|c| self.entries.get(c)) {
                if self.view.contains(&self.cursor.unwrap()) {
                    v.push(e);
                }
            }
        }
        v
    }

    pub fn selected_paths(&self) -> Vec<PathBuf> {
        self.selected_entries().into_iter().map(|e| e.path.clone()).collect()
    }

    pub fn select_only(&mut self, entry_idx: usize) {
        self.selection.clear();
        self.selection.insert(entry_idx);
        self.cursor = Some(entry_idx);
        self.anchor = Some(entry_idx);
    }

    pub fn select_range_to(&mut self, row: usize) {
        let anchor_row = self.anchor.and_then(|a| self.view.iter().position(|&i| i == a)).unwrap_or(row);
        let (lo, hi) = if anchor_row <= row { (anchor_row, row) } else { (row, anchor_row) };
        self.selection.clear();
        self.selection.extend(self.view[lo..=hi].iter().copied());
        self.cursor = Some(self.view[row]);
    }
}
