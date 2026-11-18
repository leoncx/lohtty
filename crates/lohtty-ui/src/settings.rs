//! The settings dialog: edits a draft of the config and writes it to
//! `config.toml` (in place, keeping comments) on Save. The workspace then
//! reloads it like any other config change.

use gpui::{
    AnyElement, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement,
    KeyDownEvent, MouseButton, Render, Rgba, SharedString, Subscription, Window, div, prelude::*,
    px,
};
use lohtty_config::{Config, themes_dir};
use lohtty_theme::{Theme, available_names};

use crate::colors::{mix, to_rgba};
use crate::text_field::{TextField, TextFieldEvent, TextFieldStyle};

#[derive(Debug, Clone)]
pub enum SettingsEvent {
    Close,
    /// The config was written to disk.
    Saved,
}

pub struct SettingsDialog {
    focus: FocusHandle,
    /// The config as loaded, used for "open config file" when none exists yet.
    original: Config,
    draft: Config,
    theme: Theme,
    theme_names: Vec<String>,
    family: Entity<TextField>,
    program: Entity<TextField>,
    args: Entity<TextField>,
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SettingsEvent> for SettingsDialog {}

impl SettingsDialog {
    pub fn new(config: Config, theme: Theme, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let style = field_style(&theme);
        let family = cx.new(|cx| TextField::new(config.font.family.clone(), "", style, cx));
        let program = cx.new(|cx| {
            TextField::new(
                config.shell.program.clone().unwrap_or_default(),
                default_shell_hint(),
                style,
                cx,
            )
        });
        let args =
            cx.new(|cx| TextField::new(join_args(&config.shell.args), "No arguments", style, cx));
        let subscriptions = [&family, &program, &args]
            .into_iter()
            .map(|field| cx.subscribe_in(field, window, Self::on_field_event))
            .collect();

        Self {
            focus: cx.focus_handle(),
            original: config.clone(),
            draft: config,
            theme_names: available_names(themes_dir().as_deref()),
            theme,
            family,
            program,
            args,
            error: None,
            _subscriptions: subscriptions,
        }
    }

    fn on_field_event(
        &mut self,
        _field: &Entity<TextField>,
        event: &TextFieldEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            TextFieldEvent::Submit => self.save(cx),
            TextFieldEvent::Cancel => cx.emit(SettingsEvent::Close),
            TextFieldEvent::Changed => {}
        }
    }

    /// Validates the draft and writes it to disk.
    fn save(&mut self, cx: &mut Context<Self>) {
        let mut config = self.draft.clone();
        let family = self.family.read(cx).text().trim().to_owned();
        if family.is_empty() {
            self.error = Some("The font family can't be empty.".into());
            cx.notify();
            return;
        }
        config.font.family = family;
        let program = self.program.read(cx).text().trim().to_owned();
        config.shell.program = (!program.is_empty()).then_some(program);
        config.shell.args = match split_args(self.args.read(cx).text()) {
            Ok(args) => args,
            Err(e) => {
                self.error = Some(e.into());
                cx.notify();
                return;
            }
        };

        match config.save() {
            Ok(path) => {
                tracing::info!(?path, "settings saved");
                cx.emit(SettingsEvent::Saved);
            }
            Err(e) => {
                self.error = Some(format!("Could not save: {e}").into());
                cx.notify();
            }
        }
    }

    /// Opens config.toml in the system editor (creating it from the current
    /// settings if it doesn't exist yet).
    fn open_config_file(&mut self, cx: &mut Context<Self>) {
        let Some(path) = Config::default_path() else {
            return;
        };
        if !path.exists()
            && let Err(e) = self.original.save()
        {
            self.error = Some(format!("Could not create {}: {e}", path.display()).into());
            cx.notify();
            return;
        }
        cx.open_with_system(&path);
    }

    fn on_key_down(&mut self, ev: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if ev.keystroke.key == "escape" {
            cx.emit(SettingsEvent::Close);
            cx.stop_propagation();
        }
    }
}

impl Focusable for SettingsDialog {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

/// Colors used by the dialog, derived from the theme.
struct Palette {
    panel: Rgba,
    fg: Rgba,
    muted: Rgba,
    border: Rgba,
    hover: Rgba,
    accent: Rgba,
    on_accent: Rgba,
    error: Rgba,
}

impl Palette {
    fn new(t: &Theme) -> Self {
        Self {
            panel: to_rgba(mix(t.background, t.foreground, 0.05)),
            fg: to_rgba(t.foreground),
            muted: to_rgba(mix(t.foreground, t.background, 0.45)),
            border: to_rgba(mix(t.background, t.foreground, 0.2)),
            hover: to_rgba(mix(t.background, t.foreground, 0.14)),
            accent: to_rgba(t.ansi[4]),
            on_accent: to_rgba(t.background),
            error: to_rgba(t.ansi[1]),
        }
    }
}

fn field_style(t: &Theme) -> TextFieldStyle {
    let p = Palette::new(t);
    TextFieldStyle {
        bg: to_rgba(t.background),
        fg: p.fg,
        muted: p.muted,
        border: p.border,
        accent: p.accent,
    }
}

impl SettingsDialog {
    fn row(&self, label: &'static str, control: impl IntoElement) -> AnyElement {
        let p = Palette::new(&self.theme);
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .min_h(px(30.0))
            .child(
                div()
                    .w(px(110.0))
                    .flex_none()
                    .text_color(p.muted)
                    .child(label),
            )
            .child(div().flex_1().min_w_0().child(control))
            .into_any_element()
    }

    /// The About section: version, release date and commit, set at build
    /// time (see `build.rs`).
    fn about(&self) -> Vec<AnyElement> {
        // "0.2.0", or "0.2.0 +7 commits (development build)" past the tag.
        let version = env!("CARGO_PKG_VERSION");
        let version = match env!("LOHTTY_DEV_BUILD") {
            "" => version.to_owned(),
            "dev" => format!("{version} (development build)"),
            since => format!("{version} {since} (development build)"),
        };
        let mut rows = vec![self.section("About"), self.row("Version", version)];
        for (label, value) in [
            ("Released", env!("LOHTTY_RELEASE_DATE")),
            ("Commit", env!("LOHTTY_COMMIT")),
        ] {
            if !value.is_empty() {
                rows.push(self.row(label, value));
            }
        }
        rows
    }

    fn section(&self, title: &'static str) -> AnyElement {
        let p = Palette::new(&self.theme);
        div()
            .pt_3()
            .pb_1()
            .text_xs()
            .text_color(p.accent)
            .child(title.to_uppercase())
            .into_any_element()
    }

    /// `[-] value [+]`, applying `step` to the draft through `update`.
    fn stepper(
        &self,
        id: &'static str,
        value: String,
        cx: &mut Context<Self>,
        update: impl Fn(&mut Config, i32) + Clone + 'static,
    ) -> AnyElement {
        let p = Palette::new(&self.theme);
        let button = |suffix: &'static str, label: &'static str| {
            div()
                .id(SharedString::from(format!("{id}-{suffix}")))
                .w(px(26.0))
                .h(px(26.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded_sm()
                .border_1()
                .border_color(p.border)
                .hover(|s| s.bg(p.hover))
                .child(label)
        };
        let (dec, inc) = (update.clone(), update);
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(
                button("dec", "−").on_click(cx.listener(move |this, _, _, cx| {
                    dec(&mut this.draft, -1);
                    cx.notify();
                })),
            )
            .child(div().min_w(px(64.0)).flex().justify_center().child(value))
            .child(
                button("inc", "+").on_click(cx.listener(move |this, _, _, cx| {
                    inc(&mut this.draft, 1);
                    cx.notify();
                })),
            )
            .into_any_element()
    }

    fn toggle(
        &self,
        id: &'static str,
        on: bool,
        cx: &mut Context<Self>,
        flip: impl Fn(&mut Config) + 'static,
    ) -> AnyElement {
        let p = Palette::new(&self.theme);
        div()
            .id(id)
            .w(px(40.0))
            .h(px(22.0))
            .flex()
            .flex_row()
            .items_center()
            .px(px(3.0))
            .rounded_full()
            .border_1()
            .border_color(if on { p.accent } else { p.border })
            .bg(if on { p.accent } else { p.panel })
            .when(on, |el| el.justify_end())
            .child(div().w(px(14.0)).h(px(14.0)).rounded_full().bg(if on {
                p.on_accent
            } else {
                p.muted
            }))
            .on_click(cx.listener(move |this, _, _, cx| {
                flip(&mut this.draft);
                cx.notify();
            }))
            .into_any_element()
    }

    fn theme_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = Palette::new(&self.theme);
        div()
            .flex()
            .flex_row()
            .flex_wrap()
            .gap_1()
            .children(self.theme_names.iter().enumerate().map(|(i, name)| {
                let selected = *name == self.draft.theme;
                let value = name.clone();
                div()
                    .id(("theme", i))
                    .px_2()
                    .py(px(3.0))
                    .rounded_sm()
                    .border_1()
                    .border_color(if selected { p.accent } else { p.border })
                    .text_color(if selected { p.fg } else { p.muted })
                    .hover(|s| s.bg(p.hover))
                    .child(name.clone())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.draft.theme = value.clone();
                        cx.notify();
                    }))
            }))
            .into_any_element()
    }
}

impl Render for SettingsDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::new(&self.theme);
        let d = &self.draft;
        let button = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .px_3()
                .py_1()
                .rounded_sm()
                .border_1()
                .border_color(if primary { p.accent } else { p.border })
                .bg(if primary { p.accent } else { p.panel })
                .text_color(if primary { p.on_accent } else { p.fg })
                .hover(|s| {
                    if primary {
                        s.opacity(0.85)
                    } else {
                        s.bg(p.hover)
                    }
                })
                .child(label)
        };

        let mut rows: Vec<AnyElement> = vec![
            self.section("Appearance"),
            self.row("Theme", self.theme_picker(cx)),
            self.row(
                "Opacity",
                self.stepper(
                    "opacity",
                    format!("{:.0}%", d.window.opacity * 100.0),
                    cx,
                    |c, step| {
                        c.window.opacity = (c.window.opacity + 0.05 * step as f32).clamp(0.3, 1.0);
                    },
                ),
            ),
            self.row(
                "Blur",
                self.toggle("blur", d.window.blur, cx, |c| {
                    c.window.blur = !c.window.blur
                }),
            ),
            self.row(
                "Dim unfocused panes",
                self.toggle("dim-unfocused", d.window.dim_unfocused, cx, |c| {
                    c.window.dim_unfocused = !c.window.dim_unfocused
                }),
            ),
            self.row(
                "Padding",
                self.stepper(
                    "padding",
                    format!("{:.0} px", d.window.padding),
                    cx,
                    |c, step| {
                        c.window.padding = (c.window.padding + 2.0 * step as f32).clamp(0.0, 40.0);
                    },
                ),
            ),
            self.section("Font"),
            self.row("Family", self.family.clone()),
            self.row(
                "Size",
                self.stepper("size", format!("{:.0}", d.font.size), cx, |c, step| {
                    c.font.size = (c.font.size + step as f32).clamp(6.0, 72.0);
                }),
            ),
            self.row(
                "Ligatures",
                self.toggle("ligatures", d.font.ligatures, cx, |c| {
                    c.font.ligatures = !c.font.ligatures
                }),
            ),
            self.section("Usage meter (title bar)"),
            self.row(
                "Claude",
                self.toggle("usage-claude", d.usage.claude, cx, |c| {
                    c.usage.claude = !c.usage.claude
                }),
            ),
            self.row(
                "Codex",
                self.toggle("usage-codex", d.usage.codex, cx, |c| {
                    c.usage.codex = !c.usage.codex
                }),
            ),
            self.section("Shell (new tabs and panes)"),
            self.row("Program", self.program.clone()),
            self.row("Arguments", self.args.clone()),
            self.section("Terminal"),
            self.row(
                "Scrollback",
                self.stepper(
                    "scrollback",
                    format!("{} lines", d.scrollback),
                    cx,
                    |c, step| {
                        c.scrollback =
                            (c.scrollback as i64 + 1000 * step as i64).clamp(0, 100_000) as usize;
                    },
                ),
            ),
            self.row(
                "Copy on select",
                self.toggle("copy-on-select", d.copy_on_select, cx, |c| {
                    c.copy_on_select = !c.copy_on_select
                }),
            ),
        ];
        rows.extend(self.about());

        div()
            .id("settings")
            .track_focus(&self.focus)
            .key_context("Settings")
            .on_key_down(cx.listener(Self::on_key_down))
            // Clicks on empty parts of the panel give it focus (for Esc).
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, _| window.focus(&this.focus)),
            )
            .w(px(560.0))
            .max_h(px(640.0))
            .flex()
            .flex_col()
            .rounded_lg()
            .border_1()
            .border_color(p.border)
            .bg(p.panel)
            .text_color(p.fg)
            .text_sm()
            .shadow_lg()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .px_4()
                    .py_2()
                    .border_b_1()
                    .border_color(p.border)
                    .child(div().flex_1().text_base().child("Settings"))
                    .child(
                        div()
                            .id("settings-close")
                            .px_2()
                            .rounded_sm()
                            .text_color(p.muted)
                            .hover(|s| s.bg(p.hover).text_color(p.fg))
                            .child("×")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(SettingsEvent::Close))),
                    ),
            )
            .child(
                div()
                    .id("settings-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_4()
                    .pb_3()
                    .children(rows),
            )
            .children(
                self.error
                    .clone()
                    .map(|e| div().px_4().py_1().text_color(p.error).child(e)),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_4()
                    .py_2()
                    .border_t_1()
                    .border_color(p.border)
                    .child(
                        button("settings-open-file", "Open config.toml", false)
                            .on_click(cx.listener(|this, _, _, cx| this.open_config_file(cx))),
                    )
                    .child(div().flex_1())
                    .child(
                        button("settings-cancel", "Cancel", false)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(SettingsEvent::Close))),
                    )
                    .child(
                        button("settings-save", "Save", true)
                            .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                    ),
            )
    }
}

fn default_shell_hint() -> &'static str {
    if cfg!(windows) {
        "Default (pwsh.exe, else powershell.exe)"
    } else {
        "Default ($SHELL)"
    }
}

/// Joins arguments for display, quoting those containing spaces.
fn join_args(args: &[String]) -> String {
    args.iter()
        .map(|a| {
            if a.is_empty() || a.contains(char::is_whitespace) {
                format!("\"{a}\"")
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Splits on whitespace; double quotes group an argument containing spaces.
fn split_args(s: &str) -> Result<Vec<String>, &'static str> {
    let mut args = Vec::new();
    let mut current = String::new();
    let (mut in_quotes, mut has_token) = (false, false);
    for c in s.chars() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                has_token = true;
            }
            c if c.is_whitespace() && !in_quotes => {
                if has_token {
                    args.push(std::mem::take(&mut current));
                    has_token = false;
                }
            }
            c => {
                current.push(c);
                has_token = true;
            }
        }
    }
    if in_quotes {
        return Err("Unclosed quote in the shell arguments.");
    }
    if has_token {
        args.push(current);
    }
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_args_handles_quotes() {
        assert_eq!(split_args("").unwrap(), Vec::<String>::new());
        assert_eq!(
            split_args("  -NoLogo  -NoExit ").unwrap(),
            ["-NoLogo", "-NoExit"]
        );
        assert_eq!(
            split_args(r#"-File "C:\My Scripts\init.ps1""#).unwrap(),
            ["-File", r"C:\My Scripts\init.ps1"]
        );
        assert_eq!(split_args(r#"a "" b"#).unwrap(), ["a", "", "b"]);
        assert!(split_args(r#"-c "oops"#).is_err());
    }

    #[test]
    fn join_then_split_roundtrips() {
        let args = vec![
            "-NoLogo".to_string(),
            r"C:\My Scripts\x.ps1".to_string(),
            String::new(),
        ];
        assert_eq!(split_args(&join_args(&args)).unwrap(), args);
    }
}
