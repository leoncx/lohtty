# Changelog

All notable changes to lohtty. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

To release: move the entries under **Unreleased** into a new
`## [X.Y.Z] - YYYY-MM-DD` section, set the same version in `Cargo.toml`, and
push the tag `vX.Y.Z`. The release job refuses a tag without a matching
version and changelog section, and uses that section as the release notes.

## [Unreleased]

### Added

- macOS: the tab bar is the title bar, with the traffic lights on its left,
  as on Windows. A single tab shows as the window title, centered.
- New tabs and splits start in the active pane's directory. On macOS and
  Linux it works with any shell; on Windows the shell must report it (OSC 7
  or OSC 9;9, see the README).
- An About section at the bottom of Settings: version, release date and
  commit.

### Changed

- Tabs share the title bar equally (two take half each, three a third).

### Fixed

- macOS: Option+Left/Right move by word in the shell, and Option+Fn+Delete
  deletes the next word.
- Zooming the window or switching to full screen no longer leaves copies of
  the shell prompt behind: the shell gets the new size once it settles.

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
