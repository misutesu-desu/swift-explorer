//! Arayüz dili: Türkçe metinler anahtar. İngilizce karşılıkları aşağıdaki tabloda,
//! diğer diller `i18n/` altında, her biri aynı anahtarlarla ayrı bir dosyada.
//! Dil, işletim sisteminin arayüz diline göre seçilir; Görünüm menüsünden değiştirilebilir.

mod de;
mod es;
mod fr;
mod ja;
mod ko;
mod pt;
mod ru;
mod zh;

use std::collections::HashMap;
use std::fmt::Display;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::sync::OnceLock;

/// Diller: kod ve kendi dilindeki adı. Ayarlarda bu listedeki sıra + 1 saklanır
/// (0 = sistem dili), bu yüzden yeni diller sona eklenir.
pub const LANGS: &[(&str, &str)] = &[
    ("tr", "Türkçe"),
    ("en", "English"),
    ("de", "Deutsch"),
    ("es", "Español"),
    ("fr", "Français"),
    ("pt", "Português (Brasil)"),
    ("ru", "Русский"),
    ("zh", "简体中文"),
    ("ja", "日本語"),
    ("ko", "한국어"),
];

const EN: usize = 1;

static LANG: AtomicUsize = AtomicUsize::new(0);

/// Geçerli dili `LANGS` içindeki sırasıyla seçer
pub fn set_lang(i: usize) {
    LANG.store(i.min(LANGS.len() - 1), Relaxed);
}

pub fn lang_code() -> &'static str {
    LANGS[LANG.load(Relaxed)].0
}

/// "de", "pt_BR.UTF-8", "zh-CN" gibi bir dil kodunun `LANGS` içindeki sırası
pub fn lang_index(code: &str) -> Option<usize> {
    let code = code.to_ascii_lowercase();
    if code.starts_with("zh") && !(code.starts_with("zh_cn") || code.starts_with("zh-cn") || code.starts_with("zh_sg") || code.starts_with("zh-sg") || code == "zh") {
        return None; // Geleneksel Çince için Basitleştirilmiş Çince yerine İngilizce
    }
    LANGS.iter().position(|(c, _)| code.starts_with(c))
}

/// Sistem dili (LC_ALL, LC_MESSAGES, LANG); desteklenmiyorsa İngilizce
#[cfg(not(windows))]
pub fn system_lang() -> usize {
    let lang = ["LC_ALL", "LC_MESSAGES", "LANG"].iter().find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()));
    lang.and_then(|l| lang_index(&l)).unwrap_or(EN)
}

/// Windows arayüz dili; desteklenmiyorsa İngilizce
#[cfg(windows)]
pub fn system_lang() -> usize {
    let lang = unsafe { windows_sys::Win32::Globalization::GetUserDefaultUILanguage() };
    let code = match (lang & 0x3FF, lang) {
        (0x1F, _) => "tr",
        (0x07, _) => "de",
        (0x0A, _) => "es",
        (0x0C, _) => "fr",
        (0x16, _) => "pt",
        (0x19, _) => "ru",
        (0x04, 0x0804 | 0x1004) => "zh",
        (0x11, _) => "ja",
        (0x12, _) => "ko",
        _ => "en",
    };
    lang_index(code).unwrap_or(EN)
}

fn tables() -> &'static [HashMap<&'static str, &'static str>] {
    static MAPS: OnceLock<Vec<HashMap<&'static str, &'static str>>> = OnceLock::new();
    MAPS.get_or_init(|| {
        let tables: &[&[(&str, &str)]] = &[&[], TABLE, de::TABLE, es::TABLE, fr::TABLE, pt::TABLE, ru::TABLE, zh::TABLE, ja::TABLE, ko::TABLE];
        tables.iter().map(|t| t.iter().copied().collect()).collect()
    })
}

/// Metnin geçerli dildeki karşılığı (Türkçe anahtar; çevirisi yoksa İngilizcesi, o da yoksa kendisi)
pub fn t(s: &str) -> &str {
    let i = LANG.load(Relaxed);
    if i == 0 {
        return s;
    }
    let maps = tables();
    maps[i].get(s).or_else(|| maps[EN].get(s)).copied().unwrap_or(s)
}

/// Yer tutuculu metin: `{}` sırayla, `{0}` `{1}` dizine göre doldurulur.
pub fn tf(key: &str, args: &[&dyn Display]) -> String {
    let tpl = t(key);
    let mut out = String::with_capacity(tpl.len() + 16);
    let mut next = 0;
    let mut rest = tpl;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        if let Some(stripped) = after.strip_prefix('{') {
            out.push('{');
            rest = stripped;
            continue;
        }
        let Some(j) = after.find('}') else {
            out.push_str(&rest[i..]);
            return out;
        };
        let inner = &after[..j];
        let idx = if inner.is_empty() {
            next += 1;
            next - 1
        } else {
            inner.parse().unwrap_or(usize::MAX)
        };
        if let Some(a) = args.get(idx) {
            out.push_str(&a.to_string());
        }
        rest = &after[j + 1..];
    }
    out.push_str(&rest.replace("}}", "}"));
    singular(out)
}

/// "1 items" → "1 item" gibi tekil düzeltmesi (sayının öncesinde başka rakam yoksa).
/// Kuralı olmayan dillerin metinleri çoğul eki gerektirmeyecek biçimde yazılır.
fn singular(s: String) -> String {
    let rules: &[(&str, &str)] = match lang_code() {
        "en" => &[("1 items", "1 item"), ("1 Files", "1 File"), ("1 Folders", "1 Folder")],
        "de" => &[("1 Elemente", "1 Element"), ("1 Dateien", "1 Datei")],
        "es" => &[("1 elementos seleccionados", "1 elemento seleccionado"), ("1 elementos", "1 elemento"), ("1 archivos", "1 archivo"), ("1 carpetas", "1 carpeta")],
        "fr" => &[("1 éléments sélectionnés", "1 élément sélectionné"), ("1 éléments", "1 élément"), ("1 fichiers", "1 fichier"), ("1 dossiers", "1 dossier")],
        "pt" => &[("1 itens selecionados", "1 item selecionado"), ("1 itens", "1 item"), ("1 arquivos", "1 arquivo"), ("1 pastas", "1 pasta")],
        _ => return s,
    };
    rules.iter().fold(s, |s, (many, one)| replace_count(s, many, one))
}

fn replace_count(s: String, many: &str, one: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s.as_str();
    while let Some(i) = rest.find(many) {
        let prev = rest[..i].chars().last();
        out.push_str(&rest[..i]);
        if prev.is_some_and(|c| c.is_ascii_digit() || c == '.' || c == ',') {
            out.push_str(many);
        } else {
            out.push_str(one);
        }
        rest = &rest[i + many.len()..];
    }
    out.push_str(rest);
    out
}

static TABLE: &[(&str, &str)] = &[
    ("Ayrıntılar", "Details"),
    ("Küçük simgeler", "Small icons"),
    ("Orta simgeler", "Medium icons"),
    ("Büyük simgeler", "Large icons"),
    ("Çok büyük simgeler", "Extra large icons"),
    ("Yeni klasör", "New folder"),
    ("Yeni Metin Belgesi.txt", "New Text Document.txt"),
    ("Oluşturulamadı: {}", "Couldn't create: {}"),
    ("Ad şu karakterleri içeremez: \\ / : * ? \" < > |", "A file name can't contain any of the following characters: \\ / : * ? \" < > |"),
    ("\"{}\" adında bir öğe zaten var", "There is already an item named \"{}\""),
    ("Yeniden adlandırılamadı: {}", "Couldn't rename: {}"),
    ("Geri alınacak bir işlem yok", "Nothing to undo"),
    ("Yeniden adlandırma geri alındı", "Rename undone"),
    ("Geri alınamadı: {}", "Couldn't undo: {}"),
    ("Oluşturma geri alındı", "Create undone"),
    ("Taşıma geri alındı", "Move undone"),
    ("Kopyalama geri alındı (kopyalar geri dönüşüm kutusuna)", "Copy undone (copies moved to the Recycle Bin)"),
    ("Windows \"{}\" öğesini bulamıyor. Adı doğru yazdığınızdan emin olun.", "Windows can't find \"{}\". Check the spelling and try again."),
    ("Açılamadı: {}", "Couldn't open: {}"),
    ("{} öğe {} — yapıştırmak için Ctrl+V", "{} item(s) {} — press Ctrl+V to paste"),
    ("kopyalandı", "copied"),
    ("kesildi", "cut"),
    ("Panoda yapıştırılacak dosya yok", "There are no files on the clipboard to paste"),
    ("Yol panoya kopyalandı", "Path copied to clipboard"),
    ("Hızlı erişime sabitlendi", "Pinned to Quick access"),
    ("Masaüstü", "Desktop"),
    ("İndirilenler", "Downloads"),
    ("Belgeler", "Documents"),
    ("Resimler", "Pictures"),
    ("Müzikler", "Music"),
    ("Videolar", "Videos"),
    ("Giriş", "Home"),
    ("Bu bilgisayar", "This PC"),
    ("Sekmeyi kapat (Ctrl+W)", "Close tab (Ctrl+W)"),
    ("Sekmeyi çoğalt", "Duplicate tab"),
    ("Yeni pencerede aç", "Open in new window"),
    ("Sekmeyi kapat", "Close tab"),
    ("Diğer sekmeleri kapat", "Close other tabs"),
    ("Yeni sekme (Ctrl+T)", "New tab (Ctrl+T)"),
    ("Geri getir", "Restore Down"),
    ("Ekranı kapla", "Maximize"),
    ("Simge durumuna küçült", "Minimize"),
    ("Geri (Alt+Sol ok)", "Back (Alt+Left Arrow)"),
    ("İleri (Alt+Sağ ok)", "Forward (Alt+Right Arrow)"),
    ("Üst klasör (Alt+Yukarı ok)", "Up to parent folder (Alt+Up Arrow)"),
    ("Yenile (F5)", "Refresh (F5)"),
    ("Sonuçlarda filtrele", "Filter results"),
    ("{} klasöründe ara", "Search {}"),
    ("Temizle (Esc)", "Clear (Esc)"),
    ("Yazdıkça bu klasörü anında filtreler.\nEnter: tüm alt klasörlerde ara.\n* ve ? joker karakterleri desteklenir (ör. *.pdf).", "Filters this folder as you type.\nEnter: search all subfolders.\n* and ? wildcards are supported (e.g. *.pdf)."),
    ("Tab: tamamla · Enter: git", "Tab: complete · Enter: go"),
    ("Yolu yazmak için tıklayın (Ctrl+L)", "Click to type a path (Ctrl+L)"),
    ("Alt klasör yok", "No subfolders"),
    ("Yeni", "New"),
    ("Klasör          Ctrl+Shift+N", "Folder          Ctrl+Shift+N"),
    ("Metin Belgesi", "Text Document"),
    ("Kes (Ctrl+X)", "Cut (Ctrl+X)"),
    ("Kopyala (Ctrl+C)", "Copy (Ctrl+C)"),
    ("Yapıştır (Ctrl+V)", "Paste (Ctrl+V)"),
    ("Yeniden adlandır (F2)", "Rename (F2)"),
    ("Yolu kopyala (Ctrl+Shift+C)", "Copy path (Ctrl+Shift+C)"),
    ("Sil (Del)  ·  Kalıcı sil: Shift+Del", "Delete (Del)  ·  Permanently delete: Shift+Del"),
    ("Ad", "Name"),
    ("Değiştirme tarihi", "Date modified"),
    ("Tür", "Type"),
    ("Boyut", "Size"),
    ("Sırala", "Sort"),
    ("Artan", "Ascending"),
    ("Azalan", "Descending"),
    ("Grupla", "Group by"),
    ("(Yok)", "(None)"),
    ("Görünüm", "View"),
    ("Ayrıntılar bölmesi (Alt+P)", "Details pane (Alt+P)"),
    ("Gizli öğeler (Ctrl+H)", "Hidden items (Ctrl+H)"),
    ("Dosya adı uzantıları", "File name extensions"),
    ("Varsayılan dosya gezgini olarak kullan", "Use as the default file manager"),
    ("Klasörler, sürücüler ve Win+E bu uygulamada açılır. Uygulamayı silmeden ya da taşımadan önce bunu kapatın.", "Folders, drives and Win+E open in this app. Turn this off before deleting or moving the app."),
    ("Klasörler bu uygulamada açılır.", "Folders open in this app."),
    ("Swift Explorer ile aç", "Open with Swift Explorer"),
    ("Anında açılış için arka planda bekle", "Wait in the background for instant startup"),
    ("Pencere kapanınca uygulama gizlenip bekler. Yeni sürümü kurmadan önce bunu kapatıp pencereyi kapatın.", "Closing the window hides the app and keeps it ready. Turn this off and close the window before installing a new version."),
    ("Oturum açılınca arka planda başlat", "Start in the background when signing in"),
    ("Güncellemeleri otomatik yükle", "Install updates automatically"),
    ("Swift Explorer {} yüklendi. Pencereyi kapatınca yeni sürüme geçilir.", "Swift Explorer {} is installed. Close the window to switch to the new version."),
    ("Yeni sürüm var: {}. İndirmek için: {}", "A new version is available: {}. Download: {}"),
    ("Güncellemeleri şimdi denetle", "Check for updates now"),
    ("Güncellemeler denetleniyor…", "Checking for updates…"),
    ("Swift Explorer güncel: {}", "Swift Explorer is up to date: {}"),
    ("Güncelleme denetlenemedi: {}", "Couldn't check for updates: {}"),
    ("Yeni sürüme geçmek için yeniden başlat", "Restart to switch to the new version"),
    ("Tema", "Theme"),
    ("Sistem", "System"),
    ("Açık", "Light"),
    ("Koyu", "Dark"),
    ("Dil", "Language"),
    ("Tümünü seç            Ctrl+A", "Select all            Ctrl+A"),
    ("Hiçbirini seçme        Esc", "Select none        Esc"),
    ("Seçimi tersine çevir", "Invert selection"),
    ("Geri al                   Ctrl+Z", "Undo                   Ctrl+Z"),
    ("Terminalde aç", "Open in Terminal"),
    ("{} başlatılamadı", "Couldn't start {}"),
    ("Klasör boyutlarını hesapla", "Calculate folder sizes"),
    ("Hızlı erişime sabitle   Ctrl+D", "Pin to Quick access   Ctrl+D"),
    ("Explorer'da aç", "Open in File Explorer"),
    ("Özellikler              Alt+Enter", "Properties              Alt+Enter"),
    ("Klavye kısayolları      F1", "Keyboard shortcuts      F1"),
    ("Aramayı kapat", "Close search"),
    ("Aç", "Open"),
    ("Yeni sekmede aç", "Open in new tab"),
    ("Hızlı erişime sabitle", "Pin to Quick access"),
    ("Özellikler", "Properties"),
    ("Hızlı erişimden kaldır", "Unpin from Quick access"),
    ("Ağ", "Network"),
    ("Ağ, Windows Dosya Gezgini'nde açılır", "Network opens in Windows File Explorer"),
    ("Hızlı erişim", "Quick access"),
    ("Sabitlendi", "Pinned"),
    ("Son", "Recent"),
    ("Son listesini temizle", "Clear recent list"),
    ("Cihazlar ve sürücüler", "Devices and drives"),
    ("{} boş, {} toplam", "{} free of {}"),
    ("Dosya klasörü", "File folder"),
    ("Dosya", "File"),
    ("{} Dosyası", "{} File"),
    ("Dosya yöneticisinde göster", "Show in file manager"),
    ("Dosya yöneticisinde aç", "Open in file manager"),
    ("Aç                              Enter", "Open                              Enter"),
    ("Buraya yapıştır", "Paste here"),
    ("Birlikte aç…", "Open with…"),
    ("Dosya konumunu aç", "Open file location"),
    ("Kes                             Ctrl+X", "Cut                             Ctrl+X"),
    ("Kopyala                       Ctrl+C", "Copy                       Ctrl+C"),
    ("Yol olarak kopyala   Ctrl+Shift+C", "Copy as path   Ctrl+Shift+C"),
    ("Yeniden adlandır          F2", "Rename          F2"),
    ("Sil                               Del", "Delete                               Del"),
    ("Boyutu hesapla", "Calculate size"),
    ("Explorer'da göster", "Show in File Explorer"),
    ("Özellikler                   Alt+Enter", "Properties                   Alt+Enter"),
    ("Daha fazla seçenek göster  Shift+F10", "Show more options  Shift+F10"),
    ("☰  Görünüm", "☰  View"),
    ("↕  Sırala", "↕  Sort"),
    ("Yenile                          F5", "Refresh                          F5"),
    ("Yapıştır                       Ctrl+V", "Paste                       Ctrl+V"),
    ("Geri al                         Ctrl+Z", "Undo                         Ctrl+Z"),
    ("➕  Yeni", "➕  New"),
    ("📁  Klasör    Ctrl+Shift+N", "📁  Folder    Ctrl+Shift+N"),
    ("📝  Metin Belgesi", "📝  Text Document"),
    ("Tümünü seç                Ctrl+A", "Select all                Ctrl+A"),
    ("Hızlı erişime sabitle    Ctrl+D", "Pin to Quick access    Ctrl+D"),
    ("⚠  Bu klasöre erişilemiyor: {}", "⚠  This folder can't be accessed: {}"),
    ("Aranıyor…", "Searching…"),
    ("Aramanızla eşleşen öğe yok.", "No items match your search."),
    ("Filtreyle eşleşen öğe yok — tüm alt klasörlerde aramak için Enter'a basın.", "No items match the filter — press Enter to search all subfolders."),
    ("Bu klasör boş.", "This folder is empty."),
    ("Klasör", "Folder"),
    ("Tür: {}", "Type: {}"),
    ("Boyut: {}", "Size: {}"),
    ("Değiştirme: {}", "Date modified: {}"),
    ("Ayrıntılarını görmek için bir öğe seçin.", "Select a single file to get more information."),
    ("{} öğe seçildi", "{} items selected"),
    ("Dosyaların toplam boyutu: {}", "Total size of files: {}"),
    ("{} öğe", "{} items"),
    ("{}  ({} bayt)", "{}  ({} bytes)"),
    ("Değiştirme", "Modified"),
    ("Oluşturma", "Created"),
    ("Konum", "Location"),
    ("Önizleme yapılamadı: {}", "Couldn't preview: {}"),
    ("Önizleme", "Preview"),
    ("{} öğe", "{} items"),
    ("(filtre: {} / {})", "(filter: {} / {})"),
    ("{} öğe seçildi  {}", "{} items selected  {}"),
    ("{} öğe seçildi", "{} items selected"),
    ("Arama tamamlandı · {} klasör · {} sn", "Search complete · {} folders · {} s"),
    ("Aranıyor… {} klasör tarandı", "Searching… {} folders scanned"),
    ("Büyük simgeler (Ctrl+Shift+2)", "Large icons (Ctrl+Shift+2)"),
    ("Ayrıntılar (Ctrl+Shift+6)", "Details (Ctrl+Shift+6)"),
    ("İptal", "Cancel"),
    ("Taşı", "Move"),
    ("Taşı / kopyala (Ctrl: kopyala, Shift: taşı)", "Move / copy (Ctrl: copy, Shift: move)"),
    ("\"{}\" klasörüne kopyalamak için bırakın", "Drop to copy to \"{}\""),
    ("Kalıcı olarak silinsin mi?", "Delete permanently?"),
    ("Bu {} öğe", "These {} items"),
    ("{} geri dönüşüm kutusuna gitmeden silinecek. Bu işlem geri alınamaz.", "{} will be deleted without going to the Recycle Bin. This can't be undone."),
    ("… ve {} öğe daha", "… and {} more"),
    ("Vazgeç", "Cancel"),
    ("Kalıcı olarak sil", "Delete permanently"),
    ("Dosyaları değiştir veya atla", "Replace or Skip Files"),
    ("Hedef \"{}\" klasöründe aynı adlı {} öğe var:", "The destination \"{}\" has {} item(s) with the same names:"),
    ("Hedefteki dosyaları değiştir", "Replace the files in the destination"),
    ("Klasörler birleştirilir, aynı adlı dosyaların üzerine yazılır", "Folders are merged, files with the same name are overwritten"),
    ("Bu dosyaları atla", "Skip these files"),
    ("Aynı adlı öğeler kopyalanmaz", "Items with the same name aren't copied"),
    ("İkisini de sakla", "Keep both"),
    ("Yeni öğeler \"- Kopya\" ekiyle adlandırılır", "New items get a \"- Copy\" suffix"),
    ("Klavye kısayolları", "Keyboard shortcuts"),
    ("Dosya Gezgini'ndeki kısayolların hepsi aynı; ekstralar ★ ile işaretli.", "All File Explorer shortcuts work the same; extras are marked with ★."),
    ("Kapat", "Close"),
    ("{} sn", "{} s"),
    ("{} dk {} sn", "{} min {} s"),
    ("{} sa {} dk", "{} h {} min"),
    ("Enter / çift tık", "Enter / double-click"),
    ("Alt+Sol / Alt+Sağ", "Alt+Left / Alt+Right"),
    ("Geri / İleri (farenin yan tuşları da)", "Back / Forward (mouse side buttons too)"),
    ("Alt+Yukarı / Backspace", "Alt+Up / Backspace"),
    ("Üst klasör", "Parent folder"),
    ("Adres çubuğu (Tab ile otomatik tamamla)", "Address bar (Tab to autocomplete)"),
    ("Arama kutusu — yazdıkça filtreler, Enter alt klasörlerde arar ★", "Search box — filters as you type, Enter searches subfolders ★"),
    ("Kopyala / Kes / Yapıştır (Explorer ile karşılıklı)", "Copy / Cut / Paste (works with File Explorer)"),
    ("Geri al: yeniden adlandırma, taşıma, kopyalama, silme", "Undo: rename, move, copy, delete"),
    ("Yeniden adlandır (yerinde)", "Rename (in place)"),
    ("Geri dönüşüm kutusuna / kalıcı sil", "To Recycle Bin / delete permanently"),
    ("Tümünü seç", "Select all"),
    ("Shift+ok / Ctrl+Boşluk", "Shift+Arrow / Ctrl+Space"),
    ("Aralık seç / tek tek seç", "Select range / select individually"),
    ("Harf yazmak", "Type a letter"),
    ("O harfle başlayan öğeye atla", "Jump to the item starting with that letter"),
    ("Ayrıntılar / önizleme bölmesi", "Details / preview pane"),
    ("Çok büyük … ayrıntılar görünümü", "Extra large … details view"),
    ("Ctrl+fare tekerleği", "Ctrl+mouse wheel"),
    ("Simge boyutunu değiştir", "Change icon size"),
    ("Yeni sekme / sekmeyi kapat", "New tab / close tab"),
    ("Sekmeler arası geçiş", "Switch between tabs"),
    ("Yeni pencere", "New window"),
    ("Shift+sağ tık / Shift+F10", "Shift+right-click / Shift+F10"),
    ("Windows'un tam sağ tık menüsü", "Full Windows context menu"),
    ("Orta tık", "Middle-click"),
    ("Klasörü yeni sekmede aç / sekmeyi kapat ★", "Open folder in new tab / close tab ★"),
    ("Sürükle-bırak", "Drag and drop"),
    ("Aynı sürücü: taşı, farklı sürücü: kopyala (Ctrl: kopyala, Shift: taşı)", "Same drive: move, other drive: copy (Ctrl: copy, Shift: move)"),
    ("Yolu kopyala", "Copy path"),
    ("Hızlı erişime sabitle ★", "Pin to Quick access ★"),
    ("Gizli öğeleri göster/gizle ★", "Show/hide hidden items ★"),
    ("Yenile", "Refresh"),
    ("Tam ekran", "Full screen"),
    ("Aramayı kapat / filtreyi temizle / seçimi kaldır", "Close search / clear filter / clear selection"),
    (" (+{} hata)", " (+{} errors)"),
    ("İptal edildi: {}", "Canceled: {}"),
    ("{} öğe geri dönüşüm kutusuna taşındı — geri almak için Ctrl+Z", "{} items moved to the Recycle Bin — press Ctrl+Z to undo"),
    ("Geri dönüşüm kutusundan geri yüklendi", "Restored from the Recycle Bin"),
    ("Diske yazılıyor, bitene kadar sürücüyü çıkarmayın", "Writing to disk, don't remove the drive until it finishes"),
    ("Diske yazıldı, sürücüyü çıkarabilirsiniz: {}", "Written to disk, the drive can be removed: {}"),
    ("{} bayt", "{} bytes"),
    ("Bugün", "Today"),
    ("Dün", "Yesterday"),
    ("Bu haftanın başlarında", "Earlier this week"),
    ("Geçen hafta", "Last week"),
    ("Bu ayın başlarında", "Earlier this month"),
    ("Geçen ay", "Last month"),
    ("Bu yılın başlarında", "Earlier this year"),
    ("Uzun zaman önce", "A long time ago"),
    ("{} dosyası", "{} File"),
    ("iptal edildi", "canceled"),
    (" - Kopya", " - Copy"),
    (" - Kopya ({})", " - Copy ({})"),
    ("Taşınıyor", "Moving"),
    ("Kopyalanıyor", "Copying"),
    ("{}: {} öğe → {}", "{}: {} items → {}"),
    ("{}: klasör kendi içine kopyalanamaz", "{}: a folder can't be copied into itself"),
    ("Geri dönüşüm kutusunda bulunamadı", "Not found in the Recycle Bin"),
    ("Kalıcı siliniyor", "Deleting permanently"),
    ("Geri dönüşüm kutusuna", "To Recycle Bin"),
    ("{}: {} öğe", "{}: {} items"),
    ("Boyut hesaplanıyor: {} klasör", "Calculating size: {} folders"),
    ("Geri yükleniyor: {} öğe", "Restoring: {} items"),
    ("Çıkarılabilir disk", "Removable Disk"),
    ("Yerel Disk", "Local Disk"),
    ("Ağ sürücüsü", "Network Drive"),
    ("Sürücü", "Drive"),
    ("%{}  ·  {}/sn  ·  yaklaşık {} kaldı", "{0}%  ·  {1}/s  ·  about {2} left"),
    ("Erişim", "Accessed"),
    ("Disk üzerindeki boyut", "Size on disk"),
    ("İçerik", "Contains"),
    ("{} Dosya, {} Klasör", "{} Files, {} Folders"),
    ("Hesaplanıyor…", "Calculating…"),
    ("Bağlantı hedefi", "Link target"),
    ("Sahip", "Owner"),
    ("Grup", "Group"),
    ("İzinler", "Permissions"),
    ("Özellikler gösterilemedi", "Couldn't show properties"),
    ("Birden çok tür", "Multiple types"),
    ("Değer", "Value"),
    ("Diğerleri", "Others"),
    ("Genel", "General"),
    ("Gizli", "Hidden"),
    ("Güvenlik", "Security"),
    ("MIME türü", "MIME type"),
    ("Okuma", "Read"),
    ("Salt okunur", "Read-only"),
    ("Tamam", "OK"),
    ("Uygula", "Apply"),
    ("Yazma", "Write"),
    ("{} özellikleri", "{} Properties"),
    ("Çalıştırma", "Execute"),
    ("Özellik", "Property"),
    ("Öznitelikler", "Attributes"),
    ("İzinler ayarlanamadı: {}", "Couldn't set permissions: {}"),
    ("Pazartesi", "Monday"),
    ("Salı", "Tuesday"),
    ("Çarşamba", "Wednesday"),
    ("Perşembe", "Thursday"),
    ("Cuma", "Friday"),
    ("Cumartesi", "Saturday"),
    ("Pazar", "Sunday"),
    ("Ocak", "January"),
    ("Şubat", "February"),
    ("Mart", "March"),
    ("Nisan", "April"),
    ("Mayıs", "May"),
    ("Haziran", "June"),
    ("Temmuz", "July"),
    ("Ağustos", "August"),
    ("Eylül", "September"),
    ("Ekim", "October"),
    ("Kasım", "November"),
    ("Aralık", "December"),
    ("{2} {1} {3} {0}, {4}", "{0}, {1} {2}, {3}, {4}"),
    ("%H:%M:%S", "%-I:%M:%S %p"),
    (",", "."),
    (".", ","),
    ("Kullanılamıyor", "Not available"),
    ("Açan uygulama", "Opens with:"),
    ("Bilinmiyor", "Unknown application"),
    ("Değiştir…", "Change…"),
    ("Dijital imzalar", "Digital Signatures"),
    ("Önceki sürümler", "Previous Versions"),
    ("Dijital imzalar bu sistemde desteklenmiyor.", "Digital signatures aren't supported on this system."),
    ("Önceki sürümler bu sistemde desteklenmiyor.", "Previous versions aren't supported on this system."),
    ("Önerilen uygulamalar", "Recommended apps"),
    ("Diğer uygulamalar", "Other apps"),
    ("Bu dosya türü için varsayılan uygulamayı seçin: {}", "Choose the default app for this file type: {}"),
    ("Yükleniyor…", "Loading…"),
    ("Adres çubuğu", "Address bar"),
    ("Ara", "Search"),
    ("Yeni metin belgesi", "New text document"),
    ("Komut adı yazın", "Type a command name"),
    ("Eşleşen komut yok", "No matching commands"),
    ("Komut paleti: bütün komutları adıyla ara ★", "Command palette: find any command by name ★"),
    ("Bu klasörün bulunduğu git deposunun etkin dalı", "Current branch of the git repository this folder is in"),
    ("İçindekiler", "Contents"),
    ("Klasöre git", "Go to folder"),
    ("Tümü", "All"),
    ("Toplu yeniden adlandır", "Batch rename"),
    ("Bul", "Find"),
    ("Şununla değiştir", "Replace with"),
    ("Ad şablonu", "Name pattern"),
    ("{name}: eski ad · {n}: numara · {date}: değiştirme tarihi", "{name}: old name · {n}: number · {date}: date modified"),
    ("Numara", "Number"),
    ("başlangıç", "start"),
    ("basamak", "digits"),
    ("Harfler", "Letters"),
    ("Olduğu gibi", "Unchanged"),
    ("küçük harf", "lowercase"),
    ("BÜYÜK HARF", "UPPERCASE"),
    ("Baş Harfler Büyük", "Title Case"),
    ("geçersiz ad", "invalid name"),
    ("aynı ad iki kez", "same name twice"),
    ("bu adda bir öğe zaten var", "an item with this name already exists"),
    ("{} adda sorun var", "{} names have a problem"),
    ("{} öğenin adı değişecek", "{} items will be renamed"),
    ("Yeniden adlandır", "Rename"),
    ("{} öğe yeniden adlandırıldı", "{} items renamed"),
    ("F2 (birden çok öğe seçiliyken)", "F2 (with several items selected)"),
    ("Toplu yeniden adlandır: bul ve değiştir, numara, tarih ★", "Batch rename: find and replace, numbers, dates ★"),
    ("Buraya çıkart", "Extract here"),
    ("Zip dosyasına sıkıştır", "Compress to ZIP file"),
    ("Çıkartılıyor: {}", "Extracting: {}"),
    ("Sıkıştırılıyor: {}", "Compressing: {}"),
    ("Çift bölme (Ctrl+Shift+S)", "Split view (Ctrl+Shift+S)"),
    ("Öbür bölmeye kopyala", "Copy to the other pane"),
    ("Öbür bölmeye taşı", "Move to the other pane"),
    ("Çift bölme: iki klasör yan yana ★", "Split view: two folders side by side ★"),
    ("Çift bölmede öbür bölmeye geç ★", "Switch to the other pane in split view ★"),
    ("Bu listeyi düzenle…", "Edit this list…"),
    ("Sağ tık menüsündeki uygulamalar", "Apps in the right-click menu"),
    ("İşaretli olanlar menüde görünür. Windows'un tam menüsü Shift + sağ tıkta her zaman açılır.", "Checked items appear in the menu. The full Windows menu always opens with Shift + right-click."),
    ("Paylaş", "Share"),
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_language_has_the_same_keys() {
        let keys: HashSet<&str> = TABLE.iter().map(|(k, _)| *k).collect();
        for (i, (code, _)) in LANGS.iter().enumerate().skip(2) {
            let map = &tables()[i];
            let lang: HashSet<&str> = map.keys().copied().collect();
            assert_eq!(lang, keys, "{code}");
        }
    }

    #[test]
    fn singular_only_for_a_lone_one() {
        set_lang(lang_index("en").unwrap());
        assert_eq!(singular("1 items selected".into()), "1 item selected");
        assert_eq!(singular("11 items".into()), "11 items");
        set_lang(lang_index("es").unwrap());
        assert_eq!(singular("1 elementos seleccionados".into()), "1 elemento seleccionado");
        set_lang(0);
    }

    #[test]
    fn locale_codes() {
        assert_eq!(lang_index("en_US.UTF-8"), Some(1));
        assert_eq!(lang_index("pt_BR.UTF-8"), Some(5));
        assert_eq!(lang_index("zh_CN.UTF-8"), Some(7));
        assert_eq!(lang_index("zh_TW.UTF-8"), None);
        assert_eq!(lang_index("C.UTF-8"), None);
        assert_eq!(lang_index("ko-KR"), Some(9));
        assert_eq!(lang_index("ko_KR.UTF-8"), Some(9));
    }
}
