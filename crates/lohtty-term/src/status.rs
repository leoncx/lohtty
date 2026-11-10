//! The Program Status Protocol (OSC 7501): programs such as coding agents
//! report whether they're working, done, blocked on the user or failed.
//! See <https://www.superlogical.com/rex/docs/build/program-status>.
//!
//! A report is `OSC 7501 ; key=value:key=value… ST`. A program checks for
//! support with `OSC 7501 ; ? ST`, answered with [`QUERY_REPLY`]. Each report
//! replaces one record (by `id`; none is the root record) in the pane's
//! [`StatusBoard`]. A report that breaks any rule or limit is ignored whole.

use std::collections::BTreeMap;

/// The reply to a support query (`OSC 7501 ; ? ST`).
pub const QUERY_REPLY: &[u8] = b"\x1b]7501;?\x1b\\";

/// Whole sequence (OSC through ST); the body is a little shorter.
const MAX_SEQUENCE: usize = 4096;
const MAX_KEY: usize = 16;
const MAX_MSG_ENCODED: usize = 2732;
const MAX_TITLE_ENCODED: usize = 256;
const MAX_APP: usize = 32;
const MAX_ID: usize = 128;
const MAX_SEGMENT: usize = 32;
const MAX_DEPTH: usize = 8;
/// Records kept per terminal; the least recently updated goes first.
const MAX_RECORDS: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum State {
    Idle,
    Working,
    Done,
    Blocked,
    Error,
}

impl State {
    /// How much a state calls for the user, for choosing what to show.
    pub fn urgency(self) -> u8 {
        match self {
            State::Idle => 0,
            State::Done => 1,
            State::Working => 2,
            State::Error => 3,
            State::Blocked => 4,
        }
    }
}

/// Why a program is blocked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Permission,
    Question,
    Auth,
}

/// One record: what the last report for its id said.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub state: State,
    pub kind: Option<Kind>,
    /// 0–100; absent means indeterminate.
    pub progress: Option<u8>,
    pub app: Option<String>,
    pub title: Option<String>,
    pub msg: Option<String>,
}

/// What an OSC 7501 sequence asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    Query,
    /// Sets the record `id` ("" is the root record).
    Set {
        id: String,
        record: Record,
    },
    /// Removes the record `id` and those beneath it; `None` removes all.
    Clear {
        id: Option<String>,
    },
}

/// Parses the body of an OSC 7501 sequence (after `7501;`). `None` if it's
/// to be ignored.
pub fn parse(body: &str) -> Option<Message> {
    // The whole sequence is `ESC ] 7501 ; body ESC \`.
    if body.len() + 9 > MAX_SEQUENCE {
        return None;
    }
    if body.trim() == "?" {
        return Some(Message::Query);
    }
    let mut pairs: BTreeMap<&str, &str> = BTreeMap::new();
    for pair in body.split(':') {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        let valid_key =
            !key.is_empty() && key.len() <= MAX_KEY && key.bytes().all(|b| b.is_ascii_lowercase());
        let valid_value = value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.,+/=-".contains(&b));
        if valid_key && valid_value {
            // The last value of a repeated key wins.
            pairs.insert(key, value);
        }
    }
    let id = match pairs.get("id") {
        Some(id) => Some(valid_id(id)?.to_owned()),
        None => None,
    };
    let state = match *pairs.get("state")? {
        "idle" => State::Idle,
        "working" => State::Working,
        "done" => State::Done,
        "blocked" => State::Blocked,
        "error" => State::Error,
        "clear" => return Some(Message::Clear { id }),
        _ => return None,
    };
    let kind = match pairs.get("kind") {
        Some(&"permission") => Some(Kind::Permission),
        Some(&"question") => Some(Kind::Question),
        Some(&"auth") => Some(Kind::Auth),
        _ => None,
    }
    .filter(|_| state == State::Blocked);
    let progress = pairs
        .get("progress")
        .and_then(|p| p.parse::<u8>().ok())
        .filter(|p| *p <= 100 && matches!(state, State::Working | State::Blocked));
    let app = pairs
        .get("app")
        .filter(|a| (1..=MAX_APP).contains(&a.len()) && a.bytes().all(id_byte))
        .map(|a| a.to_string());
    let title = text(pairs.get("title"), MAX_TITLE_ENCODED)?;
    let msg = text(pairs.get("msg"), MAX_MSG_ENCODED)?;
    Some(Message::Set {
        id: id.unwrap_or_default(),
        record: Record {
            state,
            kind,
            progress,
            app,
            title,
            msg,
        },
    })
}

fn id_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"_.+-".contains(&b)
}

fn valid_id(id: &str) -> Option<&str> {
    let ok = id.len() <= MAX_ID
        && id.split('/').count() <= MAX_DEPTH
        && id
            .split('/')
            .all(|s| (1..=MAX_SEGMENT).contains(&s.len()) && s.bytes().all(id_byte));
    ok.then_some(id)
}

/// Decodes a base64 text value. `Some(None)` when absent; `None` (the whole
/// report is ignored) when too long, not base64, not UTF-8, or containing
/// control characters.
fn text(value: Option<&&str>, max_encoded: usize) -> Option<Option<String>> {
    let Some(value) = value else {
        return Some(None);
    };
    if value.len() > max_encoded {
        return None;
    }
    let bytes = crate::notify::decode_base64(value)?;
    let text = String::from_utf8(bytes).ok()?;
    if text.chars().any(char::is_control) {
        return None;
    }
    Some(Some(text))
}

/// Progress reported with OSC 9;4 (ConEmu, Windows Terminal):
/// `ESC ] 9 ; 4 ; state ; value ST`, value 0–100.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Progress {
    /// State 0: no progress.
    Clear,
    /// State 1.
    Normal(Option<u8>),
    /// State 2.
    Error,
    /// State 3.
    Indeterminate,
    /// State 4 (paused or warning).
    Paused(Option<u8>),
}

/// Parses the part of an OSC 9 after `9;`, if it's progress (`4;…`).
pub fn parse_progress(rest: &str) -> Option<Progress> {
    let mut parts = rest.split(';');
    if parts.next()? != "4" {
        return None;
    }
    let state = parts.next().unwrap_or("0");
    let value = parts
        .next()
        .and_then(|v| v.trim().parse::<u32>().ok())
        .map(|v| v.min(100) as u8);
    Some(match state.trim() {
        "" | "0" => Progress::Clear,
        "1" => Progress::Normal(value),
        "2" => Progress::Error,
        "3" => Progress::Indeterminate,
        "4" => Progress::Paused(value),
        _ => return None,
    })
}

/// A pane's status records.
#[derive(Default, Debug)]
pub struct StatusBoard {
    /// Records by id, with the update counter of their last report.
    records: BTreeMap<String, (Record, u64)>,
    updates: u64,
    /// An OSC 7501 report arrived: OSC 9;4 progress is ignored from then
    /// on, until a full reset, as the protocol asks.
    native: bool,
}

/// A record as shown: its id, and its app inherited from its ancestors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shown {
    pub id: String,
    pub record: Record,
}

impl StatusBoard {
    /// Applies a report; returns whether anything changed.
    pub fn apply(&mut self, message: Message) -> bool {
        match message {
            Message::Query => false,
            Message::Set { id, record } => {
                self.native = true;
                self.set(id, record)
            }
            Message::Clear { id: None } => {
                self.native = true;
                self.remove(|_| true)
            }
            Message::Clear { id: Some(id) } => {
                self.native = true;
                let below = format!("{id}/");
                self.remove(|r| *r == id || r.starts_with(&below))
            }
        }
    }

    /// Applies OSC 9;4 progress to the root record (unless the program also
    /// speaks OSC 7501). Paused progress is still work in progress.
    pub fn progress(&mut self, progress: Progress) -> bool {
        if self.native {
            return false;
        }
        let record = |state, progress| Record {
            state,
            kind: None,
            progress,
            app: None,
            title: None,
            msg: None,
        };
        match progress {
            Progress::Clear => self.records.remove("").is_some(),
            Progress::Normal(value) | Progress::Paused(value) => {
                self.set(String::new(), record(State::Working, value))
            }
            Progress::Indeterminate => self.set(String::new(), record(State::Working, None)),
            Progress::Error => self.set(String::new(), record(State::Error, None)),
        }
    }

    /// Replaces one record, evicting the least recently updated when full.
    fn set(&mut self, id: String, record: Record) -> bool {
        self.updates += 1;
        let changed = self.records.get(&id).is_none_or(|(r, _)| *r != record);
        self.records.insert(id, (record, self.updates));
        if self.records.len() > MAX_RECORDS
            && let Some(oldest) = self
                .records
                .iter()
                .min_by_key(|(_, (_, at))| *at)
                .map(|(id, _)| id.clone())
        {
            self.records.remove(&oldest);
        }
        changed
    }

    fn remove(&mut self, doomed: impl Fn(&String) -> bool) -> bool {
        let before = self.records.len();
        self.records.retain(|id, _| !doomed(id));
        self.records.len() != before
    }

    /// A new shell prompt started (OSC 133 A) or the program exited: work
    /// in progress is over, results stay.
    pub fn prompt(&mut self) -> bool {
        let before = self.records.len();
        self.records
            .retain(|_, (r, _)| matches!(r.state, State::Done | State::Error));
        self.records.len() != before
    }

    /// A full reset (RIS) removes everything.
    pub fn reset(&mut self) -> bool {
        self.native = false;
        self.remove(|_| true)
    }

    /// The user saw the pane: finished results no longer need showing.
    pub fn seen(&mut self) -> bool {
        let before = self.records.len();
        self.records
            .retain(|_, (r, _)| !matches!(r.state, State::Done | State::Error));
        self.records.len() != before
    }

    /// The records, with inherited apps, most urgent first (then the most
    /// recently updated).
    pub fn shown(&self) -> Vec<Shown> {
        let mut shown: Vec<(Shown, u64)> = self
            .records
            .iter()
            .map(|(id, (record, at))| {
                let mut record = record.clone();
                if record.app.is_none() {
                    record.app = self.inherited_app(id);
                }
                (
                    Shown {
                        id: id.clone(),
                        record,
                    },
                    *at,
                )
            })
            .collect();
        shown.sort_by_key(|(s, at)| {
            (
                std::cmp::Reverse(s.record.state.urgency()),
                std::cmp::Reverse(*at),
            )
        });
        shown.into_iter().map(|(s, _)| s).collect()
    }

    /// The app of the nearest ancestor that has one (the root counts).
    fn inherited_app(&self, id: &str) -> Option<String> {
        let mut path = id;
        while !path.is_empty() {
            path = path.rsplit_once('/').map_or("", |(parent, _)| parent);
            if let Some((r, _)) = self.records.get(path)
                && r.app.is_some()
            {
                return r.app.clone();
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(body: &str) -> Record {
        match parse(body) {
            Some(Message::Set { record, .. }) => record,
            other => panic!("{body}: {other:?}"),
        }
    }

    #[test]
    fn query_and_reports() {
        assert_eq!(parse("?"), Some(Message::Query));
        // "Installing updates", "Password required to install updates".
        let r = set("state=working:app=brew:msg=SW5zdGFsbGluZyB1cGRhdGVz");
        assert_eq!(r.state, State::Working);
        assert_eq!(r.app.as_deref(), Some("brew"));
        assert_eq!(r.msg.as_deref(), Some("Installing updates"));
        let r = set(
            "state=blocked:kind=auth:app=brew:msg=UGFzc3dvcmQgcmVxdWlyZWQgdG8gaW5zdGFsbCB1cGRhdGVz",
        );
        assert_eq!((r.state, r.kind), (State::Blocked, Some(Kind::Auth)));
        let r = set(" state = working : progress=40 : title=VVMgRWFzdA== ");
        assert_eq!(
            (r.progress, r.title.as_deref()),
            (Some(40), Some("US East"))
        );
        assert_eq!(
            parse("state=clear:id=us-east"),
            Some(Message::Clear {
                id: Some("us-east".into())
            })
        );
        assert_eq!(parse("state=clear"), Some(Message::Clear { id: None }));
    }

    #[test]
    fn rules_and_limits() {
        assert_eq!(parse("app=x"), None, "state is required");
        assert_eq!(parse("state=sleeping"), None);
        assert_eq!(parse("state=done:id=a//b"), None, "malformed id");
        assert_eq!(
            parse(&format!("state=done:id={}", "a/".repeat(9) + "a")),
            None,
            "too deep"
        );
        // A control character in a decoded text ("a\tb") discards the report.
        assert_eq!(parse("state=done:msg=YQli"), None);
        assert_eq!(
            parse("state=done:msg=@@@"),
            Some(Message::Set {
                id: String::new(),
                record: set("state=done"),
            }),
            "a value with a forbidden byte is skipped"
        );
        assert_eq!(parse(&format!("state=done:msg={}", "A".repeat(2736))), None);
        // Fields that don't apply are dropped.
        let r = set("state=done:kind=auth:progress=50:app=bad!app");
        assert_eq!((r.kind, r.progress, r.app), (None, None, None));
        assert_eq!(set("state=working:progress=101").progress, None);
        // The last value of a repeated key wins; unknown keys are ignored.
        assert_eq!(set("state=idle:state=done:color=red").state, State::Done);
    }

    fn apply(board: &mut StatusBoard, body: &str) -> bool {
        board.apply(parse(body).unwrap())
    }

    #[test]
    fn board_records_clears_and_lifetimes() {
        let mut b = StatusBoard::default();
        assert!(apply(&mut b, "state=working:app=cargo"));
        assert!(
            !apply(&mut b, "state=working:app=cargo"),
            "same report, no change"
        );
        apply(&mut b, "state=working:id=build/test");
        apply(&mut b, "state=done:id=build/doc");
        // The child inherits the root's app.
        let shown = b.shown();
        let test = shown.iter().find(|s| s.id == "build/test").unwrap();
        assert_eq!(test.record.app.as_deref(), Some("cargo"));
        // Clearing an id removes its subtree.
        apply(&mut b, "state=working:id=build");
        assert!(apply(&mut b, "state=clear:id=build"));
        assert_eq!(b.shown().len(), 1);
        // A prompt drops work in progress but keeps results.
        apply(&mut b, "state=error:id=deploy");
        assert!(b.prompt());
        assert_eq!(
            b.shown().iter().map(|s| s.record.state).collect::<Vec<_>>(),
            [State::Error]
        );
        // Seeing the pane drops results.
        assert!(b.seen());
        assert!(b.shown().is_empty());
    }

    #[test]
    fn osc9_4_progress() {
        assert_eq!(parse_progress("4;1;50"), Some(Progress::Normal(Some(50))));
        assert_eq!(parse_progress("4;1"), Some(Progress::Normal(None)));
        assert_eq!(parse_progress("4;1;250"), Some(Progress::Normal(Some(100))));
        assert_eq!(parse_progress("4;0;0"), Some(Progress::Clear));
        assert_eq!(parse_progress("4"), Some(Progress::Clear));
        assert_eq!(parse_progress("4;2"), Some(Progress::Error));
        assert_eq!(parse_progress("4;3"), Some(Progress::Indeterminate));
        assert_eq!(parse_progress("4;4;80"), Some(Progress::Paused(Some(80))));
        assert_eq!(parse_progress("4;9"), None);
        assert_eq!(parse_progress("9;C:\\"), None, "another ConEmu command");

        let mut b = StatusBoard::default();
        assert!(b.progress(Progress::Normal(Some(30))));
        let root = &b.shown()[0];
        assert_eq!(
            (root.id.as_str(), root.record.state, root.record.progress),
            ("", State::Working, Some(30))
        );
        assert!(b.progress(Progress::Error));
        assert_eq!(b.shown()[0].record.state, State::Error);
        assert!(b.progress(Progress::Clear));
        assert!(b.shown().is_empty());
        // Once the program speaks OSC 7501, OSC 9;4 is ignored until a reset.
        apply(&mut b, "state=working:app=claude-code");
        assert!(!b.progress(Progress::Normal(Some(10))));
        assert_eq!(b.shown()[0].record.progress, None);
        b.reset();
        assert!(b.progress(Progress::Indeterminate));
    }

    #[test]
    fn most_urgent_first_and_bounded() {
        let mut b = StatusBoard::default();
        apply(&mut b, "state=done:id=a");
        apply(&mut b, "state=blocked:kind=permission:id=b");
        apply(&mut b, "state=working:id=c");
        let order: Vec<State> = b.shown().iter().map(|s| s.record.state).collect();
        assert_eq!(order, [State::Blocked, State::Working, State::Done]);
        for i in 0..300 {
            apply(&mut b, &format!("state=idle:id=r{i}"));
        }
        assert_eq!(b.shown().len(), MAX_RECORDS);
        assert!(b.reset());
        assert!(b.shown().is_empty());
    }
}
