// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! What the tap makes of tshark's lines (spec 0375 S6): which are gRPC
//! messages, which call each belongs to, and which messages it missed.
//! Pure, so it is tested on its own; `main.rs` does the input and output.

use std::collections::{HashMap, HashSet};

/// The fields the tap asks tshark for, in order. tshark prints every
/// occurrence of a field joined by commas (its default), and the fields
/// separated by `|`, which no value of these fields contains.
pub const FIELDS: &[&str] = &[
    "frame.time_epoch",
    "tcp.stream",
    "tcp.dstport",
    "http2.streamid",
    "http2.magic",
    "http2.headers.path",
    "grpc.message_data",
];

/// Asked for as well with `--proto-path`, when tshark knows the types.
pub const PROTO_FIELDS: &[&str] = &["protobuf.message.name", "protobuf.field.name"];

pub const SEPARATOR: char = '|';

/// The packets tshark reports: a gRPC message; the HTTP/2 preface, which
/// marks a connection seen from its start; a request's headers, for the
/// method path; and HTTP/2 DATA that tshark could not attribute to gRPC —
/// on a connection seen from its start only a part of a larger message,
/// and otherwise a message the tap missed.
pub const FILTER: &str =
    "grpc.message_data || http2.magic || http2.headers.path || (http2.type == 0 && !grpc)";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dir {
    Request,
    Response,
}

impl Dir {
    pub fn name(self) -> &'static str {
        match self {
            Dir::Request => "request",
            Dir::Response => "response",
        }
    }

    pub fn arrow(self) -> &'static str {
        match self {
            Dir::Request => "→",
            Dir::Response => "←",
        }
    }
}

/// One gRPC message, as the tap saves and reports it.
#[derive(Debug, PartialEq)]
pub struct Message {
    /// The call's number, shared by its request and its response; 0 for a
    /// response whose request the tap did not see.
    pub call: u64,
    pub dir: Dir,
    /// Seconds since the epoch, as tshark gives it.
    pub time: f64,
    pub path: Option<String>,
    pub bytes: Vec<u8>,
    /// With `--proto-path`: the message's type and the fields it holds,
    /// each named once.
    pub detail: Option<String>,
}

impl Message {
    pub fn file_name(&self) -> String {
        format!("{:06}-{}.pb", self.call, self.dir.name())
    }
}

/// What one line of tshark's output gave.
#[derive(Debug, Default, PartialEq)]
pub struct Fed {
    pub messages: Vec<Message>,
    /// The first message missed: the tap says why, once.
    pub first_missed: bool,
}

/// The tap's bookkeeping across tshark's lines.
pub struct Tracker {
    port: u16,
    next: u64,
    calls: HashMap<(u64, u32), u64>,
    paths: HashMap<(u64, u32), String>,
    seen_from_start: HashSet<u64>,
    missed: HashSet<(u64, u32, Dir)>,
    pub requests: u64,
    pub responses: u64,
}

impl Tracker {
    /// `next` is the number the first call takes.
    pub fn new(port: u16, next: u64) -> Self {
        Tracker {
            port,
            next,
            calls: HashMap::new(),
            paths: HashMap::new(),
            seen_from_start: HashSet::new(),
            missed: HashSet::new(),
            requests: 0,
            responses: 0,
        }
    }

    pub fn missed(&self) -> usize {
        self.missed.len()
    }

    pub fn feed(&mut self, line: &str) -> Result<Fed, String> {
        let f: Vec<&str> = line.split(SEPARATOR).collect();
        if f.len() < FIELDS.len() {
            return Err(format!("{} fields, expected {}", f.len(), FIELDS.len()));
        }
        let field = |i: usize| f[i];
        let time: f64 = field(0).parse().map_err(|e| format!("time: {e}"))?;
        let tcp: u64 = field(1).parse().map_err(|e| format!("tcp.stream: {e}"))?;
        let dport: u16 = field(2).parse().map_err(|e| format!("tcp.dstport: {e}"))?;
        // The last non-zero stream id: the packet's other frames are
        // connection-level (stream 0).
        let stream = field(3)
            .split(',')
            .filter_map(|s| s.parse::<u32>().ok())
            .rfind(|&s| s != 0)
            .unwrap_or(0);
        let dir = if dport == self.port {
            Dir::Request
        } else {
            Dir::Response
        };
        let key = (tcp, stream);
        if !field(4).is_empty() {
            self.seen_from_start.insert(tcp);
        }
        if let Some(path) = field(5).split(',').rfind(|p| !p.is_empty()) {
            self.paths.insert(key, path.to_string());
        }

        let mut fed = Fed::default();
        let data = field(6);
        if data.is_empty() {
            if !self.seen_from_start.contains(&tcp) && self.missed.insert((tcp, stream, dir)) {
                fed.first_missed = self.missed.len() == 1;
            }
            return Ok(fed);
        }

        let detail = f.get(7).filter(|n| !n.is_empty()).map(|names| {
            let mut seen = HashSet::new();
            let fields: Vec<&str> = f
                .get(8)
                .map_or("", |v| v)
                .split(',')
                .filter(|n| !n.is_empty() && seen.insert(*n))
                .collect();
            format!(
                "{}: {}",
                names.split(',').next().unwrap_or(""),
                fields.join(" ")
            )
        });
        for hex in data.split(',') {
            let call = match dir {
                Dir::Request => {
                    let call = self.next;
                    self.next += 1;
                    self.calls.insert(key, call);
                    self.requests += 1;
                    call
                }
                Dir::Response => {
                    self.responses += 1;
                    self.calls.get(&key).copied().unwrap_or(0)
                }
            };
            fed.messages.push(Message {
                call,
                dir,
                time,
                path: self.paths.get(&key).cloned(),
                bytes: unhex(hex)?,
                detail: detail.clone(),
            });
        }
        Ok(fed)
    }
}

/// tshark's hex rendering of a message back to its bytes.
pub fn unhex(hex: &str) -> Result<Vec<u8>, String> {
    if !hex.is_ascii() {
        return Err("hex with non-ASCII characters".to_string());
    }
    if !hex.len().is_multiple_of(2) {
        return Err(format!("odd-length hex ({} digits)", hex.len()));
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|e| format!("hex at {i}: {e}")))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PORT: u16 = 50051;

    /// A line as tshark prints it: time, tcp.stream, dstport, stream ids,
    /// magic, path, data.
    fn line(tcp: u64, dport: u16, sids: &str, magic: &str, path: &str, data: &str) -> String {
        format!("1790789819.803123456|{tcp}|{dport}|{sids}|{magic}|{path}|{data}")
    }

    const MAGIC: &str = r"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";
    const PATH: &str = "/grehack.life.v1.Life/Step";

    #[test]
    fn a_request_and_its_response_share_a_number() {
        let mut t = Tracker::new(PORT, 1);
        t.feed(&line(0, PORT, "", MAGIC, "", "")).unwrap();
        t.feed(&line(0, PORT, "0,0,1", "", PATH, "")).unwrap();
        let req = t.feed(&line(0, PORT, "0,1,1", "", "", "0a00")).unwrap();
        let resp = t.feed(&line(0, 40000, "1,1,1", "", "", "1001")).unwrap();
        let req = &req.messages[0];
        let resp = &resp.messages[0];
        assert_eq!((req.call, req.dir), (1, Dir::Request));
        assert_eq!((resp.call, resp.dir), (1, Dir::Response));
        assert_eq!(req.path.as_deref(), Some(PATH));
        assert_eq!(resp.path.as_deref(), Some(PATH));
        assert_eq!(req.bytes, [0x0a, 0x00]);
        assert_eq!(req.file_name(), "000001-request.pb");
        assert_eq!(resp.file_name(), "000001-response.pb");
        // The next call on the same connection takes the next number.
        t.feed(&line(0, PORT, "3", "", PATH, "")).unwrap();
        let next = t.feed(&line(0, PORT, "3,3", "", "", "0a00")).unwrap();
        assert_eq!(next.messages[0].call, 2);
        assert_eq!((t.requests, t.responses, t.missed()), (2, 1, 0));
    }

    #[test]
    fn numbering_continues_from_the_first_number() {
        let mut t = Tracker::new(PORT, 42);
        t.feed(&line(0, PORT, "", MAGIC, "", "")).unwrap();
        let fed = t.feed(&line(0, PORT, "1", "", "", "00")).unwrap();
        assert_eq!(fed.messages[0].file_name(), "000042-request.pb");
    }

    #[test]
    fn data_on_a_connection_seen_whole_is_not_missed() {
        // Part of a large message: DATA that is not (yet) gRPC.
        let mut t = Tracker::new(PORT, 1);
        t.feed(&line(0, PORT, "", MAGIC, "", "")).unwrap();
        let fed = t.feed(&line(0, PORT, "1", "", "", "")).unwrap();
        assert_eq!(fed, Fed::default());
        assert_eq!(t.missed(), 0);
    }

    #[test]
    fn a_connection_older_than_the_tap_is_missed_once_per_message() {
        let mut t = Tracker::new(PORT, 1);
        // Two frames of one request, then its response.
        let first = t.feed(&line(7, PORT, "5", "", "", "")).unwrap();
        let again = t.feed(&line(7, PORT, "5", "", "", "")).unwrap();
        let resp = t.feed(&line(7, 40000, "5", "", "", "")).unwrap();
        assert!(first.first_missed);
        assert!(!again.first_missed && !resp.first_missed);
        assert_eq!(t.missed(), 2, "one request, one response");
    }

    #[test]
    fn several_messages_in_one_packet_make_several_files() {
        let mut t = Tracker::new(PORT, 1);
        t.feed(&line(0, PORT, "", MAGIC, "", "")).unwrap();
        let fed = t.feed(&line(0, PORT, "1", "", "", "0a00,0b00")).unwrap();
        let calls: Vec<u64> = fed.messages.iter().map(|m| m.call).collect();
        assert_eq!(calls, [1, 2]);
    }

    #[test]
    fn a_response_without_its_request_is_call_zero() {
        let mut t = Tracker::new(PORT, 1);
        t.feed(&line(0, PORT, "", MAGIC, "", "")).unwrap();
        let fed = t.feed(&line(0, 40000, "9", "", "", "00")).unwrap();
        assert_eq!(fed.messages[0].file_name(), "000000-response.pb");
    }

    #[test]
    fn proto_names_become_a_detail() {
        let mut t = Tracker::new(PORT, 1);
        t.feed(&line(0, PORT, "", MAGIC, "", "")).unwrap();
        let with_names = format!(
            "{}|grehack.life.v1.StepRequest,grehack.life.v1.Grid|grid,rows,cells,rows,cells,rules",
            line(0, PORT, "1", "", "", "00")
        );
        let fed = t.feed(&with_names).unwrap();
        assert_eq!(
            fed.messages[0].detail.as_deref(),
            Some("grehack.life.v1.StepRequest: grid rows cells rules")
        );
    }

    #[test]
    fn a_short_line_is_an_error() {
        assert!(Tracker::new(PORT, 1).feed("1|2|3").is_err());
    }

    #[test]
    fn unhex_keeps_every_byte() {
        assert_eq!(unhex("00ff0a41").unwrap(), [0x00, 0xff, 0x0a, 0x41]);
        assert!(unhex("abc").is_err());
        assert!(unhex("zz").is_err());
    }
}
