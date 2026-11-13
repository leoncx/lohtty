# lohtty — Architecture & Plan

A GPU-accelerated, xterm-compatible terminal emulator written in Rust with **gpui**.
Windows is the primary target. macOS and Linux are supported on a best-effort basis.

---

## 1. Key technical decisions

| Concern | Choice | Why |
|---|---|---|
| UI / rendering | `gpui` | Required by spec. GPU text rendering, font shaping with OpenType features, and window transparency. |
| VT parsing + grid state | `alacritty_terminal` | Mature xterm emulation: grid, scrollback, selection, regex search, title and clipboard events. Zed's terminal uses the same gpui + alacritty_terminal pairing. |
| PTY | `portable-pty` (ConPTY on Windows, openpty on Unix) | Lohtty owns the read loop, so the `OutputFilter` hook can sit between the PTY and the parser. |
| Kitty graphics *(optional, last)* | Own module that intercepts APC `ESC _ G … ESC \` before alacritty sees it. Behind the cargo feature `kitty-graphics` (off by default) and the runtime config flag `graphics.kitty`. | alacritty_terminal ignores APC sequences (see Risks §6). Keeping it optional means the core terminal never depends on it. |
| Config | TOML (`serde` + `toml`), hot reload with `notify` | Simple, human-editable. |
| Themes | TOML theme files, Tokyo Night bundled through `include_str!` | Users can add themes without rebuilding. |
| Logging | `tracing` + `tracing-subscriber` | |
| Errors | `anyhow` in the app, `thiserror` in library crates | |

The terminal engine sits behind a thin `TerminalBackend` boundary in `lohtty-term`. If kitty graphics or another feature outgrows alacritty_terminal, the engine can be replaced (for example with a custom `vte`-based grid) without touching the UI.

**Optional output filter.** `lohtty-term` exposes a generic hook, `trait OutputFilter { fn filter(&mut self, input: &[u8], out: &mut Vec<u8>, pty_reply: &mut Vec<u8>); }`, that runs between the PTY and the `Term`. It is a pass-through by default. Kitty graphics plugs into this hook, so milestones M0–M6 need no graphics-specific code.

---

## 2. Workspace layout

```
lohtty/
├── Cargo.toml                 # [workspace]
├── SPEC.md
├── PLAN.md
├── assets/
│   └── icons/lohtty.ico
├── crates/
│   ├── lohtty/                # binary: main.rs, app bootstrap, actions, keymap
│   ├── lohtty-config/         # config schema, loading, defaults, hot reload
│   ├── lohtty-theme/          # Theme struct, TOML loader; themes/*.toml bundled (Tokyo Night + storm, moon, day)
│   ├── lohtty-term/           # Terminal session: PTY, alacritty Term, events, search
│   ├── lohtty-graphics/       # OPTIONAL (feature `kitty-graphics`): parser, image store, placements
│   ├── lohtty-ui/             # gpui views: Workspace, TabBar, PaneGroup, TerminalView, SearchBar
│   └── lohtty-usage/          # coding agents' usage limits (Claude, Codex); no gpui
└── tests/                     # vttest-style integration / snapshot tests
```

### Crate responsibilities

**`lohtty-config`**
- `Config { shell, font, theme, window, keybindings, scrollback, ... }`
- Location: `%APPDATA%\lohtty\config.toml` on Windows, `$XDG_CONFIG_HOME/lohtty/config.toml` elsewhere (via the `dirs` crate).
- The `notify` watcher emits `ConfigChanged`, which the UI applies live (theme, font, opacity).

**`lohtty-theme`**
- `Theme { name, foreground, background, cursor, selection, ansi: [Rgba; 16], ... }`
- Resolves `alacritty_terminal::vte::ansi::Color` (Named / Indexed / Spec) to `gpui::Hsla`.
- Default: `tokyo-night`.

**`lohtty-term`** (no gpui dependency, so it can be unit tested)
- `Session::spawn(ShellConfig, size) -> Session`
- Holds `Arc<FairMutex<Term<EventProxy>>>`, the PTY event loop, and an optional `Box<dyn OutputFilter>`.
- `EventProxy` forwards `Event::{Title, ResetTitle, Bell, ClipboardStore, PtyWrite, Exit, Wakeup}` over a channel.
- Shell resolution: config value, else `pwsh.exe`, else `powershell.exe` on Windows; `$SHELL` elsewhere.
- Input encoding: keys → xterm sequences, honoring app-cursor mode, bracketed paste, and SGR mouse modes.
- Search: wraps `RegexSearch` and returns match ranges for highlighting.

**`lohtty-graphics`** *(optional, implemented last)*
- Compiled only with `--features kitty-graphics`. The `lohtty` binary declares it as an optional dependency. Every UI hook (image painting in `TerminalView`) sits behind `#[cfg(feature = "kitty-graphics")]`.
- Even when compiled in, it activates only if `graphics.kitty = true` in the config. When it is inactive, the APC is dropped as it is today and the protocol query (`a=q`) gets no reply, so applications fall back to text rendering.
- Implements `OutputFilter`: passes everything through except APC `G` commands, which it parses.
- Supports the actions `a=t,T,p,d,q`, formats `f=24,32,100` (PNG through the `image` crate), transmission `t=d` (direct) first, then `t=f,t` (file, temp file), and chunking (`m=1`).
- `ImageStore` (id → decoded RGBA, with a memory cap) and `Placement` anchored to an absolute grid line so placements scroll with text.
- Responses (`OK` / errors) go back to the PTY. After a placement, the filter injects the cursor movement that the spec requires.

**`lohtty-usage`** (no gpui dependency)
- `UsageSnapshot { provider, plan, windows: Vec<UsageWindow>, observed_at }`; a `UsageWindow` is a session (5 h), weekly or other window with its percent used and reset time.
- One module per provider turns the agent's data into snapshots; `humanize_remaining` / `humanize_ago` and `Level` (70 % / 90 % thresholds) serve the title-bar meter.

**`lohtty-ui`**
- `Workspace` (root view): owns the `TabBar` and `Vec<Tab>`.
- `Tab`: owns a `PaneGroup` (split tree) and the active pane id. The title comes from the active pane.
- `PaneGroup`: `enum Node { Leaf(Entity<TerminalView>), Split { axis, children, ratios } }`, with draggable dividers.
- `TerminalView`: the gpui `Element` that paints the grid. Responsibilities:
  - Batches contiguous cells with the same style into text runs, so ligatures shape across cells (`calt` and `liga` enabled through `FontFeatures`). The font is monospace, and runs are snapped to cell positions.
  - Paints background rects, cursor (block/beam/underline, blinking), selection, search highlights, and kitty images.
  - Handles key, mouse, scroll, IME, and paste input.
- `SearchBar`: overlay with an input, next/prev, case and regex toggles, and a match counter.
- Window: `WindowBackgroundAppearance::Transparent` or `Blurred`. The theme background is painted with config `opacity` alpha. On Windows, Mica/Acrylic can be added later.

---

## 3. Data flow

```
 keyboard/mouse ──► TerminalView ──encode──► Session.write() ──► PTY (ConPTY) ──► shell
                                                                       │
 shell output ◄────────────────────────────────────────────────────────┘
      │
      ▼
 OutputFilter (pass-through, or kitty graphics if enabled) ──(APC G)──► ImageStore
      │ (everything else)
      ▼
 alacritty Term (grid, modes, scrollback) ──Event::Wakeup/Title/Bell──► channel
                                                                          │
 gpui foreground task ◄───────────────────────────────────────────────────┘
      └─► cx.notify() ──► TerminalView::paint() reads a locked snapshot of the grid
```

Rendering is coalesced: several `Wakeup` events received within one frame cause a single repaint.

---

## 4. Feature → implementation map

| Spec item | Where | Notes |
|---|---|---|
| Multiple tabs | `lohtty-ui::Workspace/TabBar` | Ctrl+Shift+T new, Ctrl+Shift+W close, Ctrl+Tab cycle, drag reorder (later) |
| Split H/V | `lohtty-ui::PaneGroup` | Alt+Shift+- / Alt+Shift+=, Alt+arrows to focus, resize by drag or keys |
| Search text | `lohtty-term` (RegexSearch) + `SearchBar` | Ctrl+Shift+F, searches scrollback |
| UTF-8 | alacritty_terminal + gpui shaping | Wide chars (CJK, emoji) occupy two cells; combining marks are handled with zero-width storage |
| Theme (Tokyo Night default) | `lohtty-theme` + config | Hot reload |
| Background opacity | Window appearance + painted alpha | `window.opacity = 0.0..1.0` |
| Ligature fonts | `TerminalView` run batching + `FontFeatures` | `font.ligatures = true`; disabled under the cursor so editing stays readable |
| Kitty graphics *(optional)* | `lohtty-graphics` | Last milestone. Cargo feature `kitty-graphics` plus config `graphics.kitty`. See Risks |
| Choose shell (PowerShell default) | `lohtty-config` + `Session::spawn` | `shell.program`, `shell.args`, plus named profiles later |
| Title change | OSC 0/2 → `Event::Title` → Tab title + window title | Also handles `ResetTitle` and the title stack |
| XTerm compatible | alacritty_terminal | `TERM=xterm-256color`, `COLORTERM=truecolor`; validate with vttest and esctest |
| Multi-OS | gpui + alacritty tty | CI matrix: windows-latest (primary), macos, ubuntu |

### Example `config.toml`

```toml
[shell]
program = "pwsh.exe"
args = ["-NoLogo"]

[font]
family = "JetBrains Mono"
size = 14.0
ligatures = true

[window]
opacity = 0.92
blur = true
padding = 8

theme = "tokyo-night"
scrollback = 10000

# Only used when built with --features kitty-graphics
[graphics]
kitty = false
max_memory_mb = 256
```

---

## 5. Milestones

Each milestone ends with a runnable build.

**M0 — Skeleton** *(foundation)* — **done**
- Cargo workspace, CI (fmt, clippy, test on Windows, macOS, Linux), `tracing`. Since 2026-11-13 CI runs by hand only (`workflow_dispatch`, owner's choice: Actions minutes); releases still run on `v*` tags.
- gpui window that opens, closes, and draws a colored background.

**M1 — Single working terminal** *(the critical path)* — **done** (htop in WSL and `git log` still to be checked by hand)
- `Session` spawns PowerShell through ConPTY, and output appears in the grid.
- `TerminalView` paints cells with colors, attributes (bold, italic, underline, inverse), and the cursor.
- Keyboard input encoding, resize propagated to the PTY, scrollback with mouse wheel.
- Tokyo Night hardcoded.
- ✅ Done when `pwsh`, `vim` or `nvim`, `htop` in WSL, and `git log` behave correctly.

*Implementation notes from M1:*
- The PTY uses `portable-pty`, not `alacritty_terminal::tty`. Alacritty's event loop feeds its own parser, which leaves no place for the `OutputFilter` hook. Lohtty runs its own reader, writer and waiter threads, and the writer thread means PTY replies never block the parser.
- Text input goes through gpui's `EntityInputHandler` (WM_CHAR and IME). `key_down` only encodes control, alt and named keys. Dead keys on Windows need special handling: see `TerminalView::pending_dead_key`.
- Open TODOs: the synchronized-update (mode 2026) timeout, mouse reporting (moved to M2), and drawing the IME composition text (M4).

**M2 — Config, theme, shell selection, title** — **done**
- `lohtty-config` with defaults and hot reload, `lohtty-theme` loader, bundled Tokyo Night variants.
- Shell selection from config.
- OSC title → window title.
- Selection, copy/paste (Ctrl+Shift+C/V), and bracketed paste.
- Mouse reporting (SGR 1006 and normal modes) for TUI apps.

*Implementation notes from M2:*
- The watcher covers the whole config directory (`config.toml` and `themes/*.toml`), with a 100 ms debounce. An invalid config or unknown theme keeps the previous settings and shows a red error bar until the next successful reload. Font changes apply live; shell changes affect new sessions only.
- User themes live in `<config dir>/themes/<name>.toml` and override bundled themes of the same name.
- Copy/Paste are gpui actions bound in the `Terminal` key context. gpui matches bindings before `on_key_down`, which M3's tab and split shortcuts will rely on too. The bindings are Ctrl+Shift+C/V and Ctrl/Shift+Insert, plus Cmd+C/V on macOS. Right click copies the selection, or pastes when nothing is selected (as in Windows Terminal).
- Selection supports drag, double-click for a word and triple-click for a line. Shift forces local selection while an app has mouse reporting enabled. Focus in/out reporting (mode 1004) is supported.
- Open TODO: auto-scroll while drag-selecting past the top or bottom edge.

**M3 — Tabs & splits** — **done** (the window-close setting is not configurable yet)
- Workspace, TabBar, and PaneGroup split tree with focus management and divider dragging.
- Per-pane title, with the tab showing the active pane's title.
- Closing the last pane closes the tab, and closing the last tab closes the window (configurable).
- **Pane zoom** (added 2026-11-12): `TogglePaneZoom` (Ctrl+Shift+Z like WezTerm; macOS Cmd+Shift+Enter like iTerm2) and a title-bar button left of the split buttons, shown only when the active tab has splits (outline icon; filled and accent-colored while zoomed). `Tab::zoomed` renders the active pane alone as a leaf; the others keep running unrendered and get their size back when shown. Zoom ends when splitting (the new pane must show), moving focus by direction (as tmux), closing the zoomed pane, or a notification bringing another pane forward; a hidden pane exiting keeps it. The centered title's room counts the extra button. Live (482 px window): split → zoom → restore → zoom → Ctrl+Alt+Left left the zoom and focused the left pane; `$Host.UI.RawUI.WindowSize.Width` read 26 columns split, 56 zoomed, 26 restored.

*Implementation notes from M3:*
- `pane_tree.rs` is a pure, generic split tree with unit tests. Splitting in the same direction adds a sibling instead of nesting. Removing a pane redistributes its space proportionally, and a split left with one child collapses. Focus by direction uses the actual layout geometry.
- `TerminalView` no longer touches the window. It emits `TerminalEvent::{TitleChanged, Focused, Exited}`, and the workspace owns the window title, the active pane and the closing of panes.
- Bindings on Windows/Linux:

  | Action | Keys |
  |---|---|
  | New tab | Ctrl+Shift+T |
  | Close pane | Ctrl+Shift+W (closes the tab when it is the last pane) |
  | Split right | Alt+Shift+= |
  | Split down | Alt+Shift+- |
  | Focus pane | Alt+Arrow, or Ctrl+Alt+Arrow |
  | Next / previous tab | Ctrl+Tab / Ctrl+Shift+Tab |
  | Go to tab N | Ctrl+Alt+1–9 (9 is the last tab) |

  macOS uses Cmd variants. In the tab bar, middle-click or × closes a tab and + opens one.
- gpui on Windows folds Shift into the character: Alt+Shift+= arrives as `alt-+`, so both forms are bound.
- Ctrl+Alt+Arrow exists because window managers such as GlazeWM capture Alt+Arrow globally. User-configurable keybindings should come in a later milestone.
- Open TODOs:
  - Keyboard pane resizing.
  - Tab reordering by drag.
- **New panes start in the active pane's directory** (added 2026-11-13; `lohtty_term::cwd`): new tabs and splits pass the active pane's directory as `ShellSpec::cwd` (the first tab, and a directory that no longer exists, fall back to home). Two sources: what the shell reports (OSC 7 `file://host/path`, percent-decoded, ignored from another host such as through ssh; or ConEmu's OSC 9;9), and on macOS and Linux the shell process's own directory (`proc_pidinfo` `PROC_PIDVNODEPATHINFO`; `/proc/<pid>/cwd`). The reported path wins when it names the same directory (it keeps symbolic links), so a stale report loses; on Windows only the reported one exists, so PowerShell needs a prompt that sends OSC 9;9 or OSC 7. Tested: parsing, `pick`, reading the process's directory, and a shell started in a directory whose `cd` the session follows. Live on the Mac (zsh without OSC 7): `cd /usr/local/share` then a new tab and a split both start there; after `cd /tmp` a new tab starts in `/private/tmp` (the process's path). libc is now a plain Unix dependency, not only for kitty graphics.
- **Tabs in the title bar** (Windows; `lohtty_ui::CUSTOM_TITLEBAR`):
  - The window uses `TitlebarOptions { appears_transparent: true }`. The tab bar's empty space is a `WindowControlArea::Drag`, which lets Windows handle dragging, double-click to maximize, the right-click system menu and Aero Snap.
  - The minimize, maximize/restore and close buttons are drawn with Segoe Fluent Icons (falling back to Segoe MDL2 Assets). They are mapped to `Min`, `Max` and `Close`, so Windows handles the clicks and shows the Windows 11 snap-layout flyout on maximize.
  - gpui checks control areas before the top resize border, so the drag area starts 4 px below the top edge to keep resizing from the top.
  - Tested: maximize, restore, close, the red close hover, and dimmed glyphs when the window is inactive. Double-click to maximize is blocked on the dev machine by GlazeWM, for the native caption too.
  - Linux keeps the native title bar.
  - **macOS** (added 2026-11-13): the tab bar is the title bar too (`appears_transparent`), with the native traffic lights at `TRAFFIC_LIGHT_POSITION` (12, 10), centered on the 34 px bar, and 78 px left free for them (none in full screen, where they hide). No caption buttons are drawn (`CAPTION_BUTTONS` is Windows only). gpui 0.2.2's macOS backend ignores `WindowControlArea` and has no `start_window_move`: AppKit moves the window from the whole bar, and a left double-click on the empty bar calls `titlebar_double_click` (follows the system's zoom/minimize setting). The empty space lies under the centered title too, so the title has no handler of its own: with one, a double-click there zoomed and unzoomed at once. Live on the owner's Mac (2026-11-13, synthetic CGEvents): the single tab shows the centered title and Cmd+T brings the strip back, clear of the lights; the lights and + line up on the bar; dragging moves the window from empty space at 8, 20 and 31 px down and from the title; double-click zooms and restores, on empty space and on the title (4 of 4 after the fix); full screen leaves no room for the lights. Seen meanwhile: an animated zoom or full-screen switch left many stale copies of a zsh right prompt, one per intermediate size, as the PTY was resized on every frame. The grid still resizes at once, but a `pty-resize` thread now sends the PTY the first size at once (the shell starts at a guessed 80×24) and later ones after 100 ms without change (`RESIZE_SETTLE`, kitty's default debounce); test `resize_burst_reaches_the_pty_settled` (Unix). Live: three zooms, full screen and back leave one clean prompt.
  - **Tab widths** (added 2026-11-13): with two or more tabs they share the strip equally (`flex_1` on a zero basis: two take half each, three a third), down to 90 px, then the strip clips. A 32 px gap (`DRAG_GAP`) stays between `+` and the buttons on the right, to drag the window by; it was the flex-grown empty space before. Live on the Mac: 1, 2 and 3 tabs; the gap drags, and a double-click there zooms and restores.
  - **Single tab:** the tab strip is replaced by the tab's title, centered on the whole bar (an absolutely positioned layer under the other controls, truncated at half the bar's width). The `+` button stays on the left; the strip comes back with a second tab. Only the title text takes the mouse, as a `Drag` area. Checked on Windows: one tab, Ctrl+Shift+T (strip), Ctrl+Shift+W (title again). gpui answers `WM_NCHITTEST` from the hitbox under the last mouse position, not the message's point, so the check hovers the point first: the title and the empty bar are `HTCAPTION`, the `+` button is client area.
- **Split buttons:** "split right" and "split down" sit in the tab bar, left of the window buttons (on every platform). They act on the active pane and show their shortcut in a tooltip. The icons are drawn with divs in the theme colors.
- **Settings dialog:** opened with the gear button or Ctrl+, (Cmd+, on macOS). An **About** section at the bottom (added 2026-11-13) shows the version, the release date and the commit, set by `crates/lohtty-ui/build.rs`: the commit from `git rev-parse`, the date from the version's CHANGELOG heading, and `+N commits (development build)` when HEAD isn't the tag `v<version>` (`GITHUB_REF_NAME` names the tag in the release job, whose shallow checkout has no tags). It reruns only when HEAD, a tag or the changelog changes.
  - **What it edits:** a draft of the theme (bundled plus user themes), opacity, blur, padding, font family, size, ligatures, shell program and arguments (double quotes group an argument), and scrollback.
  - **Save:** the draft is written with `Config::save`, which uses `toml_edit` to update `config.toml` in place. Comments (inline ones too), key order and unmanaged keys such as `font.fallbacks` or the window size are preserved. The workspace then reloads it.
  - **Other buttons:** "Open config.toml" opens the file in the system editor, creating it if needed. Esc, Cancel and × close without saving. Enter in a field saves.
  - **Layout:** the dialog covers the panes, not the title bar, so the window stays movable and closable.
  - **`TextField`:** a reusable single-line input with the same text-input path as the terminal. Like the terminal, it lets Windows resolve a pending dead key (e.g. `"` on US-International) before acting on Enter or Esc.
  - **Search bar:** handles a pending dead key before Enter / Shift+Enter / Esc the same way, so `b"` + Enter searches for `b"`.
  - **Shortcuts while open:** the workspace's action handlers are only registered while no dialog is open, so shortcuts like new tab, split, pane focus and tab switching do nothing behind the dialog. Ctrl+, still works and moves focus back to the dialog. In the title bar, the tabs, + and split buttons are dimmed and covered by an occluding overlay, so clicks don't reach them; the gear, the drag area and the caption buttons still work.

**M4 — Rendering polish** — **done**
- Ligatures (run batching), font fallback for emoji and CJK, wide-character correctness.
- Background opacity and blur.
- Hyperlinks (OSC 8), plus underline styles and colors (curly or dotted, SGR 58).

*Implementation notes from M4:*
- **Default font:** now Cascadia Code on Windows, which has ligatures and ships with Windows Terminal.
- **Fallbacks:** `font.fallbacks` sets an explicit list. When it is unset, lohtty uses whichever popular Nerd Fonts are installed (`DEFAULT_FALLBACKS`, detected once per process). The system fallback still covers emoji and CJK.
- **gpui quirk:** on Windows, `FontFeatures::default()` (an empty list) disables ligatures, because gpui then sets an empty DirectWrite typography. Ligatures are therefore requested explicitly (`calt`, `liga`).
- **Grid snapping:** glyphs are snapped to the grid with `shape_line`'s `force_width`, so fallback glyphs of a different width can't shift a line. Segments with combining marks aren't snapped, because snapping works per glyph.
- **Segment breaks:** wide characters and the cursor cell get their own segments. Ligatures therefore break under the cursor.
- **Hyperlinks:** OSC 8 links are underlined. Ctrl+click (Cmd+click on macOS) opens them, but only for http, https, mailto and file URLs. That click wasn't tested live, to avoid launching a browser.
- **Underlines:** SGR 58 underline colors are supported. gpui only draws straight or wavy lines, so double, dotted and dashed underlines render straight.
- **IME:** the composition text is drawn at the cursor. This wasn't tested live, because no IME is installed on the dev machine.
- **TODO:** automatic URL detection for plain-text links, and a hover style for links.

**M5 — Search** — **done**
- Search bar, regex and case toggles, highlight all matches, jump to a match with scroll.

*Implementation notes from M5:*
- **Search core:** `lohtty-term::search` wraps alacritty's `RegexSearch`. Queries are literal by default (metacharacters escaped) or regex. The case toggle overrides alacritty's smart case with `(?i)` / `(?-i)`. Unit tests run on `mock_term` grids.
- **Opening:** Ctrl+Shift+F (Cmd+F on macOS) opens a per-pane search bar. A one-line selection pre-fills the query. Searching starts from the bottom, i.e. the newest output.
- **Keys in the bar:**

  | Key | Action |
  |---|---|
  | Enter / Up | Older match |
  | Shift+Enter / Down | Newer match |
  | Esc | Close and return focus to the terminal |
  | Backspace / Ctrl+Backspace | Delete a character / clear |
  | Ctrl+V | Paste the first line of the clipboard |

  The bar also has toggles for case (`Aa`) and regex (`.*`), a counter (`n/total`, capped at 10 000), and ↑ ↓ × buttons.
- **Highlighting:** the current match uses the theme's yellow. Other visible matches use a dimmer yellow, so they stay readable on any background.
- **Bar layout:** the bar sits outside the `Terminal` key context, so terminal bindings and PTY input don't fire while typing in it. It occludes the mouse, so clicks on it don't reach the grid.
- **Text input:** the bar has its own input handler, so typing in it goes through the same WM_CHAR path as the terminal.
- **Known limitation:** the counter and current match aren't refreshed while new output streams in. They update on the next keystroke or navigation.
**M6 — Compatibility & hardening** — **partly done** (see open items)
- Pass the vttest core suites and the relevant esctest cases.
- Performance: `cat` of a large file, plus a latency check (target under 1 frame from input to paint).
- Packaging: Windows installer (MSI or WiX) or a portable zip with an icon. Then macOS `.app` and Linux AppImage.
- ✅ **v1.0 ships here, without kitty graphics.**

*Done in M6:*
- **Synchronized-update watchdog** (mode 2026): an app that begins an update and never ends it no longer freezes the screen. The update is flushed at the parser's deadline. The integration test fails if the watchdog is disabled, so it really exercises the path through ConPTY.
- **Escape-sequence check** through a real lohtty window: CUP box, relative moves, ICH, DCH, EL, ECH, DECSC/DECRC and the alt-screen round trip all render as xterm specifies.
- **Throughput:** 14.3 MB / 200k lines via `cmd /c type` takes about 7.5 s. The `throughput` example shows the same time with no rendering at all, so the limit is ConPTY, not lohtty. Run it with `cargo run --release -p lohtty-term --example throughput -- <cmd>`.
- **CPU while streaming** (release build): 5.8 CPU-seconds, of which 0.9 s is reading and parsing, and about 4.9 s is repainting at 60 Hz. Memory goes from 72 to 118 MB with 10k lines of scrollback.
- **Packaging:**
  - The icon is generated by `assets/icons/make-icon.ps1` and embedded with version info through `crates/lohtty/build.rs` (resource ID 1, which gpui uses for the window icon).
  - `scripts/package.ps1 [-Target <triple>]` builds with `--locked` and makes `dist/lohtty-<version>-windows-<arch>.zip` (exe, README, LICENSE, CHANGELOG; 4.2 MB for x64) plus `…-pdb.zip` with the matching symbols (10.6 MB).
  - **Release groundwork (packaging phase 1):**
    - **Toolchain:** pinned in `rust-toolchain.toml` (1.99.1, rustfmt and clippy). CI and the release job run `rustup toolchain install`, so the file is the only place to bump it. CI builds and tests with `--locked`.
    - **Versioning:** the tag `vX.Y.Z` must equal the `lohtty` crate version and have a `## [X.Y.Z]` section in `CHANGELOG.md` (Keep a Changelog format). `scripts/release-check.ps1 -Tag vX.Y.Z [-NotesOut file]` enforces this and extracts the section as release notes. CI checks that it rejects a wrong tag.
    - **Symbols:** the release profile keeps line tables (`debug = "line-tables-only"`). On Windows they go to the separate PDB, so the exe doesn't grow (10.49 MB). The `profiling` profile is now the same as release; it's kept for the documented profiler command.
    - **`release.yml`:** `check` (version, changelog, notes) → `windows` matrix (x64 for now; package, a 5-second smoke launch, upload) → `publish` (SHA256SUMS over all archives, GitHub release with the changelog notes; tags with a `-` are prereleases).
    - **Verified locally:** release-check rejects a malformed tag, a version mismatch and a missing changelog section, and extracts the notes. The package script, the smoke launch and the checksums work. Both workflows parse as YAML. **Not run on GitHub yet** (no remote): in particular, whether the smoke launch works on a GPU-less runner (gpui should fall back to the WARP adapter).
  - **Windows MSI and ARM64 (packaging phase 2, part 1):**
    - **Installer:** `installer/lohtty.wxs` (WiX v5, MS-RL; v6+ adds the Open Source Maintenance Fee EULA, so v5.0.2 is pinned). Per-user (`Scope="perUser"`): installs to `%LOCALAPPDATA%\Programs\lohtty` without admin rights, with the exe, README, LICENSE.txt, CHANGELOG and a Start menu shortcut, and the WixUI_Minimal dialog (license from `LICENSE`, converted to RTF by the script). Each component uses an HKCU value under `Software\lohtty` as its key path, as per-user installs require. The UpgradeCode and the two component GUIDs must never change. Prerelease versions install as their numeric part (0.2.0-rc.1 → 0.2.0).
    - **Build:** `scripts/package.ps1 -Msi` builds `dist/lohtty-<version>-windows-<arch>.msi` (3.7 MB for x64) with the `wix` .NET tool, or `-WixDll`/`-WixUiExt` for the unpacked NuGet packages run on the .NET runtime (no SDK needed).
    - **Tested locally** (x64): a silent per-user install, a launch from the install folder, an upgrade to a test 0.1.1 (one product left), a refused downgrade ("A newer version of lohtty is already installed."), and an uninstall that left no files, shortcut or registry key. The interactive dialog wasn't clicked through. `msiexec` must be given its arguments as a plain list: nested quoting mangled the package path (error 1619 / "cannot open this installation package").
    - **ARM64:** added to the release matrix (cross-compiled on the x64 runner; zip, PDB and MSI, but no launch). It can't be built on the dev machine, whose Visual Studio Build Tools lack the MSVC ARM64 compiler and libraries (`ring`, via gpui's HTTP client, compiles C).
    - **Release job:** installs WiX 5.0.2 and its UI extension, builds both targets with `-Msi`, and for x64 runs the smoke launch plus a silent install, launch and uninstall check. MSIs are published with the zips and covered by `SHA256SUMS`.
  - **Code signing:** releases are unsigned for now (owner's decision, 2026-11-10). The README explains SmartScreen's *More info → Run anyway*, the *Unknown* publisher, and checking downloads against `SHA256SUMS`. Options for later: SignPath Foundation (free for open-source projects, once the repo is public) or Azure Trusted Signing (about $10/month; individual eligibility depends on country). Either way it's one signing step on the exe before the MSI is built, and one on the MSI, with timestamps.
  - **Linux and macOS (packaging phase 3):**
    - **First Linux run** (Docker on the dev machine, Ubuntu 22.04 build image, Xvfb): clippy (with and without kitty graphics) and all 69 tests pass. The app runs with tabs, splits, colors, wide CJK, emoji and a kitty image. Fixes it needed:
      - `libc` is pinned to 0.2.189 in `Cargo.lock`: 0.2.190+ dropped `ENOATTR` on Linux, which `xattr` 0.2.3 (via gpui's HTTP client) uses. **Don't `cargo update` libc past it** until gpui moves off xattr 0.2.
      - The Linux default font `monospace` isn't resolved by gpui (it fell back to a proportional font). `fonts.rs` maps it to the first installed of `MONOSPACE_CANDIDATES` (DejaVu Sans Mono, Noto Sans Mono, Liberation Mono…). The macOS default is now Menlo.
      - The default log filter is lohtty's crates at info and the rest at warn (blade and naga logged about 50 lines at startup).
    - **Rendering check:** Mesa's software Vulkan in Ubuntu 22.04 (LLVM 15) shows a black window under Xvfb (the app still runs). Ubuntu 24.04's Mesa renders correctly, so screenshots come from a 24.04 runtime image.
    - **Package:** `scripts/package-linux.sh [--appimage]` makes `lohtty-<v>-linux-<arch>.tar.gz` (binary, `.desktop`, 256 px icon, docs; 8.6 MB), `-debug.tar.gz` (the line tables, split off with `objcopy` and linked by a debuglink; 17 MB) and an AppImage (8.9 MB). The AppImage bundles libxkbcommon, libxkbcommon-x11 and libxcb-xkb through `$ORIGIN` RUNPATHs (no `LD_LIBRARY_PATH`, which would leak into the shells), and uses appimagetool 1.9.1 and type2-runtime 20251108, pinned by sha256. Release builds need glibc 2.35.
    - **Tested:** the tarball and AppImage start on Ubuntu 24.04. The AppImage also starts on a system without any xkb libraries, where the tarball fails as documented. The shell inside doesn't see `LD_LIBRARY_PATH`. The `.desktop` file passes `desktop-file-validate`.
    - **macOS:** `scripts/package-macos.sh` builds aarch64 and x86_64 (deployment target 11.0), joins them with `lipo`, splits a dSYM, ad-hoc signs `lohtty.app` (Info.plist from `assets/macos/`, bundle id `dev.lohtty.terminal`, icon `lohtty.icns`) and makes `lohtty-<v>-macos-universal.dmg` plus `-dsym.zip`. It needs Xcode (gpui compiles Metal shaders with `xcrun metal`). The default shell is a login shell (portable-pty's default program: `$SHELL`, else the account's shell, with argv0 `-zsh`), and `LC_CTYPE=UTF-8` is set when no locale is, as Terminal.app does. **Built and run on the owner's Mac** (2026-11-10, from a git bundle): the script works and the app runs. Not yet checked in detail: the login shell's `PATH`, Gatekeeper on first launch from the disk image, the Intel slice.
    - **Icons:** `assets/icons/make-icon.ps1` also writes `lohtty.png` (256 px) and `lohtty.icns` (16–1024 px, the tile at 80% as macOS icons are).
    - **Release job:** `linux` matrix (ubuntu-22.04 and ubuntu-22.04-arm; the ARM runner is free for public repos only) packages with `--appimage` and launches both builds under Xvfb; `macos` (macos-latest) packages, checks the app is universal and signed, and launches it from the mounted disk image. Tarballs, AppImages and disk images are published with SHA256SUMS. Not run on GitHub yet (no remote).
    - **Italic without an italic face** (checked 2026-11-10 with a throwaway gpui probe): on Windows, DirectWrite slants the regular face itself (Lucida Console, which has no italic, renders slanted). On Linux and macOS, gpui 0.2.2 renders it upright: cosmic-text can fake italics (`CacheKeyFlags::FAKE_ITALIC`) but gpui always passes empty flags, and its public API can't paint a slanted glyph (`paint_glyph` uses an identity transform). Owner's decision: document it (README troubleshooting) rather than vendor a patched gpui. Revisit if gpui gains synthetic italics. (Box-drawing characters that didn't join are fixed below.)
  - **First release (0.1.0, 2026-11-10):** the repository is private (owner's choice), so the ARM64 Linux job is out of the release matrix (GitHub's ARM runners are free for public repositories only); ARM64 Windows stays (cross-compiled on x64). Private repositories also count Actions minutes (macOS ×10, Windows ×2).
  - **Next packaging steps** (not started): winget and Scoop manifests, ARM64 test on real hardware.
- **Program status (OSC 7501)**, per the Program Status Protocol (superlogical.com/rex/docs/build/program-status; Claude Code 2.1.295+ uses it, and drops its title glyph in terminals that answer the query):
  - **Protocol** (`lohtty_term::status`): `parse` follows the spec's grammar and limits (4096-byte sequence, keys ≤ 16 bytes, value charset, `id` ≤ 128 bytes / 32 per segment / 8 levels, `msg` ≤ 2732 encoded, `title` ≤ 256, `app` ≤ 32; base64 text without control characters; any violation discards the report; unknown keys and malformed pairs are skipped; last repeated key wins; `kind` only with blocked, `progress` 0–100 only with working/blocked). `StatusBoard` holds one record per id (each report replaces it), clears subtrees, inherits `app` from ancestors, keeps at most 256 (least recently updated evicted), drops working/blocked/idle at a new prompt (OSC 133 A; done/error stay), empties on RIS, and drops done/error when the user has seen the pane.
  - **Session:** the OSC scanner also yields 7501 messages, OSC 133 A and RIS. The query `OSC 7501 ; ? ST` is answered with `?` in the filter-reply batch, written before any parsing, so it precedes the DA1 reply that programs send after it (checked through ConPTY: `ESC]7501;?ESC\` then `CSI ?62;22c`). Changes raise `SessionEvent::StatusChanged`.
  - **UI:** each tab shows its panes' most urgent record (blocked > error > working > done > idle): a braille spinner (10 Hz timer, running only while something works) plus the progress, `!` (yellow), `✓` (green), `✕` (red); the tooltip gives title or app, the state (kind for blocked) and the message. Also before the centered single-tab title. Focusing the pane, or reactivating the window on it, marks done/error as seen. A pane whose top state becomes blocked, done or error raises the attention alerts with the message (defaults per kind).
  - **Tested:** parser and board tests (examples from the spec, rules, limits, clears, inheritance, lifetimes, ordering, eviction), scanner tests. Live on Windows: a background tab went working (spinner, 40%) → blocked (`!` and the attention dot) → done (✓), cleared on visit.
  - **OSC 9;4 progress** (ConEmu / Windows Terminal: state 0 none, 1 normal with a value, 2 error, 3 indeterminate, 4 paused) is mapped to the root record as the spec allows: working with the value (paused too, so it doesn't raise alerts), working without a value, error, or removed. Once a pane has sent any OSC 7501 report, OSC 9;4 is ignored until RIS. Live with PowerShell 7.6's `Write-Progress` (`$PSStyle.Progress.UseOSCIndicator = $true`): the tab showed the spinner and 48%; `9;4;2` showed ✕ plus the attention dot; `9;4;0` removed it.
  - **Taskbar** (Windows, `notifier::taskbar_progress`): the button shows the most urgent status of all tabs (so a background agent shows while minimized): working → progress bar or marquee, blocked → paused (yellow), error → red, otherwise none. `ITaskbarList3` is created once on the UI thread (`CoCreateInstance`, after `CoInitializeEx` STA) and called through a hand-declared vtable (slots 3, 9, 10); the last value per window is cached, so the call from `render` only reaches COM on changes. Seen live: partial bar during `Write-Progress`, red on error, normal after.
  - **Not done:** records aren't dropped when the program exits without a prompt starting (lohtty can't tell the agent from the shell).
- **Usage meter** (in progress, branch `feature/usage-meter`): a donut per coding agent on the right of the title bar, outer ring the 5-hour session, inner ring the week, with a tooltip giving each window's use and the time left until it resets.
  - **Step 1 — done:** crate `lohtty-usage`: the model, humanized durations, levels, an RFC 3339 parser (no date crate), and the Codex reader (`codex`): the last `token_count` event's `rate_limits` (`primary` = session, `secondary` = week; `resets_at` in Unix seconds, or the older `resets_in_seconds` from the event's timestamp; event under `payload` or the older `msg`) from the most recently *modified* `$CODEX_HOME/sessions/**/rollout-*.jsonl` (resumed sessions append to their old log), trying the 5 newest. Tested with fixtures only: Codex isn't installed on the dev machine, so the field names come from Codex's source as remembered and must be checked against a real log.
  - **Step 2 — done:** the Claude provider (`claude`): `GET https://api.anthropic.com/api/oauth/usage` with `anthropic-beta: oauth-2025-04-20` and the OAuth token from `$CLAUDE_CONFIG_DIR` or `~/.claude/.credentials.json` (macOS: the Keychain item `Claude Code-credentials`, through `security`). `five_hour` → session, `seven_day` → week, `seven_day_opus` / `seven_day_sonnet` → scoped weeks; `utilization` is a percent. The endpoint is undocumented. The token is read again before each request (Claude Code renews it), never refreshed by lohtty (two refreshers would race), never logged (`Credentials`' `Debug` redacts it), and sent only to that URL; an expired one is reported as "run Claude Code to renew it". HTTP is `ureq` 3 (rustls with ring and webpki roots, already in the tree through gpui), 15 s timeout. `Poller` runs both providers on a thread and reports `Update { provider, state: Unavailable | Ready(snapshot) | Failed(error) }` to a callback: Claude every 120 s, backing off ×2 up to 30 min on errors and waiting at least `Retry-After` on 429 (refreshes don't cut that wait); Codex every 30 s, reported only on change. Dropping the poller stops the thread (not joined, a request may be in flight). Live (2026-11-12, `cargo run -p lohtty-usage --example usage_probe`): `Claude · Team`, session 80 % resets in 24 min, week 86 % resets in 4 d 21 h; Codex "not set up here".
  - **Step 3 — done:** `lohtty-ui/src/usage_meter.rs`. `UsageBoard` keeps one entry per provider (Claude, then Codex): the last good snapshot plus the last error; `Unavailable` removes it, so agents not set up don't show. The workspace spawns the `Poller` and receives its updates over `async_channel`. Each donut (22 px, in a 30 px button left of the split buttons) is drawn on a `canvas` as stroked polylines: outer ring the session (accent, yellow from 70 %, red from 90 %), inner ring the week (muted until 70 %), on a faint track; a mark in the hole tells the agents apart (✻ Claude, ❯ Codex), `!` when there is only an error; a failed last poll dims it. Rings show the use *now* (0 once a window's reset time has passed). The tooltip (`UsageTooltip`) lists every window (label, percent, bar, "resets in 2 h 13 min" / "reset since"), "Updated 40 s ago" (Codex: "As of the last Codex turn, …"), the error if any, and "Click to refresh"; clicking calls `Poller::refresh`. The single tab's centered title used `max_w(50%)` and ran under the right-hand buttons in narrow windows: it now stays centered on the window when it fits between the buttons, and otherwise is centered in the space between them. Live (482 px wide window, GlazeWM): amber donut at 83 % / 86 %, tooltip `Claude · Team`, Session 83 % resets in < 1 min, Week 86 % resets in 4 d 21 h, Updated 8 s ago; the title no longer overlaps.
  - **Step 4 — done:** `[usage]` in `config.toml` (`UsageConfig`: `claude`, `codex`, both on; `refresh`, seconds between Claude requests, 120, at least `MIN_USAGE_REFRESH` = 60). The settings dialog has a "Usage meter (title bar)" section with a toggle per agent; saving writes `usage.claude` / `usage.codex` (`refresh` is left to the file). On a reload that changes `[usage]`, the workspace drops the poller, starts one for the enabled agents (none if both are off) and removes disabled agents from the board; updates for disabled agents (a stopped poller's last one) are ignored. README and CHANGELOG updated. Live: the dialog shows the section with both toggles (screenshot only: toggling saves the real `config.toml`, so the reload path is checked by reading, not live).
  - **Step 5 — done.** Error states checked live (2026-11-12), with environment variables set on the launched lohtty only (`Start-Process -Environment`), the real credentials untouched: **offline** (`HTTPS_PROXY=http://127.0.0.1:9`, which ureq honors): dimmed empty rings with a yellow `!`, tooltip "No data: network error: io: … (os error 10061)" (the OS message, localized); **expired** (`CLAUDE_CONFIG_DIR` → a fake `.credentials.json` with a made-up token and `expiresAt` in the past; nothing is sent): "No data: the sign-in has expired; run Claude Code to renew it"; **refused** (same, expiry in the future; the server answered 401): "No data: the sign-in was refused; sign in again in Claude Code".
  - **Colors and resets** (fake Codex logs through `CODEX_HOME`, Claude hidden with an empty `CLAUDE_CONFIG_DIR`, both on the launched lohtty only): 75 % / 95 % gave a yellow outer and a red inner ring, tooltip `Codex · plus`, "resets in 2 h 13 min" / "resets in 3 d 4 h", "As of the last Codex turn, 22 s ago". A session resetting during the run went from "60 %, resets in < 1 min" to "0 %, reset since" in the tooltip, but the ring only redrew because hovering did: nothing else repaints the bar (Codex's poller reports changes only). Fixed: `UsageBoard::next_reset` gives the soonest reset to come, and the workspace keeps a timer (`usage_reset_redraw`) that repaints a second after it and re-arms. Checked live with the mouse parked off the window: 60 % blue ring before, empty after, the week's 40 % kept.
  - **Wide window:** the 854 px runs are above the narrow threshold (about 770 px with one donut); the title was centered on the window (x ≈ 424) and clear of the donut.
  - **Not checked:** Codex with a real log (the owner chose to skip it): the Codex reader is only tested against fixtures written from memory of Codex's format. Going offline after a first success (kept numbers, dimmed, "Couldn't update: …") is covered by unit tests only.
- **Shift+Enter / kitty keyboard protocol** (for coding agents and others):
  - **Unix:** alacritty_terminal implements the protocol's mode stack and its `CSI ? u` reply only with `Config::kitty_keyboard`, now on. The encoder (`input::kitty_key`) implements the "disambiguate escape codes" level (flag 1, what agents request): Escape → `CSI 27 u`; Enter, Tab, Backspace with modifiers → `CSI 13/9/127 ; mods u`; Ctrl/Alt with a character key → `CSI codepoint ; mods u` (the unshifted key, lower case); AltGr text stays text; other keys keep their legacy forms, which carry modifiers. Higher flags (event types, alternate keys, all keys as escapes, text) are accepted and reported but not acted on. Live on Linux: a raw-mode program pushing flag 1 got `CSI ? 1 u` for its query, `\r` for Enter, `CSI 13;2u` for Shift+Enter, `CSI 13;5u` for Ctrl+Enter, `CSI 27u` for Escape.
  - **Windows:** ConPTY asks for win32 input mode (`CSI ? 9001 h`) at startup, and passes the kitty requests through. `modes::ModeWatcher` tracks 9001 in the output. Input policy: a program that pushed kitty flags gets kitty encoding (ConPTY passes `CSI u` input through to VT readers); otherwise, under 9001, modified Enter is sent as win32 key-down/key-up records (`CSI 13;sc;13|10;1|0;state;1 _`, scan code from `MapVirtualKeyW`), so programs that read console key records (crossterm, so Codex) see the Shift. Everything else keeps VT input, which ConPTY accepts alongside.
  - **Measured on Windows** with a Python probe in lohtty (Enter, then Shift+Enter): key records went from `state=0x0`/`0x0` (main) to `0x0`/`0x10`; a VT reader that pushed kitty flags went from `\r\r` to `\r`, `CSI 13;2u`; a plain VT reader gets `\r\r` (ConPTY doesn't mark Shift on Enter in VT; same as Windows Terminal).
- **Attention alerts** (for coding agents and others):
  - **Detection** (`lohtty_term::notify`): `OscScanner` watches the raw PTY output (before the kitty filter; it changes nothing, alacritty ignores these OSCs) for OSC 9 (bodies starting with a number and `;` are ConEmu sub-commands such as progress, skipped), OSC 777 `notify;title;body` and OSC 99 (kitty: `p=title|body`, `e=1` base64, `d=0` parts joined by `i`; other payload kinds ignored), with BEL or ST, across reads, capped at 8 KB per sequence; text is cleaned of control characters and cut at 256 characters. Each becomes `SessionEvent::Notification`; the bell already was `SessionEvent::Bell`. Both reach the workspace as `TerminalEvent::Attention`.
  - **Policy** (workspace): nothing if the window is active and the pane focused. Otherwise the pane is marked (a yellow dot on its tab, also before the centered single-tab title) until it gets focus, or until the window is activated with it focused. When the window isn't active, the taskbar button flashes (`FlashWindowEx`, until the window comes forward) and, unless `notifications.desktop = false`, a notification shows the message (title: the notification's, else the pane's title; a bell says "Needs your attention"), at most one per pane every 5 s.
  - **Notifications** (`notifier.rs`, Windows): notification-area balloons, which Windows 10 and 11 show as toasts under the exe's description ("lohtty terminal") with the app icon. Unlike WinRT toasts they need no registered AppUserModelID, so the portable build works too. A message-only window on the UI thread (serviced by gpui's message loop) gets the balloon events: a click sends the pane's entity id to the workspace, which shows that tab and pane and activates the window; the icon is removed after a click or timeout. Elsewhere `Notifier` does nothing (tab marks still work).
  - **Agents:** Claude Code sends nothing to terminals it doesn't recognize; `preferredNotifChannel` `"iterm2"` (OSC 9), `"ghostty"` (OSC 777), `"kitty"` (OSC 99) or `"terminal_bell"` makes it reach lohtty. Codex: `[tui] notifications = true`, `notification_method = "osc9"` (its default only notifies while the terminal is unfocused, which lohtty reports with focus events). Documented in the README.
  - **Tested:** scanner tests (forms, ConEmu skips, kitty parts and base64, split reads, cleaning, limits). Live on Windows with a guarded driver: a second tab sending OSC 9 while the first was shown got the dot, cleared on visit; with lohtty minimized, OSC 777 showed a "lohtty terminal" toast with title and message (screenshot of the screen corner); posting the balloon-click message to the notifier window restored lohtty to the foreground on that tab.
  - **macOS** (added 2026-11-13, `notifier.rs`, objc2 0.6 crates as gpui uses): Notification Center through UserNotifications. Permission (alert, sound, badge) is asked at startup, so the prompt shows while the user is there and lohtty is listed in System Settings → Notifications; each post asks again (only reads the answer). The request id is the pane's token, so a pane's notification replaces its previous one only; a delegate class reports clicks (token → `show_pane`) and shows banners even while lohtty is in front. `flash` bounces the Dock icon once (`NSInformationalRequest`), and `dock_badge` shows the number of panes waiting (`self.attention`) as the Dock tile's badge, set from `render` when it changes. Needs the app bundle: a bare binary has no bundle id (UserNotifications would throw), so it logs a warning and skips. Also refused ("Notifications are not allowed for this application") for an app run from a temporary folder (`/private/tmp`); the same bundle under the project's `target/` was accepted. Live on the owner's Mac: permission granted, an OSC 777 written to the test shell's tty while lohtty was in the background was posted, and the banner showed once Focus (*Ne pas déranger*) was off.
  - **Known gap:** if lohtty is killed while a balloon shows, its notification-area icon can linger until hovered (Windows cleans it up then).
- **Box drawing:** U+2500–U+257F and U+2580–U+259F are drawn as shapes (`terminal_view/box_drawing.rs`) instead of font glyphs, which are made for the font's own line height and didn't join at lohtty's 1.25 spacing.
  - Lines run to the cell edges; edges are snapped to device pixels so neighbors meet exactly. Light lines are font size / 12 device pixels (at least 1), heavy twice that, double lines two light lines one light apart.
  - Each arm is drawn on its own and covers the perpendicular lines it meets, which gives closed corners, T-junctions and crosses for every mix of light, heavy and double (the double rules pick inner corner, straight-through, outer corner or a single crossing).
  - Dashes (2, 3 or 4 per cell), the half lines, all blocks and quadrants, and shades (25/50/75% of the foreground's opacity) are rectangles; rounded corners (cubic quarter circles) and diagonals are stroked paths. All are painted in the glyph layer, in the cell's foreground color.
  - Cells with underline, strikethrough or combining marks keep the font's glyph.
  - **Tested:** 8 geometry tests (every character drawn, edges reached, continuity, closed single and double corners, the double cross gap, block fractions, arcs). Live on Linux (Docker, Xvfb) and on Windows, the same script showed every box style, the dashes, blocks, a progress bar and inverse video joined cleanly.
  - **Powerline** (U+E0B0–U+E0BF, the Nerd Font Powerline and Powerline Extra symbols) is drawn the same way: solid arrows, corner triangles and half circles are filled paths reaching the snapped cell edges and the full line height, so they meet the neighboring segment backgrounds; the thin arrows, half-circle outlines and slash separators are strokes. Checked live on Windows with arrow, thin-separator, pill and slanted segments on colored backgrounds, and with the owner's oh-my-posh prompt: no seams where a shape meets the next background.
- **Docs:** `README.md` covers features, config, themes, shortcuts and troubleshooting.
- **Render profiling:** frame-phase counters are logged with `LOHTTY_LOG=lohtty_ui::perf=debug`. Sampling profilers (samply, WPR) need admin rights on Windows. Measured on a 14 MB `cat` with the release build on a 165 Hz display:

  | | fps | Segments/frame | Shape ms | Paint ms | lohtty frame work | Total lohtty CPU |
  |---|---|---|---|---|---|---|
  | Before | 165 | 834 | 1.74 | 0.64 | 45% of a core | 5.2 s |
  | Whole-row segments | 165 | 70 | 1.83 | 0.28 | 39% | — |
  | + output frame cap | 65 | 70 | 1.84 | 0.29 | 16% | **3.1 s** |

  - **Whole-row segments:** blank cells no longer split segments. Held-back spaces join the next text on the same row, as a separate undecorated run so that underlines don't extend under them. Grid snapping keeps glyphs aligned.
  - **Output frame cap:** `MIN_OUTPUT_FRAME` (12 ms) applies only while output streams. Isolated output, like a keystroke's echo, still paints immediately.
  - **Remaining cost:** shaping about 70 new rows per frame (DirectWrite). The layout cache can't help, because every row is new text during a flood.
  - Wall time is unchanged at about 7 s, because ConPTY is the bottleneck.
- **Sampling profile** (samply, 14 MB `cat`, profiling build). UI-thread CPU over the 33 s session was 2.69 s; the PTY reader used only 0.11 s.
  - **Startup:** about 190 ms of NVIDIA driver signature checks (`nvldumdx.dll` → wintrust) when gpui creates the D3D device. This happens once and isn't lohtty code.
  - **Flood:** about 185 ms of CPU per second (18.5% of a core). Of that, `FramePainter::prepaint` takes ~107 ms/s, 95% of which is `shape_line`, i.e. DirectWrite and TextShaping plus their heap allocations. lohtty's own grid walk is 2% of `prepaint`. gpui glyph painting (`paint_glyph`, `raster_bounds`, atlas lookups) is about 7% of the thread.
  - **Idle:** about 17 ms/s (1.7%), mostly kernel time in `NtUserGetMessage` while gpui's Windows run loop wakes for its frame timer. That's gpui behaviour.
  - **Shaping fast path** (done, see below) removed most of the flood cost.
- **Shaping fast path:** most cells are no longer shaped by DirectWrite. `FramePainter::layout_segment` paints each cell's glyph straight at its grid position (`Window::paint_glyph`, or `paint_emoji`), using glyph ids cached per (font, character) in a thread-local table. A character's glyph is found once by shaping it alone, which also resolves font fallback (icons, CJK, symbols).
  - **Still shaped:** runs of two or more ASCII punctuation characters when ligatures are on (`->`, `::`, `!=`, `/*`...). These spans keep their runs' colors and fonts, so a ligature can still form across a color change. They're short and repeat, so gpui's line-layout cache usually has them. Segments with combining marks, underline or strikethrough are still shaped whole.
  - **Layer:** all cell glyphs are painted inside one `paint_layer`. Without it, gpui's scene computes a draw order per glyph, which made painting cost more than the shaping it replaced (1.37 ms vs 0.45 ms per frame).
  - **Baseline:** taken from a shaped reference glyph of the regular font, so cells line up with shaped spans.
  - **Side effect:** italic text now renders with the italic face. When a whole row was shaped as one line, DirectWrite drew the italic runs upright. Decorated segments still go through whole-row shaping and keep that quirk.
  - **Limit:** contextual alternates involving letters or digits (e.g. Fira Code's `0xFF` or a centered `:` in `12:30`) aren't applied, since those characters are painted individually.
  - **Measured** (release, 21 MB / 200k log lines through `cmd /c type`, two runs each; this machine was slower than the earlier table):

    | | Shape ms/frame | Paint ms/frame | lohtty frame work | Process CPU during the flood |
    |---|---|---|---|---|
    | Before | 4.29 | 0.46 | 33% of a core | 6.3 s |
    | Fast path | 0.30 | 0.44 | 7% | 3.4 s |
  - **Symbolizing:** samply showed placeholder names (`fun_*`) for `lohtty.exe`. Symbolize the profile's addresses against `target/profiling/lohtty.pdb` with dbghelp (`SymFromAddrW`, without `SYMOPT_DEFERRED_LOADS`).
- **License:** Apache-2.0, chosen by the owner. The text is in `LICENSE`, it is set in `Cargo.toml`, and it ships in the zip.

- **Input latency** (keystroke to screen, Typometer style): `scripts/latency.ps1` sends a real `W` key event, then polls a few screen pixels where the character appears (GDI `BitBlt`, which returns about once per desktop composition, so samples are quantized to ~6 ms at 165 Hz). Same shell in every terminal: pwsh 7.6 + oh-my-posh + PSReadLine; 80 samples per run, two runs each, release build:

  | | median | p90 |
  |---|---|---|
  | lohtty before | 12.2 ms | 13.5 ms |
  | lohtty now | 6.4–7.0 ms | 12 ms |
  | Windows Terminal | 6.7 ms | 12.3 ms |
  | WezTerm | 42.7 ms | 48.3 ms |

  - **Breakdown inside lohtty** (`LOHTTY_LOG=lohtty_ui::latency=debug` logs each keystroke): the shell's echo arrives about 0.8 ms after the key is written. The frame was then painted 0–6 ms later, because gpui on Windows only draws on WM_PAINT, which its vsync thread sends right after each composition (`DwmFlush`). The frame then missed the composition it could have caught.
  - **Fix:** `PaintNow` calls `InvalidateRect` on the window for output arriving within 250 ms of user input (`ECHO_WINDOW`). gpui then draws and presents as soon as the UI thread is free, about 0.5 ms after the echo. Streaming output still repaints on the vsync cadence, so flood CPU is unchanged (within run-to-run noise).
  - Remaining time is outside lohtty: key delivery, presenting, and the desktop composition. Windows Terminal has the same floor.

*Open items (blocking a v1.0 tag):*
- **vttest/esctest:** these need a Unix environment. The dev machine's WSL only has Rancher Desktop's internal distros. Install a regular distro (for example `wsl --install Ubuntu`) and then run `vttest` inside lohtty.
- **Later:** an MSI installer, a macOS `.app` and a Linux AppImage.

**M7 — Kitty graphics protocol** *(optional, last)*
- Add the `kitty-graphics` cargo feature and the `graphics.kitty` config flag. CI builds with and without the feature.
- Start with a spike to check whether APC sequences survive ConPTY (Risks §6.2). If they don't, scope the feature to WSL or bundle a newer ConPTY.
- APC interceptor as an `OutputFilter`, direct transmission plus PNG and RGB/RGBA, placements anchored to scrollback.
- Delete commands, then file and temp-file transmission, then animation (stretch goal).
- ✅ Done when `kitty +kitten icat` (through WSL), `timg`, and `yazi` previews work, and a build without the feature is unchanged.

*Status: core done; checked with real clients on Linux (below).*
- **ConPTY spike:** APC sequences pass through ConPTY intact on Windows 11 build 26300 (`cargo run -p lohtty-term --example apc_probe`), in both directions: a query's reply (`ESC _G i=31;OK ESC \`) reaches a console program reading stdin in VT input mode.
- **Feature and config:** `kitty-graphics` cargo feature, on by default in the `lohtty` binary (`--no-default-features` leaves it out; CI builds both). `graphics.kitty = false` turns it off for new tabs and panes.
- **Filter** (`lohtty_term::kitty`): a streaming scanner takes `ESC _ G … ESC \` out of the PTY stream, across read boundaries; other output, including other APCs, passes through. Handles chunked transfers (`m=1`), `a=t/T/p/q/d`, formats 24/32/100 (PNG), zlib (`o=z`), direct, file, temporary-file and shared-memory media (`t=d/f/t/s`), image ids and numbers (`i`/`I`), placement ids (`p`), source rectangles, cell offsets, `c`/`r` with aspect ratio, `C=1`, `z`, and quiet modes. Replies follow kitty's rules. Decoded data is capped at 320 MB (oldest images dropped first), a command at 400 MB, an image at 100 Mpx.
- **Anchoring:** for each displayed image, the filter injects one marker cell per image row into the parser's input: a blank cell with an OSC 8 hyperlink `lohtty-kitty:<placement>:<row>` at the image's left column, then moves the cursor as kitty does (after the image on its last row; `C=1` saves and restores it without scrolling). Markers scroll, get cleared and leave scrollback with the text, so the renderer simply draws each placement where its markers are visible, clipped to the visible rows. Trade-off: the first cell of each image row is overwritten with a blank, and text later written over a marker hides that image row.
- **Rendering:** markers are skipped as links (no underline, not clickable). Each image is converted once to a gpui BGRA texture, shared by all its placements: a placement draws the whole texture, scaled and shifted so its source rectangle lands on the destination, clipped to it (so tiles of one image join without seams). Drawn above text (`z ≥ 0`) or below it (`z < 0`), and released when no placement is on screen.
- **Files:** only regular local files are read. Device, pipe (`\\.\…`) and network paths are refused, since opening them could block the PTY reader. Temporary files are deleted only if their path contains `tty-graphics-protocol` and, once resolved, is in a temporary directory: the system's, or `/tmp` and `/dev/shm` on Unix.
- **Real clients** (2026-11-10, Docker on Ubuntu 24.04 under Xvfb: kitty 0.32.2's `icat`, timg 1.5.2, chafa 1.14.0, yazi 26.9.1). All four now display images. Fixes they needed:
  - **icat hung** in its support detection: it ignores a DA1 reply of three characters or fewer, and alacritty_terminal answers `CSI ? 6 c`. lohtty now answers `CSI ? 62 ; 22 c` (VT220 with ANSI color; `session.rs`, `device_attributes`).
  - **chafa's images were rejected:** it pads every chunk of a chunked transfer, and its pieces end in non-canonical base64 characters. The payload is now decoded piece by piece (split after each `=` run), ignoring unused trailing bits, as kitty does.
  - **Temporary files in `/dev/shm`** (where icat makes them) weren't deleted.
  - **yazi's preview showed a faint grid:** it places an image as one cell-sized tile per cell (`c=1,r=1` with a source rectangle each). Fractional cells stretched each tile, and per-tile textures couldn't sample across tile edges. Cells are now whole device pixels (as in kitty, WezTerm and Windows Terminal; recomputed when the display scale changes), and placements draw from the image's single texture (above).
  - timg plays its GIF animation itself by sending frames, so it works without protocol animation.
  - The Linux test image (`lohtty-clients`: the 24.04 runtime image plus the four clients and test images) and its driver script are in the session scratchpad, not the repo.
- **Deletes by position** (`d=c/p/q/x/y`, upper-case to free the images): they depend on the screen at that point of the output, but the filter runs before the parser. `OutputFilter` has two hooks for this: `take_checkpoints()` gives offsets into the filtered output, and the reader parses up to each one and calls `at_checkpoint(&dyn Screen)`. `Screen` (implemented for alacritty's `Term`) exposes the cursor and the screen's hyperlinked cells, i.e. the markers: a placement covers its marker rows, from the marker column across its `cols`. Filter replies are still sent before any parsing, so a client's graphics replies keep arriving before the parser's DA1. `d=r/R` (image ids in `x..=y`) needs no screen. Checked by tests (cell, cell plus z-index, column, row, cursor moved earlier in the same read, id range, upper-case freeing) and live on Linux (two images, one deleted by a cell inside it, the other at the cursor). During a synchronized update (mode 2026) the parser buffers output, so a positional delete inside one sees the screen from before it.
- **Unicode placeholders** (`U=1`): `a=p`/`a=T` with `U=1` stores a *virtual* placement per (image id, placement id), sized like a normal one (`c`/`r`, or from the image), with no markers and no cursor movement. The client prints U+10EEEE where the image goes: the image id is in the foreground color (24-bit value, or 256-color index), its top byte optionally in a third diacritic; the placement id is in the underline color; row and column are diacritics, indexes into the protocol's 297-entry table (`kitty::placeholder`, generated from kitty's `rowcolumn-diacritics.txt`). A missing row or column continues the placeholder to the left. The painter skips these cells' text and hands them to the image layer, which merges runs of consecutive cells, fits the source into the placement's c×r cells (aspect kept, centered) and draws the image's shared texture clipped to the run. A cell without a placement id uses the image's lowest-numbered virtual placement. Deletes by id, number, z-index, id range and `a` remove virtual placements too; positional ones don't (no position); freeing checks both kinds.
  - **Checked:** decoding tests (table, explicit values, inheritance, image id top byte) and store tests (no markers, sizes, lookup, deletes, freeing). Live on Linux: `kitty icat --unicode-placeholder` directly, and `kitty icat` inside tmux 3.4. tmux needs `allow-passthrough on` and `terminal-features …:RGB`, otherwise it downgrades the 24-bit colors that carry the image id (documented in the README). On Windows, a PowerShell script drawing a 10×5 virtual placement with explicit and inherited cells.
  - **yazi inside tmux shows no image:** it decides from its support queries, whose replies tmux doesn't pass back, and it doesn't know lohtty by name. That's for yazi to change.
- **Shared memory** (`t=s`, `kitty::shm`): the payload names the object. On Unix it's opened with `shm_open` (a leading `/` is added if missing; other slashes are refused), mapped read-only (macOS can't `read` shared memory), copied for `O`/`S`, then removed with `shm_unlink` as the protocol requires. On Windows it's a named file mapping (`OpenFileMappingW`, `MapViewOfFile`, size from `VirtualQuery`), declared by hand like `paint_now`'s `InvalidateRect`; it disappears when the client closes its handle. Sizes are capped like a command (400 MB). Tests create real objects on both systems (Unix: read without the slash, then gone). Live on Linux: `icat --detect-support` now answers `memory`; icat and yazi transmit with `t=s` and display, and `/dev/shm` is empty afterwards.
- **Not done:** animation (`a=f/a/c`). `kitty icat` through WSL on Windows isn't checked (no WSL distro on the dev machine); on Windows the protocol is checked with PowerShell senders (chunked PNG, display, partial scroll-off, scrollback, `cls`) and a query script.

---

## 6. Risks & open questions

*Risks 1 and 2 only affect the optional M7. They no longer block v1.0.*

1. **Kitty graphics with alacritty_terminal.** alacritty drops APC sequences and has no notion of image cells. The interceptor approach works, but cursor movement after a placement and image eviction on scroll or clear must be kept in sync with the `Term`'s state. If this becomes fragile, fall back to a custom grid built on `vte` behind the `TerminalBackend` boundary.
2. **Kitty graphics over ConPTY.** Windows ConPTY can strip or rewrite unknown escape sequences. Recent Windows builds pass through more, but APC passthrough must be verified early (spike during M1). If ConPTY eats APC, the options are: rely on WSL with a direct PTY (no ConPTY), or require a recent ConPTY (ship `conpty.dll` and `OpenConsole.exe` from Windows Terminal, as WezTerm does).
3. **gpui maturity.** gpui is pre-1.0, and its API changes between versions. Pin an exact version (or git rev) and expect migrations. Windows support is now usable (Zed ships on Windows), but transparency and blur are less mature there.
4. **Ligatures vs. grid alignment.** Shaped runs must not drift from the cell grid. Run batching has to break at style changes, wide chars, and the cursor.
5. **IME / dead keys on Windows.** gpui's `InputHandler` must be wired up correctly for accented characters on non-US layouts (for example AZERTY).

---

## 7. Immediate next steps

1. Create the workspace and crates (M0).
2. Build M1. Add the pass-through `OutputFilter` hook from the start, so kitty graphics can be added later without refactoring.
3. The ConPTY/APC spike moves to the start of M7.
