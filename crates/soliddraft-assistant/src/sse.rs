//! A server-sent events reader: lines in, `(event, data)` pairs out.
//!
//! Follows the WHATWG event-stream rules the Messages API needs: `event:` and `data:`
//! fields, multi-line data joined with `\n`, comments starting with `:`, a blank line
//! ending the event, and `\n`, `\r\n` or `\r` line ends.

use std::io::BufRead;

/// One event from the stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SseEvent {
    /// The `event:` field, or `message` when absent.
    pub event: String,
    /// The `data:` lines joined with `\n`.
    pub data: String,
}

/// Reads events from a buffered reader until it ends.
pub struct SseReader<R> {
    reader: R,
    line: Vec<u8>,
}

impl<R: BufRead> SseReader<R> {
    /// Wraps a reader.
    pub fn new(reader: R) -> Self {
        Self { reader, line: Vec::new() }
    }

    /// The next event, `Ok(None)` at the end of the stream.
    pub fn next_event(&mut self) -> std::io::Result<Option<SseEvent>> {
        let mut event = String::new();
        let mut data: Option<String> = None;
        loop {
            self.line.clear();
            let n = self.reader.read_until(b'\n', &mut self.line)?;
            if n == 0 {
                // A stream that ends without a blank line still delivers its last event.
                return Ok(
                    data.map(|data| SseEvent { event: if event.is_empty() { "message".into() } else { event }, data })
                );
            }
            let text = String::from_utf8_lossy(&self.line);
            let text = text.trim_end_matches(['\n', '\r']);
            // A lone `\r` inside a chunk also ends a line.
            for line in text.split('\r') {
                if line.is_empty() {
                    if let Some(data) = data.take() {
                        let event = if event.is_empty() { "message".into() } else { std::mem::take(&mut event) };
                        return Ok(Some(SseEvent { event, data }));
                    }
                    event.clear();
                    continue;
                }
                if line.starts_with(':') {
                    continue;
                }
                let (field, value) = match line.split_once(':') {
                    Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
                    None => (line, ""),
                };
                match field {
                    "event" => event = value.to_owned(),
                    "data" => match &mut data {
                        Some(d) => {
                            d.push('\n');
                            d.push_str(value);
                        }
                        None => data = Some(value.to_owned()),
                    },
                    _ => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(text: &str) -> Vec<SseEvent> {
        let mut r = SseReader::new(text.as_bytes());
        let mut out = Vec::new();
        while let Some(e) = r.next_event().unwrap() {
            out.push(e);
        }
        out
    }

    #[test]
    fn parses_events_comments_and_crlf() {
        let events = all(
            "event: ping\ndata: {\"type\":\"ping\"}\n\n: comment\r\nevent: a\r\ndata: x\r\ndata: y\r\n\r\ndata: last",
        );
        assert_eq!(events.len(), 3);
        assert_eq!(events[0], SseEvent { event: "ping".into(), data: "{\"type\":\"ping\"}".into() });
        assert_eq!(events[1], SseEvent { event: "a".into(), data: "x\ny".into() });
        assert_eq!(events[2], SseEvent { event: "message".into(), data: "last".into() });
    }
}
