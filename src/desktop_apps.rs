//! Linux: kurulu uygulamaların (`.desktop` dosyaları) listesi ve bir dosya türünün varsayılan
//! uygulamasının değiştirilmesi. Değiştirme `xdg-mime` ile yapılır (`mimeapps.list`'e yazar).

use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq)]
pub struct DesktopApp {
    /// `org.gnome.TextEditor.desktop` gibi
    pub id: String,
    pub name: String,
    pub mimes: Vec<String>,
    /// `Icon=` değeri: tema ikon adı ya da mutlak yol
    pub icon: Option<String>,
}

struct Entry {
    name: String,
    icon: Option<String>,
    no_display: bool,
    mimes: Vec<String>,
}

/// `LANG` değerinden aday yerel adlar: "de_DE.UTF-8" → ["de_DE", "de"]
fn locales() -> Vec<String> {
    let lang = ["LC_ALL", "LC_MESSAGES", "LANG"].iter().find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty())).unwrap_or_default();
    let base = lang.split(['.', '@']).next().unwrap_or("").to_string();
    let mut v = Vec::new();
    if !base.is_empty() && base != "C" && base != "POSIX" {
        if let Some((l, _)) = base.split_once('_') {
            v.push(base.clone());
            v.push(l.to_string());
        } else {
            v.push(base);
        }
    }
    v
}

/// `[Desktop Entry]` bölümünü okur; uygulama değilse ya da gizliyse `None` döner
fn parse_entry(text: &str, locales: &[String]) -> Option<Entry> {
    let (mut name, mut localized, mut app, mut hidden) = (None, None, false, false);
    let (mut no_display, mut mimes, mut icon) = (false, Vec::new(), None);
    let mut in_section = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_section = line == "[Desktop Entry]";
            continue;
        }
        if !in_section {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        match k.trim() {
            "Type" => app = v.trim() == "Application",
            "Name" => name = Some(v.trim().to_string()),
            "Icon" => icon = Some(v.trim().to_string()).filter(|i| !i.is_empty()),
            "NoDisplay" => no_display = v.trim() == "true",
            "Hidden" => hidden = v.trim() == "true",
            "MimeType" => mimes = v.split(';').filter(|m| !m.is_empty()).map(String::from).collect(),
            k => {
                if let Some(loc) = k.strip_prefix("Name[").and_then(|r| r.strip_suffix(']')) {
                    // Daha önce eşleşen (daha belirgin) yerel ad korunur
                    if let Some(rank) = locales.iter().position(|l| l == loc) {
                        if localized.as_ref().is_none_or(|(r, _): &(usize, String)| rank < *r) {
                            localized = Some((rank, v.trim().to_string()));
                        }
                    }
                }
            }
        }
    }
    if !app || hidden {
        return None;
    }
    Some(Entry { name: localized.map(|(_, n)| n).or(name)?, icon, no_display, mimes })
}

fn data_dirs() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = dirs::data_dir().into_iter().collect();
    let system = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    v.extend(system.split(':').filter(|d| !d.is_empty()).map(PathBuf::from));
    // Flatpak uygulamaları buraya dışa aktarılır
    v.push(PathBuf::from("/var/lib/flatpak/exports/share"));
    if let Some(d) = dirs::data_dir() {
        v.push(d.join("flatpak/exports/share"));
    }
    let mut seen = Vec::new();
    v.retain(|d| {
        let new = !seen.contains(d);
        seen.push(d.clone());
        new
    });
    v
}

/// Görünür tüm uygulamalar, ada göre sıralı. Aynı kimlik birden çok yerdeyse ilk bulunan geçerlidir.
pub fn installed_apps() -> Vec<DesktopApp> {
    let locales = locales();
    let mut apps: Vec<DesktopApp> = Vec::new();
    for dir in data_dirs() {
        let Ok(rd) = std::fs::read_dir(dir.join("applications")) else { continue };
        for de in rd.flatten() {
            let id = de.file_name().to_string_lossy().into_owned();
            if !id.ends_with(".desktop") || apps.iter().any(|a| a.id == id) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(de.path()) else { continue };
            if let Some(e) = parse_entry(&text, &locales) {
                if !e.no_display {
                    apps.push(DesktopApp { id, name: e.name, mimes: e.mimes, icon: e.icon });
                }
            }
        }
    }
    apps.sort_by_key(|a| a.name.to_lowercase());
    apps
}

/// Bir uygulamanın görünen adı (NoDisplay olanlar dahil)
pub fn app_name(id: &str) -> Option<String> {
    let locales = locales();
    data_dirs().into_iter().find_map(|d| std::fs::read_to_string(d.join("applications").join(id)).ok()).and_then(|t| parse_entry(&t, &locales)).map(|e| e.name)
}

/// `xdg-mime default <kimlik> <tür>`: dosya türünün varsayılan uygulamasını ayarlar
pub fn set_default(id: &str, mime: &str) -> bool {
    crate::win::host_command("xdg-mime").args(["default", id, mime]).status().is_ok_and(|s| s.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENTRY: &str = "[Desktop Entry]\nType=Application\nName=Text Editor\nName[de]=Texteditor\nName[de_DE]=Texteditor (DE)\nMimeType=text/plain;text/x-log;\nExec=gedit %U\nIcon=org.gnome.gedit\n\n[Desktop Action new]\nName=New\n";

    #[test]
    fn parses_name_and_mimes() {
        let e = parse_entry(ENTRY, &[]).unwrap();
        assert_eq!(e.name, "Text Editor");
        assert_eq!(e.mimes, ["text/plain", "text/x-log"]);
        assert!(!e.no_display);
        assert_eq!(e.icon.as_deref(), Some("org.gnome.gedit"));
    }

    #[test]
    fn prefers_the_most_specific_locale() {
        let e = parse_entry(ENTRY, &["de_DE".into(), "de".into()]).unwrap();
        assert_eq!(e.name, "Texteditor (DE)");
        let e = parse_entry(ENTRY, &["de".into()]).unwrap();
        assert_eq!(e.name, "Texteditor");
        let e = parse_entry(ENTRY, &["fr".into()]).unwrap();
        assert_eq!(e.name, "Text Editor");
    }

    #[test]
    fn skips_non_apps_and_hidden_entries() {
        assert!(parse_entry("[Desktop Entry]\nType=Link\nName=X\n", &[]).is_none());
        assert!(parse_entry("[Desktop Entry]\nType=Application\nName=X\nHidden=true\n", &[]).is_none());
        assert!(parse_entry("[Desktop Entry]\nType=Application\nName=X\nNoDisplay=true\n", &[]).unwrap().no_display);
    }
}
