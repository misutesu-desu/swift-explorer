//! Dosya sistemi tarafı: girdiler, sıralama, desen eşleme, arka plan yükleyici/arama/işler.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

const ATTR_READONLY: u32 = 0x1;
const ATTR_HIDDEN: u32 = 0x2;
const ATTR_SYSTEM: u32 = 0x4;
pub const ATTR_DIR: u32 = 0x10;
const ATTR_REPARSE: u32 = 0x400;

/// Windows dosya öznitelikleri. Diğer sistemlerde aynı bitler üretilir:
/// adı nokta ile başlayan gizli, sembolik bağlantı "reparse point" sayılır.
#[cfg(windows)]
pub fn attributes(_path: &Path, md: &fs::Metadata) -> u32 {
    use std::os::windows::fs::MetadataExt;
    md.file_attributes()
}

#[cfg(not(windows))]
pub fn attributes(path: &Path, md: &fs::Metadata) -> u32 {
    let mut a = 0;
    if md.file_type().is_symlink() {
        a |= ATTR_REPARSE;
        if fs::metadata(path).is_ok_and(|m| m.is_dir()) {
            a |= ATTR_DIR;
        }
    } else if md.is_dir() {
        a |= ATTR_DIR;
    }
    if path.file_name().is_some_and(|n| n.as_encoded_bytes().first() == Some(&b'.')) {
        a |= ATTR_HIDDEN;
    }
    a
}

/// Gizli ya da sistem dosyası
pub fn is_hidden_attr(a: u32) -> bool {
    a & (ATTR_HIDDEN | ATTR_SYSTEM) != 0
}

#[derive(Clone)]
pub struct Entry {
    pub name: String,
    pub name_lower: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub hidden: bool,
    pub ext: String,
    /// Sıradan klasör: ikonu standart klasör ikonudur, tüm bu klasörler tek ikonu paylaşır.
    /// Windows klasöre özel ikonu (desktop.ini) yalnızca salt okunur ya da sistem
    /// özniteliği varsa uygular; bağlantı noktaları da ayrı tutulur.
    pub plain_dir: bool,
}

impl Entry {
    pub fn from_metadata(path: PathBuf, md: &fs::Metadata) -> Entry {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        let attrs = attributes(&path, md);
        let is_dir = attrs & ATTR_DIR != 0;
        let ext = if is_dir {
            String::new()
        } else {
            path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
        };
        Entry {
            name_lower: name.to_lowercase(),
            name,
            path,
            is_dir,
            size: if is_dir { 0 } else { md.len() },
            modified: md.modified().ok(),
            hidden: is_hidden_attr(attrs),
            ext,
            plain_dir: cfg!(windows) && is_dir && attrs & (ATTR_READONLY | ATTR_SYSTEM | ATTR_REPARSE) == 0,
        }
    }
}

// ---------------------------------------------------------------- sıralama

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, Debug)]
pub enum SortKey {
    #[default]
    Name,
    Modified,
    Type,
    Size,
}

/// "dosya2" < "dosya10" olacak şekilde doğal karşılaştırma. Bellek ayırmaz: on binlerce
/// öğeyi sıralarken her karşılaştırmada String oluşturmak belirgin takılmaya yol açıyordu.
/// UTF-8 bayt sırası kod noktası sırasıyla aynıdır; rakamlar tek bayttır.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        let (ca, cb) = (a[i], b[j]);
        if ca.is_ascii_digit() && cb.is_ascii_digit() {
            let digits = |s: &[u8], mut k: usize| {
                while k < s.len() && s[k] == b'0' {
                    k += 1;
                }
                let start = k;
                while k < s.len() && s[k].is_ascii_digit() {
                    k += 1;
                }
                (start, k)
            };
            let (sa, ea) = digits(a, i);
            let (sb, eb) = digits(b, j);
            let o = (ea - sa).cmp(&(eb - sb)).then_with(|| a[sa..ea].cmp(&b[sb..eb]));
            if o != Ordering::Equal {
                return o;
            }
            i = ea;
            j = eb;
        } else {
            if ca != cb {
                return ca.cmp(&cb);
            }
            i += 1;
            j += 1;
        }
    }
    (a.len() - i).cmp(&(b.len() - j))
}

pub fn compare(a: &Entry, b: &Entry, key: SortKey, asc: bool) -> Ordering {
    // Explorer gibi: tarihe göre sıralarken klasörler dosyalarla karışır, diğerlerinde önce gelir
    if key != SortKey::Modified && a.is_dir != b.is_dir {
        return b.is_dir.cmp(&a.is_dir);
    }
    let name = || natural_cmp(&a.name_lower, &b.name_lower);
    let o = match key {
        SortKey::Name => name(),
        SortKey::Size => a.size.cmp(&b.size).then_with(name),
        SortKey::Modified => a.modified.cmp(&b.modified).then_with(name),
        SortKey::Type => a.ext.cmp(&b.ext).then_with(name),
    };
    if asc {
        o
    } else {
        o.reverse()
    }
}

// ---------------------------------------------------------------- desen

/// Boşlukla ayrılmış kelimelerin hepsi geçmeli; `*` veya `?` varsa joker karakter eşlemesi.
#[derive(Clone)]
pub struct Pattern {
    glob: Option<Vec<char>>,
    words: Vec<String>,
}

impl Pattern {
    pub fn new(q: &str) -> Pattern {
        let q = q.trim().to_lowercase();
        if q.contains('*') || q.contains('?') {
            Pattern { glob: Some(q.chars().collect()), words: vec![] }
        } else {
            Pattern { glob: None, words: q.split_whitespace().map(str::to_owned).collect() }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.glob.is_none() && self.words.is_empty()
    }

    pub fn matches(&self, name_lower: &str) -> bool {
        match &self.glob {
            Some(g) => wildcard(g, &name_lower.chars().collect::<Vec<_>>()),
            None => self.words.iter().all(|w| name_lower.contains(w.as_str())),
        }
    }
}

fn wildcard(p: &[char], s: &[char]) -> bool {
    let (mut pi, mut si, mut star, mut mark) = (0, 0, None, 0);
    while si < s.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == s[si]) {
            pi += 1;
            si += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = si;
            pi += 1;
        } else if let Some(st) = star {
            pi = st + 1;
            mark += 1;
            si = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

// ---------------------------------------------------------------- biçimlendirme

pub fn format_size(n: u64) -> String {
    const U: [&str; 5] = ["bayt", "KB", "MB", "GB", "TB"];
    if n < 1024 {
        return crate::i18n::tf("{} bayt", &[&n]);
    }
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    let s = if v < 10.0 { format!("{v:.2}") } else if v < 100.0 { format!("{v:.1}") } else { format!("{v:.0}") };
    format!("{} {}", s.replace('.', ","), U[i])
}

pub fn format_time(t: Option<SystemTime>) -> String {
    match t {
        Some(t) => chrono::DateTime::<chrono::Local>::from(t).format("%d.%m.%Y %H:%M").to_string(),
        None => String::new(),
    }
}

/// Explorer'ın "Değiştirme tarihi"ne göre grup adları, en yeniden en eskiye
pub const DATE_GROUPS: [&str; 8] = ["Bugün", "Dün", "Bu haftanın başlarında", "Geçen hafta", "Bu ayın başlarında", "Geçen ay", "Bu yılın başlarında", "Uzun zaman önce"];

/// `DATE_GROUPS` içindeki sıra
pub fn date_group(t: Option<SystemTime>, now: chrono::DateTime<chrono::Local>) -> u8 {
    use chrono::{Datelike, Duration as CDur};
    let Some(t) = t else { return 7 };
    let d = chrono::DateTime::<chrono::Local>::from(t).date_naive();
    let today = now.date_naive();
    if d >= today {
        return 0;
    }
    if d == today - CDur::days(1) {
        return 1;
    }
    let week_start = today - CDur::days(today.weekday().num_days_from_monday() as i64);
    if d >= week_start {
        return 2;
    }
    if d >= week_start - CDur::days(7) {
        return 3;
    }
    let month_start = today.with_day(1).unwrap_or(today);
    if d >= month_start {
        return 4;
    }
    let last_month = month_start - CDur::days(1);
    if d.year() == last_month.year() && d.month() == last_month.month() {
        return 5;
    }
    if d.year() == today.year() {
        return 6;
    }
    7
}

pub fn kind_label(e: &Entry) -> String {
    if e.is_dir {
        crate::i18n::t("Klasör").into()
    } else if e.ext.is_empty() {
        crate::i18n::t("Dosya").into()
    } else {
        crate::i18n::tf("{} dosyası", &[&e.ext.to_uppercase()])
    }
}

pub fn is_image(ext: &str) -> bool {
    matches!(ext, "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "ico" | "svg")
}

pub fn icon(e: &Entry) -> &'static str {
    if e.is_dir {
        return "📁";
    }
    match e.ext.as_str() {
        x if is_image(x) => "🖼",
        "mp3" | "wav" | "flac" | "ogg" | "m4a" | "aac" | "wma" | "opus" => "🎵",
        "mp4" | "mkv" | "avi" | "mov" | "wmv" | "webm" | "flv" | "m4v" => "🎞",
        "zip" | "rar" | "7z" | "tar" | "gz" | "xz" | "bz2" | "zst" | "cab" | "iso" => "📦",
        "exe" | "msi" | "bat" | "cmd" | "ps1" | "dll" | "sys" | "com" => "⚙",
        "pdf" => "📕",
        "doc" | "docx" | "odt" | "rtf" | "xls" | "xlsx" | "csv" | "ppt" | "pptx" => "📊",
        "lnk" | "url" => "🔗",
        "txt" | "md" | "log" | "ini" | "cfg" | "json" | "toml" | "yaml" | "yml" | "xml" | "html" | "css"
        | "js" | "ts" | "rs" | "py" | "c" | "cpp" | "h" | "cs" | "java" | "go" | "lua" | "sh" => "📝",
        _ => "📄",
    }
}

// ---------------------------------------------------------------- dizin yükleyici

pub enum LoadMsg {
    Batch(Vec<Entry>),
    Done,
    Error(String),
}

/// Dizini arka planda okur. Windows'ta `DirEntry::metadata` ek sistem çağrısı yapmaz
/// (FindNextFile verisinden gelir), bu yüzden büyük klasörler bile çok hızlı listelenir.
pub fn spawn_load(path: PathBuf, ctx: egui::Context) -> Receiver<LoadMsg> {
    if let Some(rx) = take_prefetch(&path) {
        return rx;
    }
    spawn_load_with(path, move || ctx.request_repaint())
}

/// Açılışta, pencere ve GPU hazırlanırken başlatılan dizin okuması
static PREFETCH: Mutex<Option<(PathBuf, Receiver<LoadMsg>)>> = Mutex::new(None);

/// İlk sekmenin klasörünü pencere beklenmeden okumaya başlar; `spawn_load` aynı
/// yol için çağrılınca bu sonuç kullanılır.
pub fn prefetch(path: PathBuf) {
    let rx = spawn_load_with(path.clone(), crate::images::repaint);
    *PREFETCH.lock().unwrap() = Some((path, rx));
}

fn take_prefetch(path: &Path) -> Option<Receiver<LoadMsg>> {
    let mut slot = PREFETCH.lock().unwrap();
    match slot.take() {
        Some((p, rx)) if p == path => Some(rx),
        _ => None, // başka bir yere gidildi; eski sonuç atılır
    }
}

fn spawn_load_with(path: PathBuf, repaint: impl Fn() + Send + 'static) -> Receiver<LoadMsg> {
    let (tx, rx) = channel();
    thread::spawn(move || {
        let rd = match fs::read_dir(&path) {
            Ok(rd) => rd,
            Err(e) => {
                let _ = tx.send(LoadMsg::Error(e.to_string()));
                repaint();
                return;
            }
        };
        let mut batch = Vec::with_capacity(1024);
        let mut last = Instant::now();
        for de in rd.flatten() {
            if let Ok(md) = de.metadata() {
                batch.push(Entry::from_metadata(de.path(), &md));
            }
            if batch.len() >= 8192 || (last.elapsed() > Duration::from_millis(60) && !batch.is_empty()) {
                if tx.send(LoadMsg::Batch(std::mem::take(&mut batch))).is_err() {
                    return;
                }
                repaint();
                last = Instant::now();
            }
        }
        let _ = tx.send(LoadMsg::Batch(batch));
        let _ = tx.send(LoadMsg::Done);
        repaint();
    });
    rx
}

// ---------------------------------------------------------------- özyinelemeli arama

pub enum SearchMsg {
    Batch(Vec<Entry>),
    Done,
}

pub struct SearchHandle {
    pub rx: Receiver<SearchMsg>,
    pub cancel: Arc<AtomicBool>,
    pub scanned: Arc<AtomicU64>,
}

pub fn spawn_search(root: PathBuf, pattern: Pattern, ctx: egui::Context) -> SearchHandle {
    let (tx, rx) = channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let scanned = Arc::new(AtomicU64::new(0));
    let (c, s) = (cancel.clone(), scanned.clone());
    thread::spawn(move || {
        search_dir(&root, &pattern, &tx, &c, &s);
        let _ = tx.send(SearchMsg::Done);
        ctx.request_repaint();
    });
    SearchHandle { rx, cancel, scanned }
}

fn search_dir(dir: &Path, pat: &Pattern, tx: &Sender<SearchMsg>, cancel: &AtomicBool, scanned: &AtomicU64) {
    if cancel.load(Relaxed) {
        return;
    }
    let Ok(rd) = fs::read_dir(dir) else { return };
    scanned.fetch_add(1, Relaxed);
    let mut found = Vec::new();
    let mut subdirs = Vec::new();
    for de in rd.flatten() {
        let Ok(md) = de.metadata() else { continue };
        let path = de.path();
        let attrs = attributes(&path, &md);
        if attrs & ATTR_DIR != 0 && attrs & ATTR_REPARSE == 0 {
            subdirs.push(path.clone());
        }
        let name = de.file_name();
        if pat.matches(&name.to_string_lossy().to_lowercase()) {
            found.push(Entry::from_metadata(path, &md));
        }
    }
    if !found.is_empty() && tx.send(SearchMsg::Batch(found)).is_err() {
        cancel.store(true, Relaxed);
        return;
    }
    subdirs.par_iter().for_each(|d| search_dir(d, pat, tx, cancel, scanned));
}

// ---------------------------------------------------------------- klasör boyutu

pub fn dir_size(dir: &Path, cancel: &AtomicBool) -> u64 {
    if cancel.load(Relaxed) {
        return 0;
    }
    let Ok(rd) = fs::read_dir(dir) else { return 0 };
    let mut total = 0;
    let mut subdirs = Vec::new();
    for de in rd.flatten() {
        let Ok(md) = de.metadata() else { continue };
        let attrs = attributes(&de.path(), &md);
        if attrs & ATTR_DIR != 0 {
            if attrs & ATTR_REPARSE == 0 {
                subdirs.push(de.path());
            }
        } else {
            total += md.len();
        }
    }
    total + subdirs.par_iter().map(|d| dir_size(d, cancel)).sum::<u64>()
}

// ---------------------------------------------------------------- özellikler penceresi

/// Özellikler penceresi için bir ağacın toplam boyutu ve öğe sayıları; arka planda dolar
#[derive(Default)]
pub struct TreeStats {
    pub size: AtomicU64,
    /// Diskte kapladığı yer (Linux'ta blok sayısından); diğer sistemlerde `size` ile aynı
    pub on_disk: AtomicU64,
    pub files: AtomicU64,
    pub dirs: AtomicU64,
    pub done: AtomicBool,
    pub cancel: AtomicBool,
    /// Windows'ta diskteki yer küme boyutuna yuvarlanır (0: yuvarlama yok)
    #[cfg_attr(not(windows), allow(dead_code))]
    pub cluster: u64,
}

impl TreeStats {
    pub fn with_cluster(cluster: u64) -> TreeStats {
        TreeStats { cluster, ..Default::default() }
    }
}

/// `len` baytın `cluster` boyutlu kümelerde kapladığı yer
#[cfg_attr(not(windows), allow(dead_code))]
pub fn round_to_cluster(len: u64, cluster: u64) -> u64 {
    if cluster == 0 { len } else { len.div_ceil(cluster) * cluster }
}

/// `path` bir dosyaysa onu, klasörse içindekileri sayar. Sembolik bağlantılar izlenmez.
pub fn scan_tree(path: &Path, st: &TreeStats) {
    let Ok(md) = fs::symlink_metadata(path) else { return };
    if md.is_dir() {
        scan_dir(path, st);
    } else {
        add_file(&md, st);
    }
}

fn add_file(md: &fs::Metadata, st: &TreeStats) {
    st.files.fetch_add(1, Relaxed);
    st.size.fetch_add(md.len(), Relaxed);
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::MetadataExt;
        st.on_disk.fetch_add(md.blocks() * 512, Relaxed);
    }
    #[cfg(windows)]
    st.on_disk.fetch_add(round_to_cluster(md.len(), st.cluster), Relaxed);
}

fn scan_dir(dir: &Path, st: &TreeStats) {
    if st.cancel.load(Relaxed) {
        return;
    }
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut subdirs = Vec::new();
    for de in rd.flatten() {
        let Ok(ft) = de.file_type() else { continue };
        if ft.is_dir() {
            st.dirs.fetch_add(1, Relaxed);
            subdirs.push(de.path());
        } else if let Ok(md) = de.metadata() {
            add_file(&md, st);
        }
    }
    subdirs.par_iter().for_each(|d| scan_dir(d, st));
}

/// "rwxr-xr-x" biçimi; özel bitler (setuid, setgid, sticky) ls gibi gösterilir
#[cfg(not(windows))]
pub fn mode_string(mode: u32) -> String {
    let bit = |m: u32, c: char| if mode & m != 0 { c } else { '-' };
    // Çalıştırma hanesi: özel bit varsa s/t (çalıştırılabilirse küçük, değilse büyük harf)
    let exec = |x: u32, special: u32, lower: char| match (mode & x != 0, mode & special != 0) {
        (true, true) => lower,
        (false, true) => lower.to_ascii_uppercase(),
        (true, false) => 'x',
        (false, false) => '-',
    };
    [
        bit(0o400, 'r'),
        bit(0o200, 'w'),
        exec(0o100, 0o4000, 's'),
        bit(0o040, 'r'),
        bit(0o020, 'w'),
        exec(0o010, 0o2000, 's'),
        bit(0o004, 'r'),
        bit(0o002, 'w'),
        exec(0o001, 0o1000, 't'),
    ]
    .iter()
    .collect()
}

#[cfg(not(windows))]
pub fn user_name(uid: u32) -> Option<String> {
    let mut buf = vec![0 as libc::c_char; 4096];
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut res: *mut libc::passwd = std::ptr::null_mut();
    let rc = unsafe { libc::getpwuid_r(uid, &mut pwd, buf.as_mut_ptr(), buf.len(), &mut res) };
    if rc != 0 || res.is_null() {
        return None;
    }
    Some(unsafe { std::ffi::CStr::from_ptr(pwd.pw_name) }.to_string_lossy().into_owned())
}

#[cfg(not(windows))]
pub fn group_name(gid: u32) -> Option<String> {
    let mut buf = vec![0 as libc::c_char; 4096];
    let mut grp: libc::group = unsafe { std::mem::zeroed() };
    let mut res: *mut libc::group = std::ptr::null_mut();
    let rc = unsafe { libc::getgrgid_r(gid, &mut grp, buf.as_mut_ptr(), buf.len(), &mut res) };
    if rc != 0 || res.is_null() {
        return None;
    }
    Some(unsafe { std::ffi::CStr::from_ptr(grp.gr_name) }.to_string_lossy().into_owned())
}

// ---------------------------------------------------------------- işler (kopyala/taşı/sil/boyut)

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    Transfer,
    Delete,
    Size,
    Restore,
}

/// Hedefte aynı adda öğe varsa ne yapılacağı
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Conflict {
    KeepBoth,
    Replace,
    Skip,
}

pub struct Job {
    pub label: String,
    pub kind: JobKind,
    pub cut: bool,
    pub permanent: bool,
    pub started: Instant,
    pub total: AtomicU64,
    pub done: AtomicU64,
    pub files: AtomicU64,
    pub finished: AtomicBool,
    /// Kopyalama bitti, yazılanlar diske indiriliyor
    pub syncing: AtomicBool,
    pub cancel: AtomicBool,
    pub current: Mutex<String>,
    pub errors: Mutex<Vec<String>>,
    /// (kaynak, hedef) — geri alma için
    pub pairs: Mutex<Vec<(PathBuf, PathBuf)>>,
}

impl Job {
    fn new(label: String, kind: JobKind) -> Arc<Job> {
        Arc::new(Job {
            label,
            kind,
            cut: false,
            permanent: false,
            started: Instant::now(),
            total: AtomicU64::new(0),
            done: AtomicU64::new(0),
            files: AtomicU64::new(0),
            finished: AtomicBool::new(false),
            syncing: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
            current: Mutex::new(String::new()),
            errors: Mutex::new(Vec::new()),
            pairs: Mutex::new(Vec::new()),
        })
    }
    fn err(&self, e: String) {
        self.errors.lock().unwrap().push(e);
    }
    fn interrupted(&self) -> io::Result<()> {
        if self.cancel.load(Relaxed) {
            Err(io::Error::new(io::ErrorKind::Interrupted, crate::i18n::t("iptal edildi")))
        } else {
            Ok(())
        }
    }
}

/// Hedefte aynı adda öğe varsa Explorer gibi "ad - Kopya", "ad - Kopya (2)" üretir.
pub fn unique_destination(dir: &Path, name: &str, is_dir: bool) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let p = Path::new(name);
    let (stem, ext) = if is_dir {
        (name.to_string(), String::new())
    } else {
        (
            p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| name.into()),
            p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default(),
        )
    };
    for n in 1.. {
        let suffix = if n == 1 { crate::i18n::t(" - Kopya").to_string() } else { crate::i18n::tf(" - Kopya ({})", &[&n]) };
        let c = dir.join(format!("{stem}{suffix}{ext}"));
        if !c.exists() {
            return c;
        }
    }
    unreachable!()
}

fn tree_size(p: &Path, cancel: &AtomicBool) -> u64 {
    match fs::symlink_metadata(p) {
        Ok(md) if md.is_dir() => dir_size(p, cancel),
        Ok(md) => md.len(),
        Err(_) => 0,
    }
}

/// `written`: diske indirilecek dosyalar, bkz. `sync_to_disk`
fn copy_tree(src: &Path, dst: &Path, job: &Job, written: &mut Vec<PathBuf>) -> io::Result<()> {
    job.interrupted()?;
    let md = fs::symlink_metadata(src)?;
    let attrs = attributes(src, &md);
    if attrs & ATTR_DIR != 0 {
        if attrs & ATTR_REPARSE != 0 {
            return Ok(()); // bağlantı noktalarını izleme: sonsuz döngü riski
        }
        fs::create_dir_all(dst)?;
        for de in fs::read_dir(src)? {
            let de = de?;
            if let Err(e) = copy_tree(&de.path(), &dst.join(de.file_name()), job, written) {
                if e.kind() == io::ErrorKind::Interrupted {
                    return Err(e);
                }
                job.err(format!("{}: {e}", de.path().display()));
            }
        }
    } else {
        *job.current.lock().unwrap() = src.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let len = md.len();
        if len < 32 * 1024 * 1024 {
            fs::copy(src, dst)?; // CopyFileExW: küçük dosyalar için en hızlısı
            job.done.fetch_add(len, Relaxed);
        } else {
            let mut r = fs::File::open(src)?;
            let mut w = fs::File::create(dst)?;
            let mut buf = vec![0u8; 4 * 1024 * 1024];
            loop {
                if job.cancel.load(Relaxed) {
                    drop(w);
                    let _ = fs::remove_file(dst);
                    return job.interrupted();
                }
                let n = r.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                w.write_all(&buf[..n])?;
                job.done.fetch_add(n as u64, Relaxed);
            }
            if let Ok(t) = md.modified() {
                let _ = w.set_modified(t);
            }
        }
        job.files.fetch_add(1, Relaxed);
        written.push(dst.to_path_buf());
    }
    Ok(())
}

/// Yazılanları işletim sisteminin önbelleğinden diske indirir, terminaldeki `sync` gibi.
/// Bu olmadan iş bittiğinde veri hâlâ bellekte olabilir; USB bellek o anda çıkarılırsa
/// dosyalar yarım ya da boş kalır.
#[cfg(windows)]
fn sync_to_disk(written: &[PathBuf], _dest: &Path) -> Vec<String> {
    written.par_iter().filter_map(|f| flush_file(f).err().map(|e| format!("{}: {e}", f.display()))).collect()
}

/// FlushFileBuffers yazma izni ister; salt okunur dosyada öznitelik geçici olarak kaldırılır.
#[cfg(windows)]
fn flush_file(p: &Path) -> io::Result<()> {
    let open = || fs::OpenOptions::new().write(true).open(p)?.sync_all();
    match open() {
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
            let mut perm = fs::metadata(p)?.permissions();
            if !perm.readonly() {
                return Err(e);
            }
            #[allow(clippy::permissions_set_readonly_false)]
            perm.set_readonly(false);
            fs::set_permissions(p, perm.clone())?;
            let r = open();
            perm.set_readonly(true);
            fs::set_permissions(p, perm)?;
            r
        }
        r => r,
    }
}

/// syncfs hedefin bulunduğu dosya sisteminin tamamını tek çağrıda yazar; dosya dosya fsync'ten
/// çok daha hızlı. Aynı sürücüdeki yeniden adlandırmalar da böylece kalıcı olur.
#[cfg(not(windows))]
fn sync_to_disk(_written: &[PathBuf], dest: &Path) -> Vec<String> {
    use std::os::fd::AsRawFd;
    let d = match fs::File::open(dest) {
        Ok(d) => d,
        Err(e) => return vec![format!("{}: {e}", dest.display())],
    };
    if unsafe { libc::syncfs(d.as_raw_fd()) } == 0 {
        vec![]
    } else {
        vec![format!("{}: {}", dest.display(), io::Error::last_os_error())]
    }
}

pub fn spawn_transfer(srcs: Vec<PathBuf>, dest: PathBuf, cut: bool, conflict: Conflict, ctx: egui::Context) -> Arc<Job> {
    let verb = if cut { crate::i18n::t("Taşınıyor") } else { crate::i18n::t("Kopyalanıyor") };
    let dest_name = dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| dest.to_string_lossy().into_owned());
    let mut job = Job::new(crate::i18n::tf("{}: {} öğe → {}", &[&verb, &srcs.len(), &dest_name]), JobKind::Transfer);
    Arc::get_mut(&mut job).unwrap().cut = cut;
    let j = job.clone();
    thread::spawn(move || {
        let mut remaining = Vec::new();
        for s in srcs {
            let Some(name) = s.file_name().map(|n| n.to_string_lossy().into_owned()) else { continue };
            if dest.starts_with(&s) {
                j.err(crate::i18n::tf("{}: klasör kendi içine kopyalanamaz", &[&name]));
                continue;
            }
            let same_dir = s.parent() == Some(dest.as_path());
            if cut && same_dir {
                continue;
            }
            let is_dir = s.is_dir();
            let plain = dest.join(&name);
            let target = if same_dir {
                unique_destination(&dest, &name, is_dir)
            } else if plain.exists() {
                match conflict {
                    Conflict::KeepBoth => unique_destination(&dest, &name, is_dir),
                    Conflict::Replace => plain,
                    Conflict::Skip => continue,
                }
            } else {
                plain
            };
            // Aynı sürücüde taşıma: anında yeniden adlandırma
            if cut && fs::rename(&s, &target).is_ok() {
                j.files.fetch_add(1, Relaxed);
                j.pairs.lock().unwrap().push((s, target));
                continue;
            }
            remaining.push((s, target));
        }
        let total: u64 = remaining.par_iter().map(|(s, _)| tree_size(s, &j.cancel)).sum();
        j.total.store(total, Relaxed);
        let mut written = Vec::new();
        let mut copied = Vec::new();
        for (s, target) in remaining {
            match copy_tree(&s, &target, &j, &mut written) {
                Ok(()) => copied.push((s, target)),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => break,
                Err(e) => j.err(format!("{}: {e}", s.display())),
            }
        }
        j.syncing.store(true, Relaxed);
        ctx.request_repaint();
        let sync_errors = sync_to_disk(&written, &dest);
        let synced = sync_errors.is_empty();
        for e in sync_errors {
            j.err(e);
        }
        for (s, target) in copied {
            if cut {
                // Kaynak ancak kopyası diske yazıldıktan sonra silinir
                if !synced {
                    continue;
                }
                let r = if s.is_dir() { fs::remove_dir_all(&s) } else { fs::remove_file(&s) };
                if let Err(e) = r {
                    j.err(format!("{}: {e}", s.display()));
                }
            }
            j.pairs.lock().unwrap().push((s, target));
        }
        j.finished.store(true, Relaxed);
        ctx.request_repaint();
    });
    job
}

/// Hedef klasörde aynı adı taşıyan öğeler (aynı klasöre yapıştırma hariç)
pub fn conflicts(srcs: &[PathBuf], dest: &Path) -> Vec<String> {
    srcs.iter()
        .filter(|s| s.parent() != Some(dest))
        .filter_map(|s| s.file_name())
        .filter(|n| dest.join(n).exists())
        .map(|n| n.to_string_lossy().into_owned())
        .collect()
}

/// Geri dönüşüm kutusundaki en son silinmiş kopyaları geri yükler.
pub fn restore_from_trash(paths: &[PathBuf]) -> Result<(), String> {
    let items = trash::os_limited::list().map_err(|e| e.to_string())?;
    let low = |p: &Path| p.to_string_lossy().to_lowercase();
    // Çöp kutusu öğe adını "görünen ad"dan verir: uzantılar gizliyse "dosya.txt" → "dosya".
    let matches = |it: &trash::TrashItem, p: &Path| -> bool {
        let parent_ok = p.parent().map(|pp| low(pp) == low(&it.original_parent)).unwrap_or(false);
        let name = it.name.to_string_lossy().to_lowercase();
        let full = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        let stem = p.file_stem().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        parent_ok && (name == full || name == stem)
    };
    let mut picked: HashMap<PathBuf, trash::TrashItem> = HashMap::new();
    for it in items {
        if let Some(p) = paths.iter().find(|p| matches(&it, p)) {
            let newer = picked.get(p).is_none_or(|old| old.time_deleted < it.time_deleted);
            if newer {
                picked.insert(p.clone(), it);
            }
        }
    }
    if picked.is_empty() {
        return Err(crate::i18n::t("Geri dönüşüm kutusunda bulunamadı").into());
    }
    trash::os_limited::restore_all(picked.into_values()).map_err(|e| e.to_string())
}

pub fn spawn_delete(paths: Vec<PathBuf>, permanent: bool, ctx: egui::Context) -> Arc<Job> {
    let label = if permanent { crate::i18n::t("Kalıcı siliniyor") } else { crate::i18n::t("Geri dönüşüm kutusuna") };
    let mut job = Job::new(crate::i18n::tf("{}: {} öğe", &[&label, &paths.len()]), JobKind::Delete);
    Arc::get_mut(&mut job).unwrap().permanent = permanent;
    let j = job.clone();
    thread::spawn(move || {
        j.total.store(paths.len() as u64, Relaxed);
        if permanent {
            for p in &paths {
                if j.cancel.load(Relaxed) {
                    break;
                }
                *j.current.lock().unwrap() = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
                let r = if p.is_dir() { fs::remove_dir_all(p) } else { fs::remove_file(p) };
                if let Err(e) = r {
                    j.err(format!("{}: {e}", p.display()));
                }
                j.done.fetch_add(1, Relaxed);
            }
        } else if let Err(e) = trash::delete_all(&paths) {
            j.err(e.to_string());
        } else {
            j.done.store(paths.len() as u64, Relaxed);
            j.pairs.lock().unwrap().extend(paths.iter().map(|p| (p.clone(), p.clone())));
        }
        j.finished.store(true, Relaxed);
        ctx.request_repaint();
    });
    job
}

pub fn spawn_sizes(dirs: Vec<PathBuf>, out: Arc<Mutex<HashMap<PathBuf, u64>>>, ctx: egui::Context) -> Arc<Job> {
    let job = Job::new(crate::i18n::tf("Boyut hesaplanıyor: {} klasör", &[&dirs.len()]), JobKind::Size);
    let j = job.clone();
    thread::spawn(move || {
        j.total.store(dirs.len() as u64, Relaxed);
        for d in dirs {
            *j.current.lock().unwrap() = d.file_name().unwrap_or_default().to_string_lossy().into_owned();
            let s = dir_size(&d, &j.cancel);
            if j.cancel.load(Relaxed) {
                break;
            }
            out.lock().unwrap().insert(d, s);
            j.done.fetch_add(1, Relaxed);
            ctx.request_repaint();
        }
        j.finished.store(true, Relaxed);
        ctx.request_repaint();
    });
    job
}

pub fn spawn_restore(paths: Vec<PathBuf>, ctx: egui::Context) -> Arc<Job> {
    let job = Job::new(crate::i18n::tf("Geri yükleniyor: {} öğe", &[&paths.len()]), JobKind::Restore);
    let j = job.clone();
    thread::spawn(move || {
        if let Err(e) = restore_from_trash(&paths) {
            j.err(e);
        }
        j.finished.store(true, Relaxed);
        ctx.request_repaint();
    });
    job
}

/// Shell'in gerçek küçük resim üretebildiği türler (diğerleri için büyük ikon kullanılır)
pub fn has_thumbnail(ext: &str) -> bool {
    is_image(ext)
        || matches!(
            ext,
            "mp4" | "mkv" | "avi" | "mov" | "wmv" | "webm" | "m4v" | "pdf" | "heic" | "avif" | "tif" | "tiff" | "psd"
                | "cr2" | "nef" | "arw" | "dng" | "mp3" | "flac" | "m4a"
        )
}

// ---------------------------------------------------------------- önizleme

pub enum PreviewData {
    Text(String),
    Binary,
    Error(String),
}

pub fn read_preview(path: &Path) -> PreviewData {
    let mut f = match fs::File::open(path) {
        Ok(f) => f,
        Err(e) => return PreviewData::Error(e.to_string()),
    };
    let mut buf = vec![0u8; 64 * 1024];
    let n = match f.read(&mut buf) {
        Ok(n) => n,
        Err(e) => return PreviewData::Error(e.to_string()),
    };
    buf.truncate(n);
    // UTF-16 LE BOM
    if buf.starts_with(&[0xFF, 0xFE]) {
        let u: Vec<u16> = buf[2..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        return PreviewData::Text(String::from_utf16_lossy(&u));
    }
    if buf.iter().take(8192).any(|&b| b == 0) {
        return PreviewData::Binary;
    }
    PreviewData::Text(String::from_utf8_lossy(&buf).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cluster_rounding() {
        assert_eq!(round_to_cluster(0, 4096), 0);
        assert_eq!(round_to_cluster(1, 4096), 4096);
        assert_eq!(round_to_cluster(4096, 4096), 4096);
        assert_eq!(round_to_cluster(12_591, 4096), 16_384);
        assert_eq!(round_to_cluster(5, 0), 5);
    }

    #[cfg(not(windows))]
    #[test]
    fn mode_strings() {
        assert_eq!(mode_string(0o755), "rwxr-xr-x");
        assert_eq!(mode_string(0o644), "rw-r--r--");
        assert_eq!(mode_string(0o000), "---------");
        assert_eq!(mode_string(0o4755), "rwsr-xr-x");
        assert_eq!(mode_string(0o4644), "rwSr--r--");
        assert_eq!(mode_string(0o1777), "rwxrwxrwt");
        assert_eq!(mode_string(0o1776), "rwxrwxrwT");
    }

    #[test]
    fn tree_stats_count_files_dirs_and_size() {
        let root = std::env::temp_dir().join(format!("swift-explorer-props-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("a/b")).unwrap();
        fs::write(root.join("one.txt"), [0u8; 10]).unwrap();
        fs::write(root.join("a/two.txt"), [0u8; 20]).unwrap();
        fs::write(root.join("a/b/three.txt"), [0u8; 30]).unwrap();
        let st = TreeStats::default();
        scan_tree(&root, &st);
        assert_eq!(st.files.load(Relaxed), 3);
        assert_eq!(st.dirs.load(Relaxed), 2);
        assert_eq!(st.size.load(Relaxed), 60);
        let single = TreeStats::default();
        scan_tree(&root.join("one.txt"), &single);
        assert_eq!((single.files.load(Relaxed), single.dirs.load(Relaxed), single.size.load(Relaxed)), (1, 0, 10));
        let _ = fs::remove_dir_all(&root);
    }

    /// Önceki (bellek ayıran) sürüm: yenisi bununla birebir aynı sırayı vermeli
    fn natural_cmp_reference(a: &str, b: &str) -> Ordering {
        let (mut ai, mut bi) = (a.chars().peekable(), b.chars().peekable());
        loop {
            match (ai.peek().copied(), bi.peek().copied()) {
                (None, None) => return Ordering::Equal,
                (None, _) => return Ordering::Less,
                (_, None) => return Ordering::Greater,
                (Some(ca), Some(cb)) if ca.is_ascii_digit() && cb.is_ascii_digit() => {
                    let mut na = String::new();
                    while let Some(c) = ai.peek().copied().filter(char::is_ascii_digit) {
                        na.push(c);
                        ai.next();
                    }
                    let mut nb = String::new();
                    while let Some(c) = bi.peek().copied().filter(char::is_ascii_digit) {
                        nb.push(c);
                        bi.next();
                    }
                    let (ta, tb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                    let o = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                    if o != Ordering::Equal {
                        return o;
                    }
                }
                (Some(ca), Some(cb)) => {
                    if ca != cb {
                        return ca.cmp(&cb);
                    }
                    ai.next();
                    bi.next();
                }
            }
        }
    }

    #[test]
    fn natural_cmp_matches_reference() {
        let alphabet: Vec<char> = "0123456789aAbz._- çğışöüİ€😀".chars().collect();
        let mut seed = 0x9E3779B97F4A7C15u64;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let mut words = vec![
            "dosya2".to_string(), "dosya10".into(), "dosya010".into(), "dosya0010".into(), "a".into(), "".into(),
            "a1b2".into(), "a01b2".into(), "a1b02".into(), "x9".into(), "x09".into(), "x0".into(), "x".into(),
        ];
        for _ in 0..3000 {
            let n = (rnd() % 10) as usize;
            words.push((0..n).map(|_| alphabet[(rnd() % alphabet.len() as u64) as usize]).collect());
        }
        for a in &words {
            for b in words.iter().step_by(7) {
                assert_eq!(natural_cmp(a, b), natural_cmp_reference(a, b), "{a:?} <> {b:?}");
            }
        }
    }

    #[test]
    fn natural_cmp_large_sort_is_fast() {
        // WinSxS benzeri adlar: çok sayıda sayı parçası
        let names: Vec<String> = (0..15000)
            .map(|i| format!("amd64_microsoft-windows-{}_31bf3856ad364e35_10.0.{}.{}_none_{:x}", i % 97, 22621 + i % 13, i, i * 7919))
            .collect();
        let t = std::time::Instant::now();
        let mut v: Vec<&String> = names.iter().collect();
        v.sort_by(|a, b| natural_cmp(a, b));
        let new = t.elapsed();
        let t = std::time::Instant::now();
        let mut v2: Vec<&String> = names.iter().collect();
        v2.sort_by(|a, b| natural_cmp_reference(a, b));
        let old = t.elapsed();
        assert_eq!(v, v2);
        println!("15000 ad sıralama: yeni {new:?}, eski {old:?}");
    }

    #[test]
    fn copy_waits_for_disk_and_keeps_read_only_files() {
        let root = std::env::temp_dir().join(format!("swift-explorer-sync-{}", std::process::id()));
        let (src, dest) = (root.join("src"), root.join("dest"));
        fs::create_dir_all(src.join("klasör")).unwrap();
        fs::create_dir_all(&dest).unwrap();
        let ro = src.join("klasör").join("salt-okunur.txt");
        fs::write(&ro, b"veri").unwrap();
        let mut perm = fs::metadata(&ro).unwrap().permissions();
        perm.set_readonly(true);
        fs::set_permissions(&ro, perm).unwrap();

        let job = spawn_transfer(vec![src.join("klasör")], dest.clone(), false, Conflict::Replace, egui::Context::default());
        while !job.finished.load(Relaxed) {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(job.syncing.load(Relaxed));
        assert_eq!(*job.errors.lock().unwrap(), Vec::<String>::new());
        let copied = dest.join("klasör").join("salt-okunur.txt");
        assert_eq!(fs::read(&copied).unwrap(), b"veri");
        assert!(fs::metadata(&copied).unwrap().permissions().readonly());

        for f in [&ro, &copied] {
            let mut perm = fs::metadata(f).unwrap().permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            perm.set_readonly(false);
            fs::set_permissions(f, perm).unwrap();
        }
        fs::remove_dir_all(&root).unwrap();
    }
}
