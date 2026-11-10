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

- Terminal: xterm-compatible emulation (alacritty_terminal) over ConPTY,
  UTF-8, wide characters and emoji, 10,000 lines of scrollback.
- Tabs and split panes; tabs, split buttons and a settings button in the
  Windows title bar.
- Settings dialog (Ctrl+,) that edits `config.toml` in place.
- Search with regex and case options, highlighted matches.
- Themes (Tokyo Night by default) and live config reload.
- Background opacity and blur, ligatures, Nerd Font icon fallbacks.
- Mouse selection, clipboard, bracketed paste, mouse reporting, OSC 8 links.
- Kitty graphics protocol: PNG and raw images, anchored to scrollback.
- Box-drawing, block and Powerline characters drawn by lohtty, so lines,
  blocks and prompt segments join across cells with any font.
- Fast rendering of streaming output, and keystroke-to-screen latency on par
  with Windows Terminal.
- Releases for Windows (zip and per-user MSI, x64 and ARM64), Linux (tarball
  and AppImage, x86_64 and ARM64) and macOS (universal app in a disk image).
