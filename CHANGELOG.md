# Changelog

All notable changes to lohtty. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

To release: move the entries under **Unreleased** into a new
`## [X.Y.Z] - YYYY-MM-DD` section, set the same version in `Cargo.toml`, and
push the tag `vX.Y.Z`. The release job refuses a tag without a matching
version and changelog section, and uses that section as the release notes.

## [Unreleased]

## [0.4.0] - 2026-11-19

### Added

- While the shell is at its prompt, the tab and window title show its
  directory (`~` for home) instead of the shell's name; a running program
  still shows its own title. Windows needs the shell to report it (OSC 7 or
  OSC 9;9, see the README).
- Kitty graphics animations: programs can send an image's frames and let
  lohtty play them (`kitty icat some.gif` animates), or switch frames
  themselves. Frames can be built from earlier ones and composed onto each
  other. An animation runs only while it is on screen.
- Shell integration (OSC 133): a green or red bar beside each prompt whose
  command succeeded or failed, Ctrl+Shift+Up/Down to jump between prompts,
  and Ctrl+Shift+O to select and copy the last command's output (Cmd on
  macOS). The README shows how to turn the marks on in PowerShell and bash.
- Font zoom per pane: Ctrl+= / Ctrl+- / Ctrl+0 (Cmd on macOS), or
  Ctrl+mouse wheel, a point at a time between 6 and 72.
- Panes without the focus show their text faded, so the active one stands
  out. Turn it off in Settings or with `window.dim_unfocused = false`.
- Alt+drag (Option+drag on macOS) selects a rectangle.
- `copy_on_select` (also in Settings) copies mouse selections to the
  clipboard as soon as the button is released. On Linux, selections also
  go to the primary selection, which a middle click pastes.
- `LOHTTY_CONFIG` names another config file to use.
- A command palette (Ctrl+Shift+P, Cmd+Shift+P on macOS) lists every
  action with its shortcut; type to filter, Enter to run.
- Record a pane to an asciinema file from the command palette: asciicast
  v3 (or v2), starting from the current screen, with prompts as markers,
  the theme, resizes and the exit code. Saved in Videos/lohtty by default
  (`[recording]` section).

### Fixed

- Windows: kitty graphics images (and animations) show again. Recent
  Windows updates make the system's ConPTY drop them, so lohtty now ships
  Microsoft's ConPTY (`conpty.dll` and `OpenConsole.exe`, MIT) next to
  `lohtty.exe`, in the zip and the installer.
- Search highlights every match on screen again when nothing has scrolled
  off the top yet (a new tab, or after `clear`); only the current match
  was highlighted.

## [0.3.0] - 2026-11-13

### Added

- macOS: the tab bar is the title bar, with the traffic lights on its left,
  as on Windows. A single tab shows as the window title, centered.
- New tabs and splits start in the active pane's directory. On macOS and
  Linux it works with any shell; on Windows the shell must report it (OSC 7
  or OSC 9;9, see the README).
- An About section at the bottom of Settings: version, release date and
  commit.
- Linux: lohtty draws its own title bar, as on Windows: the tabs, with
  minimize, maximize and close buttons, and a single tab's title centered.
  It drags the window (double-click maximizes, right-click opens the window
  menu), and the window resizes from its edges. GNOME on Wayland showed no
  title bar or buttons at all.
- macOS: desktop notifications when a program in a pane asks for attention
  (Claude Code, Codex, ...) and lohtty isn't in front. Clicking one brings you
  to that pane; the Dock icon bounces and shows how many panes are waiting.

### Changed

- Tabs share the title bar equally (two take half each, three a third).

### Fixed

- macOS: Option+Left/Right move by word in the shell, and Option+Fn+Delete
  deletes the next word.
- Zooming the window or switching to full screen no longer leaves copies of
  the shell prompt behind: the shell gets the new size once it settles.
- Text no longer drifts off the cell grid, overlapping colored backgrounds,
  on a row with a colored underline (SGR 58), a combining mark or an emoji
  variation selector (such as ⚠️).

## [0.2.0] - 2026-11-12

### Added

- Usage meter: Claude's and Codex's session (5-hour) and weekly limits as a
  donut per agent on the right of the title bar, with a tooltip giving each
  window's use and the time left until it resets. Claude's come from Claude
  Code's sign-in, Codex's from its session logs. `[usage]` in `config.toml`,
  and toggles in Settings.
- Pane zoom: Ctrl+Shift+Z (macOS: Cmd+Shift+Enter) or the title-bar button
  shows the active pane alone, filling the tab, and toggles back to the
  splits. The other panes keep running.

### Fixed

- The single tab's centered title no longer runs under the title-bar buttons
  in narrow windows.

## [0.1.0] - 2026-11-10

### Added

- Terminal: xterm-compatible emulation (alacritty_terminal) over ConPTY,
  UTF-8, wide characters and emoji, 10,000 lines of scrollback.
- Tabs and split panes; tabs, split buttons and a settings button in the
  Windows title bar.
- Settings dialog (Ctrl+,) that edits `config.toml` in place.
- Search with regex and case options, highlighted matches.
- Themes (Tokyo Night by default) and live config reload.
- Background opacity and blur, ligatures, Nerd Font icon fallbacks.
- Mouse selection, clipboard, bracketed paste, mouse reporting, OSC 8 links.
- Progress (OSC 9;4, as in Windows Terminal) on the tab, and on Windows on
  the taskbar button, which also reflects agents' status.
- Session status for coding agents and other programs (Program Status
  Protocol, OSC 7501): working, blocked, done or failed, shown on the tab.
- Shift+Enter in coding agents and other programs: the kitty keyboard
  protocol (disambiguate level) and, on Windows, ConPTY's win32 input mode
  for modified Enter.
- Attention alerts: a program that rings the bell or sends a notification
  (OSC 9, 777, 99), such as a coding agent that finished or waits for input,
  marks its tab; on Windows, when lohtty isn't in front, the taskbar button
  flashes and a desktop notification opens that pane when clicked.
- Kitty graphics protocol: PNG and raw images, anchored to scrollback. Works
  with `kitty icat`, timg, chafa and yazi. Images can be deleted by id,
  number, z-index, id range, cell, cursor, column or row. Unicode placeholders
  (`U=1`) show images through text, so `kitty icat` works inside tmux. Image
  data can come inline, from a file or temporary file, or from shared
  memory.
- Box-drawing, block and Powerline characters drawn by lohtty, so lines,
  blocks and prompt segments join across cells with any font.
- Fast rendering of streaming output, and keystroke-to-screen latency on par
  with Windows Terminal.
- Releases for Windows (zip and per-user MSI, x64 and ARM64), Linux (tarball
  and AppImage, x86_64) and macOS (universal app in a disk image).
