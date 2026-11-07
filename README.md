# Swift Explorer

[![Release](https://img.shields.io/github/v/release/misutesu-desu/swift-explorer)](../../releases/latest)
[![Build](https://github.com/misutesu-desu/swift-explorer/actions/workflows/build.yml/badge.svg)](../../actions/workflows/build.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue)](LICENSE)

[Türkçe](README.tr.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja.md)

A file manager for Windows and Linux that looks and behaves like the Windows 11 File Explorer, written in Rust. Folders are read on background threads, so large ones open without freezing the window, and search runs across subfolders in parallel.

![Large icons, grouped by date](docs/large-icons.png)

## Download

Get `SwiftExplorer.exe` from the [Releases](../../releases) page. It is a single file with no installer. Made for Windows 11; Windows 10 should work but hasn't been tested.

With [Scoop](https://scoop.sh):

```powershell
scoop bucket add misutesu https://github.com/misutesu-desu/scoop-bucket
scoop install swift-explorer
```

On Linux, install it for your user with:

```bash
curl -fsSL https://raw.githubusercontent.com/misutesu-desu/swift-explorer/main/install.sh | sh
```

This puts `swift-explorer` in `~/.local/bin` and adds it to the app menu. To remove it, run the same command with `-s -- --uninstall` after `sh`. You can also download `swift-explorer-linux-x86_64.tar.gz` and run `swift-explorer` from it without installing.

Releases after 0.2.0 also include `swift-explorer.flatpak`, which installs with `flatpak install --user swift-explorer.flatpak`.

## What it does

- Same layout as Explorer: tabs in the title bar, address bar with breadcrumbs, command bar, navigation pane, Home and This PC pages.
- Uses Windows' own icons, thumbnails and file type names.
- Every Explorer shortcut works the same way. Press F1 for the full list.
- Ctrl+Shift+P opens a command palette: type part of a command's name, like `hidden` or `large icons`, and press Enter.
- Ctrl+Shift+S shows two folders side by side. F6 switches between them, and dragging between them copies or moves files.
- Pressing F2 with several items selected opens batch rename: find and replace, a name pattern with `{name}`, `{n}` and `{date}`, and upper or lower case, with a preview of every new name.
- Zip files can be extracted and created from the right-click menu.
- Search results can be narrowed to one file extension with the buttons above the list.
- The details pane lists what's inside a selected folder, and the status bar shows the git branch when the folder is in a git repository.
- Typing in the search box filters the current folder instantly. Press Enter to search all subfolders; `*` and `?` wildcards work.
- Typing `cmd`, `powershell`, `pwsh` or `wt` in the address bar opens that terminal in the current folder, and `terminal` opens Windows Terminal. On Linux, `terminal` opens your default terminal, and a terminal's name (`konsole`, `kitty`…) opens that one.
- Copy, cut, paste and drag and drop work in both directions with Explorer and the desktop.
- Shift + right-click opens the full Windows context menu, so 7-Zip, WinRAR and Send to are there.
- The right-click menu also lists the apps from the Windows menu, such as 7-Zip, WinRAR, Notepad++ and Send to. "Edit this list" at the bottom chooses which ones appear.
- Ctrl+Z undoes renames, moves, copies and deletes to the Recycle Bin.
- Right-clicking This PC opens the menu Explorer has there: Manage, Map network drive, Disconnect network drive and Properties.
- When a file can't be deleted or moved because another program has it open, the app shows which program it is. It can be closed from there, and the delete or move runs again once the program exits.
- Besides System, Light and Dark, View > Theme has Catppuccin, Gruvbox, Everforest, Nord, Dracula, Tokyo Night, Rosé Pine and Solarized.
- A copy or move counts as done only after the files are written to the disk, like running `sync` in a terminal. Once the progress bar is gone, a USB stick can be pulled out. When moving to another drive, the originals are deleted only after that.
- On Windows, closing the window keeps the app waiting in the background, so opening it again shows the window right away instead of waiting for the graphics driver to start. With View > Start in the background when signing in, the first launch after a reboot is quick too.
- On Windows the app updates itself: a new release is downloaded from GitHub, checked against its SHA-256 and takes over once the window is closed, or right away with View > Restart to switch to the new version. If the new version fails to start twice, the previous one comes back. View > Check for updates now checks without waiting for the hourly check. Scoop installs are left to `scoop update`. View > Install updates automatically turns this off.
- View > Use as the default file manager opens folders, drives and Win+E in Swift Explorer. On Linux it sets the app for `inode/directory`, and turning it off gives folders back to the previous app. The setting keeps working after updates. If the exe is moved, the next launch points Windows at the new location. Before deleting the app, turn the option off or run `SwiftExplorer.exe --unregister`.
- When a file with the same name already exists, it asks whether to replace, skip or keep both.
- Folders update live when files change on disk.
- Downloads is grouped by date modified (Today, Yesterday, Last week…), like in Explorer. Other folders can be grouped from Sort > Group by.
- Interface in English, Turkish, German, Spanish, French, Brazilian Portuguese, Russian, Simplified Chinese and Japanese. The language follows Windows and can be changed from View > Language.
- Chinese, Japanese and Korean file names display with the system's CJK fonts.

| Details | This PC |
| --- | --- |
| ![Details view](docs/details.png) | ![This PC](docs/this-pc.png) |

| Light theme | Turkish |
| --- | --- |
| ![Light theme](docs/light.png) | ![Turkish interface](docs/turkish.png) |

The Type column and the Shift + right-click menu come from Windows, so they stay in the Windows display language even when the app is set to English.

## Linux

The Linux version is new and less tested than the Windows one. It keeps the same look and shortcuts, with these differences:

- Icons come from your desktop's icon theme. Thumbnails are shown for PNG, JPEG, GIF, WebP and BMP.
- There is no Windows context menu, so Shift + right-click and Shift+F10 open the app's own menu.
- Copied files go to the clipboard as a list of paths. Pasting paths or `file://` links copied from other file managers works; pasting into other file managers may not.
- Dragging files out of the window to other apps doesn't work yet. Dropping files into it works on X11.
- "This PC" lists `/` and disks mounted under `/media`, `/run/media` and `/mnt`.
- Properties opens the app's own window with General, Security and Details tabs, where permissions and the default app can be changed.
- "Show in file manager" uses your file manager over D-Bus (Nautilus, Dolphin, Nemo and Thunar support it).

## Building

You need Rust 1.82 or newer.

```bash
cargo build --release
```

The executable ends up in `target/release/swift-explorer.exe` on Windows and `target/release/swift-explorer` on Linux.

## Translations

Each language is one file in [`src/i18n/`](src/i18n). To add one, copy `de.rs`, translate the second string on every line, then add a `mod` line and entries in `LANGS`, `tables()` and `system_lang()` in [`src/i18n.rs`](src/i18n.rs). The [German commit](../../commits/main/src/i18n/de.rs) shows every change needed. Fixes to the existing translations are welcome too; native speakers haven't checked them yet.

## Settings

Settings are stored in `%APPDATA%\SwiftExplorer\config.json` on Windows and `~/.config/SwiftExplorer/config.json` on Linux. Setting the `SWIFT_EXPLORER_LANG` environment variable to a language code (`en`, `tr`, `de`, `es`, `fr`, `pt`, `ru`, `zh`, `ja`) overrides the language setting.

## Known limitations

- In Details view, the selection rectangle can only be started from the empty area below the rows.
- Windows can't move files with very long paths (around 260+ characters) to the Recycle Bin, so they are deleted directly and Ctrl+Z can't bring them back.
- Network locations open in Windows File Explorer.

## License

[MIT](LICENSE). The Linux build includes a few icons from Microsoft's [Fluent UI System Icons](https://github.com/microsoft/fluentui-system-icons) ([MIT](assets/LICENSE-fluent-icons.txt)).
