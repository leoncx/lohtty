/// Hook between the PTY output and the VT parser. Pass-through by default;
/// the optional kitty-graphics crate (M7) plugs in here.
pub trait OutputFilter: Send {
    /// Consumes `input`, appends bytes meant for the parser to `out`, and
    /// any reply meant for the shell to `pty_reply`.
    fn filter(&mut self, input: &[u8], out: &mut Vec<u8>, pty_reply: &mut Vec<u8>);
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
