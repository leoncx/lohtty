//! Search within a terminal pane: opening the bar, navigating matches.

use gpui::{AppContext, Context, Entity, Focusable, Subscription, Window};
use lohtty_term::search::{self, Match, RegexSearch};

use super::{Search, TerminalView};
use crate::search_bar::{SearchBar, SearchEvent};

/// Maximum number of matches counted for the "n/total" display.
const COUNT_LIMIT: usize = 10_000;

pub(super) struct SearchState {
    pub bar: Entity<SearchBar>,
    pub regex: Option<RegexSearch>,
    pub current: Option<Match>,
    _subscription: Subscription,
}

impl TerminalView {
    /// Opens the search bar (prefilled with a one-line selection), or
    /// focuses it if already open.
    pub(super) fn open_search(&mut self, _: &Search, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(search) = &self.search {
            window.focus(&search.bar.focus_handle(cx));
            return;
        }
        let selection = self
            .selection_text()
            .filter(|s| !s.contains('\n'))
            .unwrap_or_default();
        let theme = self.theme.clone();
        let bar = cx.new(|cx| SearchBar::new(theme, selection, cx));
        let subscription = cx.subscribe_in(&bar, window, Self::on_search_event);
        window.focus(&bar.focus_handle(cx));
        self.search = Some(SearchState {
            bar,
            regex: None,
            current: None,
            _subscription: subscription,
        });
        self.search_changed(cx);
    }

    fn on_search_event(
        &mut self,
        _bar: &Entity<SearchBar>,
        event: &SearchEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            SearchEvent::Changed => self.search_changed(cx),
            SearchEvent::Up => self.search_step(true, cx),
            SearchEvent::Down => self.search_step(false, cx),
            SearchEvent::Close => {
                self.search = None;
                window.focus(&self.focus);
                cx.notify();
            }
        }
    }

    /// Recompiles the query and jumps to the nearest match above the bottom.
    fn search_changed(&mut self, cx: &mut Context<Self>) {
        let Some(search) = &mut self.search else {
            return;
        };
        search.current = None;
        match search.bar.read(cx).query().compile() {
            Ok(regex) => {
                search.regex = regex;
                self.search_step(true, cx);
            }
            Err(error) => {
                search.regex = None;
                search.bar.update(cx, |bar, cx| bar.set_status(error, cx));
                cx.notify();
            }
        }
    }

    /// Moves to the next match up or down, scrolls it into view and updates
    /// the "n/total" counter.
    fn search_step(&mut self, upward: bool, cx: &mut Context<Self>) {
        let Some(search) = &mut self.search else {
            return;
        };
        let status = match &mut search.regex {
            None => String::new(),
            Some(regex) => {
                let mut term = self.session.term().lock();
                search.current = search::next_match(&term, regex, search.current.as_ref(), upward);
                match &search.current {
                    None => "No results".to_owned(),
                    Some(current) => {
                        term.scroll_to_point(*current.start());
                        let all = search::all_matches(&term, regex, COUNT_LIMIT);
                        let total = if all.len() >= COUNT_LIMIT {
                            format!("{COUNT_LIMIT}+")
                        } else {
                            all.len().to_string()
                        };
                        match all.iter().position(|m| m == current) {
                            Some(i) => format!("{}/{total}", i + 1),
                            None => format!("?/{total}"),
                        }
                    }
                }
            }
        };
        search.bar.update(cx, |bar, cx| bar.set_status(status, cx));
        cx.notify();
    }
}
