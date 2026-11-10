use alacritty_terminal::Term;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};

/// Hook between the PTY output and the VT parser. Pass-through by default;
/// the optional kitty-graphics crate (M7) plugs in here.
pub trait OutputFilter: Send {
    /// Consumes `input`, appends bytes meant for the parser to `out`, and
    /// any reply meant for the shell to `pty_reply`.
    fn filter(&mut self, input: &[u8], out: &mut Vec<u8>, pty_reply: &mut Vec<u8>);

    /// Offsets into the `out` of the last [`filter`](Self::filter) call
    /// where the filter needs the screen as parsed up to there. The reader
    /// parses `out` up to each offset in turn and calls
    /// [`at_checkpoint`](Self::at_checkpoint) there.
    fn take_checkpoints(&mut self) -> Vec<usize> {
        Vec::new()
    }

    /// Called at each checkpoint, in order, with the screen parsed up to it.
    fn at_checkpoint(&mut self, _screen: &dyn Screen) {}
}

/// What a filter can see of the terminal: the screen (not the scrollback),
/// in the coordinates programs use.
pub trait Screen {
    /// The cursor's row and column, from 0.
    fn cursor(&self) -> (usize, usize);
    /// Calls `f(uri, row, column)` for each screen cell carrying a hyperlink.
    fn for_each_link(&self, f: &mut dyn FnMut(&str, usize, usize));
}

impl<T> Screen for Term<T> {
    fn cursor(&self) -> (usize, usize) {
        let point = self.grid().cursor.point;
        (point.line.0.max(0) as usize, point.column.0)
    }

    fn for_each_link(&self, f: &mut dyn FnMut(&str, usize, usize)) {
        let grid = self.grid();
        for row in 0..grid.screen_lines() {
            for col in 0..grid.columns() {
                if let Some(link) = grid[Point::new(Line(row as i32), Column(col))].hyperlink() {
                    f(link.uri(), row, col);
                }
            }
        }
    }
}

/// The default filter: forwards everything unchanged.
#[derive(Debug, Default)]
pub struct PassThrough;

impl OutputFilter for PassThrough {
    fn filter(&mut self, input: &[u8], out: &mut Vec<u8>, _pty_reply: &mut Vec<u8>) {
        out.extend_from_slice(input);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pass_through_is_identity() {
        let (mut out, mut reply) = (Vec::new(), Vec::new());
        PassThrough.filter(b"\x1b[31mhi", &mut out, &mut reply);
        assert_eq!(out, b"\x1b[31mhi");
        assert!(reply.is_empty());
    }
}
