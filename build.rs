fn main() {
    // Windows'ta exe dosyasına uygulama ikonunu göm
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        // MSVC bağlayıcısı uygulama bildirimini (comctl32 v6 dahil) kendisi ekler; MinGW'de eklemez.
        // Bildirim olmazsa SetWindowSubclass gibi işlevler bulunamaz ve uygulama açılmaz.
        if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu") {
            res.set_manifest_file("packaging/windows/swift-explorer.manifest");
            println!("cargo:rerun-if-changed=packaging/windows/swift-explorer.manifest");
        }
        res.compile().expect("ikon kaynağı derlenemedi");
    }
}
