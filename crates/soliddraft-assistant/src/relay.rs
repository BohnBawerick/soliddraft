//! A provider on another machine: the request goes to a server, which runs it on its own
//! provider and streams the reply back as JSON lines.
//!
//! The web build uses this so the browser never holds an API key: the server adds the key
//! when it calls the model. The lines carry the provider-neutral [`StreamEvent`]s and
//! [`ProviderResponse`], so the relay works with any provider the server runs.

use crate::provider::{Provider, ProviderError, ProviderResponse, StreamEvent};
use crate::wire::ChatRequest;
use serde::{Deserialize, Serialize};

/// One line of a relayed reply. A reply is any number of events, then one `done` or
/// `error`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayLine {
    /// An event as it streamed.
    Event(StreamEvent),
    /// The finished response.
    Done(ProviderResponse),
    /// The request failed.
    Error(ProviderError),
}

impl RelayLine {
    /// The line as JSON text, without the newline.
    pub fn encode(&self) -> String {
        serde_json::to_string(self).expect("relay lines always encode")
    }

    /// Reads one line.
    pub fn decode(line: &str) -> Result<RelayLine, ProviderError> {
        serde_json::from_str(line).map_err(|e| ProviderError::Protocol(format!("bad relay line: {e}")))
    }
}

/// Runs `request` on `provider` and writes each line of the reply with `write`, which
/// returns `false` once the client has gone; the request then stops.
pub fn serve(provider: &dyn Provider, request: &ChatRequest, write: &mut dyn FnMut(&str) -> bool) {
    let result = provider.send(request, &mut |event| write(&RelayLine::Event(event).encode()));
    let last = match result {
        Ok(response) => RelayLine::Done(response),
        Err(e) => RelayLine::Error(e),
    };
    write(&last.encode());
}

/// Splits a relayed reply into lines as its bytes arrive.
#[derive(Debug, Default)]
pub struct LineReader {
    buffer: Vec<u8>,
}

impl LineReader {
    /// Adds bytes and returns every line they complete.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Result<RelayLine, ProviderError>> {
        self.buffer.extend_from_slice(bytes);
        let mut lines = Vec::new();
        while let Some(end) = self.buffer.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buffer.drain(..=end).collect();
            let text = String::from_utf8_lossy(&line[..end]);
            if !text.trim().is_empty() {
                lines.push(RelayLine::decode(text.trim()));
            }
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{MockProvider, MockTurn};
    use crate::wire::Effort;

    fn request() -> ChatRequest {
        ChatRequest {
            system: "system".into(),
            tools: Vec::new(),
            messages: Vec::new(),
            effort: Effort::Medium,
            max_tokens: 1000,
        }
    }

    #[test]
    fn a_relayed_reply_reads_back_whole() {
        let provider = MockProvider::new(vec![MockTurn::text("Two holes, 6 mm.")]);
        let mut wire = String::new();
        serve(&provider, &request(), &mut |line| {
            wire.push_str(line);
            wire.push('\n');
            true
        });

        // Bytes arrive in pieces that cut lines anywhere.
        let mut reader = LineReader::default();
        let mut text = String::new();
        let mut done = None;
        for chunk in wire.as_bytes().chunks(7) {
            for line in reader.push(chunk) {
                match line.unwrap() {
                    RelayLine::Event(StreamEvent::TextDelta(t)) => text.push_str(&t),
                    RelayLine::Event(_) => {}
                    RelayLine::Done(r) => done = Some(r),
                    RelayLine::Error(e) => panic!("{e}"),
                }
            }
        }
        assert_eq!(text, "Two holes, 6 mm.");
        assert_eq!(done.expect("done line").stop_reason.as_deref(), Some("end_turn"));
    }

    #[test]
    fn a_failed_request_relays_its_error() {
        let provider = MockProvider::new(Vec::new());
        let mut lines = Vec::new();
        serve(&provider, &request(), &mut |line| {
            lines.push(line.to_owned());
            true
        });
        assert_eq!(lines.len(), 1);
        assert!(matches!(RelayLine::decode(&lines[0]), Ok(RelayLine::Error(ProviderError::Protocol(_)))));
    }
}
