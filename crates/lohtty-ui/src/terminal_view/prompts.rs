//! Shell integration (OSC 133, see `lohtty_term::prompt`): jumping between
//! prompts, and selecting the last command's output.

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use gpui::{ClipboardItem, Context, Window};
use lohtty_term::prompt::{self, Mark};

use super::{NextPrompt, PreviousPrompt, SelectLastOutput, TerminalView};

impl TerminalView {
    /// The selected text, without shell integration marks.
    pub(super) fn selection_text(&self) -> Option<String> {
        let text = self.session.term().lock().selection_to_string()?;
        Some(prompt::strip(&text)).filter(|t| !t.is_empty())
    }

    pub(super) fn previous_prompt(
        &mut self,
        _: &PreviousPrompt,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.scroll_to_prompt(false, cx);
    }

    pub(super) fn next_prompt(
        &mut self,
        _: &NextPrompt,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.scroll_to_prompt(true, cx);
    }

    /// Scrolls so the prompt above (or below) the top of the view is at the
    /// top; below the last prompt, back to the bottom.
    fn scroll_to_prompt(&mut self, down: bool, cx: &mut Context<Self>) {
        let delta = {
            let term = self.session.term().lock();
            let offset = term.grid().display_offset() as i32;
            let top = Line(-offset);
            let prompts = prompt::all_marks(&term)
                .into_iter()
                .filter(|m| matches!(m.mark, Mark::Prompt { .. }))
                .map(|m| m.point.line);
            let target = if down {
                prompts.into_iter().find(|&line| line > top)
            } else {
                prompts.into_iter().rev().find(|&line| line < top)
            };
            let new_offset = match target {
                Some(line) => (-line.0).max(0),
                None if down => 0,
                None => return,
            };
            new_offset - offset
        };
        if delta != 0 {
            self.session.scroll(delta);
            cx.notify();
        }
    }

    /// Selects the output of the last command (still running or not) and
    /// copies it, scrolling up to its start if it is above the view.
    pub(super) fn select_last_output(
        &mut self,
        _: &SelectLastOutput,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (text, scroll) = {
            let mut term = self.session.term().lock();
            let marks = prompt::all_marks(&term);
            let Some((start, end)) = prompt::last_output(&marks) else {
                return;
            };
            let last_column = term.last_column();
            let end = match end {
                // Up to the cell before the next prompt.
                Some(p) if p.column.0 > 0 => Point::new(p.line, p.column - 1),
                Some(p) => Point::new(p.line - 1, last_column),
                // Still running: up to the cursor's line.
                None => Point::new(term.grid().cursor.point.line, last_column),
            };
            if end < start {
                return;
            }
            let mut selection = Selection::new(SelectionType::Simple, start, Side::Left);
            selection.update(end, Side::Right);
            term.selection = Some(selection);
            let text = term
                .selection_to_string()
                .map(|t| prompt::strip(&t).trim_end().to_owned());
            let top = Line(-(term.grid().display_offset() as i32));
            let scroll = (start.line < top).then(|| {
                let max = term.grid().history_size() as i32;
                (-start.line.0).clamp(0, max) - term.grid().display_offset() as i32
            });
            (text, scroll)
        };
        if let Some(delta) = scroll {
            self.session.scroll(delta);
        }
        if let Some(text) = text.filter(|t| !t.is_empty()) {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
        cx.notify();
    }
}
