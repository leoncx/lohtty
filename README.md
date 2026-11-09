# lohtty

A GPU-accelerated, xterm-compatible terminal for Windows (macOS and Linux
best-effort), written in Rust with [gpui](https://gpui.rs).

- Tabs and split panes
- Search (literal or regex, case toggle, highlighted matches)
- Themes, Tokyo Night by default, with live config reload
- Background opacity and blur
- Ligatures, plus Nerd Font icons through font fallbacks
- UTF-8, wide characters (CJK) and emoji
- Mouse selection, clipboard, bracketed paste
- Mouse reporting for terminal apps
- OSC 8 hyperlinks
- PowerShell by default, any shell configurable

## Install

**Portable build.** Download `lohtty-<version>-windows-x64.zip` from the
releases page, unzip it and run `lohtty.exe`.

**From source** (Rust stable):

```powershell
cargo build --release          # target\release\lohtty.exe
pwsh -File scripts/package.ps1 # dist\lohtty-<version>-windows-x64.zip
```

## Configuration

The config lives at `%APPDATA%\lohtty\config.toml` (on Linux and macOS,
`$XDG_CONFIG_HOME/lohtty/config.toml`). Every key is optional. Changes apply
as soon as the file is saved, except `shell`, which only affects new tabs and
panes. If a config is invalid, lohtty keeps the previous settings and shows a
red error bar.

```toml
theme = "tokyo-night"      # tokyo-night, tokyo-night-storm, tokyo-night-moon, tokyo-night-day
scrollback = 10000

[shell]
program = "pwsh.exe"       # default: pwsh.exe, else powershell.exe ($SHELL elsewhere)
args = ["-NoLogo"]

[font]
family = "Cascadia Code"
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
- **Logs.** Set `LOHTTY_LOG=debug` (or `lohtty_term::pty=trace` for raw
  shell output) and start lohtty from another terminal. Logs go to stderr.
