# lohtty

A GPU-accelerated, xterm-compatible terminal for Windows (macOS and Linux
best-effort), written in Rust with [gpui](https://gpui.rs).

- Tabs and split panes, with the tabs in the title bar on Windows (like Windows
  Terminal). With a single tab, the title bar shows the window title, centered.
- Search (literal or regex, case toggle, highlighted matches)
- Themes, Tokyo Night by default, with live config reload
- Background opacity and blur
- Ligatures, plus Nerd Font icons through font fallbacks
- UTF-8, wide characters (CJK) and emoji
- Box-drawing, block and Powerline characters drawn to join across cells, whatever the font
- Mouse selection, clipboard, bracketed paste
- Mouse reporting for terminal apps
- OSC 8 hyperlinks
- Images through the kitty graphics protocol (PNG, RGB/RGBA; direct, file and
  temporary-file transfer)
- PowerShell by default, any shell configurable

## Install

**Installer.** Download `lohtty-<version>-windows-x64.msi` (or `-arm64.msi`)
from the releases page and run it. It installs for the current user only, in
`%LOCALAPPDATA%\Programs\lohtty`, without admin rights, and adds a Start menu
shortcut. Newer versions upgrade in place; uninstall from *Settings → Apps*.

**Portable build.** Download `lohtty-<version>-windows-x64.zip` (or
`-arm64.zip`), unzip it and run `lohtty.exe`.

**Releases aren't code-signed yet.** Windows SmartScreen shows *Windows
protected your PC* the first time you run the installer or the exe: click
**More info → Run anyway**. The installer shows the publisher as *Unknown*.
To check that a download is the published file, compare its hash with the
release's `SHA256SUMS`:

```powershell
Get-FileHash .\lohtty-<version>-windows-x64.msi -Algorithm SHA256
```

For the zip, you can also clear the download mark before extracting
(*Properties → Unblock*, or `Unblock-File .\lohtty-<version>-windows-x64.zip`).

**Linux.** Releases need glibc 2.35 or later (Ubuntu 22.04, Debian 12,
Fedora 36 and newer) and a Vulkan driver.

- **AppImage:** download `lohtty-<version>-linux-x86_64.AppImage` (or
  `-aarch64`), then `chmod +x` it and run it. It needs FUSE (most desktops
  have it); without it, run it with `--appimage-extract-and-run`.
- **Tarball:** `lohtty-<version>-linux-x86_64.tar.gz` holds the binary, a
  `.desktop` entry and the icon. It uses the system's `libxkbcommon` and
  `libxkbcommon-x11`, which the AppImage bundles. To add it to your menus:

  ```sh
  install -Dm755 lohtty ~/.local/bin/lohtty
  install -Dm644 lohtty.desktop ~/.local/share/applications/lohtty.desktop
  install -Dm644 lohtty.png ~/.local/share/icons/hicolor/256x256/apps/lohtty.png
  ```

Check a download with `sha256sum -c SHA256SUMS --ignore-missing`.

**macOS** 11 or later, Apple Silicon or Intel. Open
`lohtty-<version>-macos-universal.dmg` and drag *lohtty* to *Applications*.
The app isn't notarized, so macOS blocks the first launch: open *System
Settings → Privacy & Security* and click **Open Anyway** (on macOS 14 and
earlier, right-click the app and choose **Open**). Or clear the download mark
with `xattr -dr com.apple.quarantine /Applications/lohtty.app`. Check the
download with `shasum -a 256 -c SHA256SUMS --ignore-missing`.

**From source** (rustup installs the Rust version pinned in
`rust-toolchain.toml`):

```powershell
cargo build --release          # target\release\lohtty.exe
cargo build --release --no-default-features  # without kitty graphics
pwsh -File scripts/package.ps1 # dist\lohtty-<version>-windows-x64.zip
```

On Linux, install the development packages first (names for Debian and
Ubuntu), then package with `scripts/package-linux.sh [--appimage]`:

```sh
sudo apt install build-essential pkg-config libxkbcommon-dev libxkbcommon-x11-dev \
  libwayland-dev libvulkan-dev libx11-xcb-dev libxcb1-dev libfontconfig-dev libssl-dev
```

On macOS, building needs Xcode, not just the Command Line Tools (gpui compiles
its Metal shaders with it). `scripts/package-macos.sh` builds the universal
app and its disk image.

## Configuration

The config lives at `%APPDATA%\lohtty\config.toml` on Windows,
`~/Library/Application Support/lohtty/config.toml` on macOS and
`$XDG_CONFIG_HOME/lohtty/config.toml` (usually `~/.config/lohtty`) on Linux. Every key is optional. Changes apply
as soon as the file is saved, except `shell`, which only affects new tabs and
panes. If a config is invalid, lohtty keeps the previous settings and shows a
red error bar.

The **Settings** dialog edits the common options. Open it with the gear button
in the title bar or with Ctrl+, (Cmd+, on macOS). Saving updates
`config.toml` in place, so your comments and any other settings in the file
are kept.

```toml
theme = "tokyo-night"      # tokyo-night, tokyo-night-storm, tokyo-night-moon, tokyo-night-day
scrollback = 10000

[shell]
program = "pwsh.exe"       # default: pwsh.exe, else powershell.exe; $SHELL elsewhere
                           # (a login shell on macOS)
args = ["-NoLogo"]

[font]
family = "Cascadia Code"   # default; Menlo on macOS, the first installed common monospace font on Linux
size = 14.0
ligatures = true
# Fonts used for characters the main font lacks (e.g. Nerd Font icons).
# Default: whichever common Nerd Fonts are installed.
fallbacks = ["Symbols Nerd Font Mono"]

[window]
opacity = 0.92             # 1.0 = opaque
blur = true
padding = 8
width = 1000
height = 650

[graphics]
kitty = true               # kitty graphics protocol (new tabs and panes)
```

### Themes

A theme is a TOML file. Files in `%APPDATA%\lohtty\themes\<name>.toml` are
used by name and override bundled themes with the same name:

```toml
name = "My Theme"
foreground = "#c0caf5"
background = "#1a1b26"
cursor = "#c0caf5"
selection = "#283457"
ansi = [
  "#15161e", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#a9b1d6",
  "#414868", "#ff899d", "#9fe044", "#faba4a", "#8db0ff", "#c7a9ff", "#a4daff", "#c0caf5",
]
```

## Keyboard shortcuts

These are the Windows and Linux shortcuts. On macOS, Cmd replaces Ctrl+Shift
for copy, paste, search and tabs.

| Action | Shortcut |
|---|---|
| Copy / paste | Ctrl+Shift+C / Ctrl+Shift+V (also Ctrl+Insert / Shift+Insert) |
| Search | Ctrl+Shift+F |
| Next / previous match (in search) | Enter / Shift+Enter |
| Close search | Esc |
| New tab | Ctrl+Shift+T |
| Next / previous tab | Ctrl+Tab / Ctrl+Shift+Tab |
| Go to tab 1–9 | Ctrl+Alt+1 … 9 (9 = last tab) |
| Split right / split down | Alt+Shift+= / Alt+Shift+- |
| Move focus between panes | Alt+Arrow, or Ctrl+Alt+Arrow |
| Close pane | Ctrl+Shift+W |
| Settings | Ctrl+, |

Mouse:
- Drag to select. Double-click selects a word, triple-click a line.
- Right-click copies the selection, or pastes if nothing is selected.
- Ctrl+click opens a hyperlink.
- Middle-click on a tab closes it.
- Drag a divider to resize panes.
- Shift+drag selects text even when an app captures the mouse.

## Troubleshooting

- **Alt+Arrow doesn't move between panes.** A window manager such as GlazeWM
  may capture Alt+Arrow globally. Use Ctrl+Alt+Arrow instead.
- **No colors in PowerShell or prompts.** Check whether `NO_COLOR` is set in
  your environment. lohtty passes the environment through unchanged.
- **Prompt icons show as boxes.** Install a Nerd Font (for example
  *Symbols Nerd Font Mono*) or list one in `font.fallbacks`.
- **Italic text shows upright (Linux, macOS).** The font has no italic face:
  lohtty can't slant it there (Windows does). Use a font that ships italics,
  such as DejaVu Sans Mono with its Oblique files (`fonts-dejavu-extra` on
  Debian and Ubuntu), JetBrains Mono, Fira Code or Cascadia Code.
- **Images don't show.** Programs detect kitty graphics by sending a query,
  which lohtty answers unless `graphics.kitty = false`. Images need a Windows
  ConPTY that passes the protocol's escape sequences through (verified on
  Windows 11 build 26300). Animation isn't supported yet.
- **Images in tmux.** Programs such as `kitty icat` draw images through
  Unicode placeholders inside tmux. tmux must pass the graphics commands
  through and keep 24-bit colors (the image id travels in the text color):

  ```
  set -g allow-passthrough on
  set -as terminal-features ",xterm-256color:RGB"
  ```

  Programs that wait for the terminal's reply to decide (yazi) don't get it
  through tmux, and don't recognize lohtty by name, so they show no images
  there.
- **Black or missing window on Linux.** lohtty draws with Vulkan: check that
  a Vulkan driver is installed (`vulkaninfo --summary`, from `vulkan-tools`).
  Mesa's software renderer in Ubuntu 22.04 shows a black window under Xvfb;
  newer Mesa works.
- **Logs.** Set `LOHTTY_LOG=debug` (or `lohtty_term::pty=trace` for raw
  shell output) and start lohtty from another terminal. Logs go to stderr. By
  default they show lohtty's own messages and the libraries' warnings.

## License

Licensed under the [Apache License, Version 2.0](LICENSE).
