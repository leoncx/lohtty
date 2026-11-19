//! Searching the terminal's text (screen and scrollback).

use alacritty_terminal::Term;
use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Boundary, Column, Direction, Point, Side};
use alacritty_terminal::term::search::RegexIter;
pub use alacritty_terminal::term::search::{Match, RegexSearch};

/// What the user typed in the search bar.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchQuery {
    pub text: String,
    pub case_sensitive: bool,
    /// Interpret `text` as a regular expression instead of literal text.
    pub regex: bool,
}

impl SearchQuery {
    /// Compiles the query. `Ok(None)` for an empty query; `Err` for an
    /// invalid regex (with a short message).
    pub fn compile(&self) -> Result<Option<RegexSearch>, String> {
        if self.text.is_empty() {
            return Ok(None);
        }
        let pattern = if self.regex {
            self.text.clone()
        } else {
            escape(&self.text)
        };
        // Explicit flag: overrides alacritty's smart-case default.
        let flag = if self.case_sensitive { "(?-i)" } else { "(?i)" };
        RegexSearch::new(&format!("{flag}{pattern}"))
            .map(Some)
            .map_err(|e| {
                let msg = e.to_string();
                msg.lines().last().unwrap_or(&msg).trim().to_owned()
            })
    }
}

/// Escapes regex metacharacters so `text` matches literally.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if "\\.+*?()|[]{}^$#&-~".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Every match in the scrollback and screen, top to bottom (at most `limit`).
pub fn all_matches<T>(term: &Term<T>, regex: &mut RegexSearch, limit: usize) -> Vec<Match> {
    let start = Point::new(term.topmost_line(), Column(0));
    let end = Point::new(term.bottommost_line(), term.last_column());
    RegexIter::new(start, end, Direction::Right, term, regex)
        .take(limit)
        .collect()
}

/// Matches intersecting the lines `first..=last` (viewport coordinates
/// already shifted by the display offset).
pub fn matches_in_lines<T>(
    term: &Term<T>,
    regex: &mut RegexSearch,
    first: Point,
    last: Point,
) -> Vec<Match> {
    // Start a little earlier so matches wrapping in from above are found,
    // stopping at the top of the scrollback: wrapping around to the bottom
    // would skip everything on screen when there is no history.
    let start = first.sub(term, Boundary::Grid, term.columns());
    RegexIter::new(start, last, Direction::Right, term, regex)
        .filter(|m| *m.end() >= first)
        .collect()
}

/// The next match above (`upward`) or below `from`, wrapping around the
/// buffer. With no current match, searches upward from the bottom of the screen.
pub fn next_match<T: EventListener>(
    term: &Term<T>,
    regex: &mut RegexSearch,
    from: Option<&Match>,
    upward: bool,
) -> Option<Match> {
    let bottom_right = Point::new(term.bottommost_line(), term.last_column());
    let (origin, direction, side) = match (from, upward) {
        (Some(m), true) => (
            m.start().sub(term, Boundary::None, 1),
            Direction::Left,
            Side::Right,
        ),
        (Some(m), false) => (
            m.end().add(term, Boundary::None, 1),
            Direction::Right,
            Side::Left,
        ),
        (None, true) => (bottom_right, Direction::Left, Side::Right),
        (None, false) => (
            Point::new(term.topmost_line(), Column(0)),
            Direction::Right,
            Side::Left,
        ),
    };
    term.search_next(regex, origin, direction, side, None)
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::index::Line;
    use alacritty_terminal::term::test::mock_term;

    use super::*;

    fn query(text: &str) -> SearchQuery {
        SearchQuery {
            text: text.into(),
            ..Default::default()
        }
    }

    #[test]
    fn empty_query_compiles_to_nothing() {
        assert!(query("").compile().unwrap().is_none());
    }

    #[test]
    fn invalid_regex_is_an_error_but_literal_is_fine() {
        let bad = SearchQuery {
            text: "a(".into(),
            regex: true,
            ..Default::default()
        };
        assert!(bad.compile().is_err());
        assert!(query("a(").compile().unwrap().is_some());
    }

    #[test]
    fn literal_search_escapes_metacharacters() {
        let term = mock_term("a.b axb\r\na.b");
        let mut regex = query("a.b").compile().unwrap().unwrap();
        assert_eq!(all_matches(&term, &mut regex, 100).len(), 2);
        let mut as_regex = SearchQuery {
            regex: true,
            ..query("a.b")
        }
        .compile()
        .unwrap()
        .unwrap();
        assert_eq!(all_matches(&term, &mut as_regex, 100).len(), 3);
    }

    #[test]
    fn case_toggle() {
        let term = mock_term("Foo foo FOO");
        let mut insensitive = query("foo").compile().unwrap().unwrap();
        assert_eq!(all_matches(&term, &mut insensitive, 100).len(), 3);
        let mut sensitive = SearchQuery {
            case_sensitive: true,
            ..query("foo")
        }
        .compile()
        .unwrap()
        .unwrap();
        assert_eq!(all_matches(&term, &mut sensitive, 100).len(), 1);
        // Uppercase in the query doesn't force case sensitivity when off.
        let mut upper = query("FOO").compile().unwrap().unwrap();
        assert_eq!(all_matches(&term, &mut upper, 100).len(), 3);
    }

    #[test]
    fn navigation_walks_up_and_wraps() {
        let term = mock_term("x1\r\nx2\r\nx3");
        let mut regex = query("x").compile().unwrap().unwrap();
        let line = |m: &Match| m.start().line;

        let first = next_match(&term, &mut regex, None, true).unwrap();
        assert_eq!(line(&first), Line(2));
        let second = next_match(&term, &mut regex, Some(&first), true).unwrap();
        assert_eq!(line(&second), Line(1));
        let third = next_match(&term, &mut regex, Some(&second), true).unwrap();
        assert_eq!(line(&third), Line(0));
        let wrapped = next_match(&term, &mut regex, Some(&third), true).unwrap();
        assert_eq!(line(&wrapped), Line(2));
        let down = next_match(&term, &mut regex, Some(&third), false).unwrap();
        assert_eq!(line(&down), Line(1));
    }

    #[test]
    fn matches_in_lines_limits_to_range() {
        let term = mock_term("x1\r\nx2\r\nx3");
        let mut regex = query("x").compile().unwrap().unwrap();
        let found = matches_in_lines(
            &term,
            &mut regex,
            Point::new(Line(1), Column(0)),
            Point::new(Line(1), term.last_column()),
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].start().line, Line(1));
    }

    #[test]
    fn matches_in_lines_from_the_top_without_history() {
        // No scrollback: the range starts at the topmost line.
        let term = mock_term("x1\r\nx2\r\nx3");
        let mut regex = query("x").compile().unwrap().unwrap();
        let found = matches_in_lines(
            &term,
            &mut regex,
            Point::new(Line(0), Column(0)),
            Point::new(Line(2), term.last_column()),
        );
        let lines: Vec<_> = found.iter().map(|m| m.start().line).collect();
        assert_eq!(lines, [Line(0), Line(1), Line(2)]);
    }
}
