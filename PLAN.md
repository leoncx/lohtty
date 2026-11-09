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
│   ├── themes/tokyo-night.toml (+ storm, moon, day)
│   └── icons/lohtty.ico
├── crates/
│   ├── lohtty/                # binary: main.rs, app bootstrap, actions, keymap
│   ├── lohtty-config/         # config schema, loading, defaults, hot reload
│   ├── lohtty-theme/          # Theme struct, palette (16 ANSI + 256 + truecolor), loader
│   ├── lohtty-term/           # Terminal session: PTY, alacritty Term, events, search
│   ├── lohtty-graphics/       # OPTIONAL (feature `kitty-graphics`): parser, image store, placements
│   └── lohtty-ui/             # gpui views: Workspace, TabBar, PaneGroup, TerminalView, SearchBar
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
- Cargo workspace, CI (fmt, clippy, test on Windows, macOS, Linux), `tracing`.
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

**M2 — Config, theme, shell selection, title**
- `lohtty-config` with defaults and hot reload, `lohtty-theme` loader, bundled Tokyo Night variants.
- Shell selection from config.
- OSC title → window title.
- Selection, copy/paste (Ctrl+Shift+C/V), and bracketed paste.
- Mouse reporting (SGR 1006 and normal modes) for TUI apps.

**M3 — Tabs & splits**
- Workspace, TabBar, and PaneGroup split tree with focus management and divider dragging.
- Per-pane title, with the tab showing the active pane's title.
- Closing the last pane closes the tab, and closing the last tab closes the window (configurable).

**M4 — Rendering polish**
- Ligatures (run batching), font fallback for emoji and CJK, wide-character correctness.
- Background opacity and blur.
- Hyperlinks (OSC 8), plus underline styles and colors (curly or dotted, SGR 58).

**M5 — Search**
- Search bar, regex and case toggles, highlight all matches, jump to a match with scroll.

**M6 — Compatibility & hardening**
- Pass the vttest core suites and the relevant esctest cases.
- Performance: `cat` of a large file, plus a latency check (target under 1 frame from input to paint).
- Packaging: Windows installer (MSI or WiX) or a portable zip with an icon. Then macOS `.app` and Linux AppImage.
- ✅ **v1.0 ships here, without kitty graphics.**

**M7 — Kitty graphics protocol** *(optional, last)*
- Add the `kitty-graphics` cargo feature and the `graphics.kitty` config flag. CI builds with and without the feature.
- Start with a spike to check whether APC sequences survive ConPTY (Risks §6.2). If they don't, scope the feature to WSL or bundle a newer ConPTY.
- APC interceptor as an `OutputFilter`, direct transmission plus PNG and RGB/RGBA, placements anchored to scrollback.
- Delete commands, then file and temp-file transmission, then animation (stretch goal).
- ✅ Done when `kitty +kitten icat` (through WSL), `timg`, and `yazi` previews work, and a build without the feature is unchanged.

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
