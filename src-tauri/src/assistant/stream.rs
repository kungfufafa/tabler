//! Streamed replies: a Server-Sent Events parser (Claude API, OpenAI-compatible servers), a JSON
//! lines parser (Ollama) and the per-provider decoders turning their events into text deltas,
//! a thinking marker, notices, the stop reason or a failure. Pure: fed with byte chunks as they
//! arrive (a chunk may end anywhere, even inside a UTF-8 sequence).

use serde_json::Value as Json;

use crate::types::assistant::{AssistantFailure, AssistantFailureKind, AssistantProvider};

/// One SSE event (`event:` name and the joined `data:` lines).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

/// Splits a byte stream into complete lines (LF or CRLF), keeping a partial line for later.
#[derive(Debug, Default)]
struct Lines {
    buf: Vec<u8>,
}

impl Lines {
    fn feed(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some(pos) = self.buf.iter().position(|b| *b == b'\n') {
            let mut line: Vec<u8> = self.buf.drain(..=pos).collect();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            out.push(String::from_utf8_lossy(&line).into_owned());
        }
        out
    }

    /// The last line when the stream ended without a line break.
    fn rest(&mut self) -> Option<String> {
        if self.buf.is_empty() {
            return None;
        }
        let line = String::from_utf8_lossy(&self.buf)
            .trim_end_matches('\r')
            .to_string();
        self.buf.clear();
        Some(line)
    }
}

/// Server-Sent Events parser (WHATWG event-stream rules, minus `id` / `retry`).
#[derive(Debug, Default)]
pub struct SseParser {
    lines: Lines,
    event: Option<String>,
    data: Vec<String>,
}

impl SseParser {
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        let lines = self.lines.feed(chunk);
        let mut out = Vec::new();
        for line in lines {
            self.line(&line, &mut out);
        }
        out
    }

    /// Dispatches what is left when the stream ends.
    pub fn finish(&mut self) -> Vec<SseEvent> {
        let mut out = Vec::new();
        if let Some(line) = self.lines.rest() {
            self.line(&line, &mut out);
        }
        self.line("", &mut out);
        out
    }

    fn line(&mut self, line: &str, out: &mut Vec<SseEvent>) {
        if line.is_empty() {
            if !self.data.is_empty() {
                out.push(SseEvent {
                    event: self.event.take(),
                    data: self.data.join("\n"),
                });
                self.data.clear();
            }
            self.event = None;
            return;
        }
        if line.starts_with(':') {
            return;
        }
        let (field, value) = match line.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (line, ""),
        };
        match field {
            "event" => self.event = Some(value.to_string()),
            "data" => self.data.push(value.to_string()),
            _ => {}
        }
    }
}

/// What a decoded stream produces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamItem {
    /// The model is reasoning before (or between) text.
    Thinking,
    Text(String),
    Notice(String),
    /// The reply is complete; the provider's stop reason.
    Stop(Option<String>),
    Failed(AssistantFailure),
}

fn failure(kind: AssistantFailureKind, message: impl Into<String>) -> StreamItem {
    StreamItem::Failed(AssistantFailure::new(kind, message))
}

/// Failure kind of a Claude API error `type`.
pub fn anthropic_error_kind(kind: &str) -> AssistantFailureKind {
    match kind {
        "authentication_error" | "permission_error" => AssistantFailureKind::Auth,
        "rate_limit_error" => AssistantFailureKind::RateLimit,
        "overloaded_error" | "api_error" | "timeout_error" => AssistantFailureKind::Server,
        "invalid_request_error" | "not_found_error" | "request_too_large" => {
            AssistantFailureKind::BadRequest
        }
        _ => AssistantFailureKind::Other,
    }
}

fn str_at<'a>(v: &'a Json, path: &[&str]) -> Option<&'a str> {
    let mut cur = v;
    for key in path {
        cur = cur.get(key)?;
    }
    cur.as_str()
}

/// Decodes one provider's reply stream.
#[derive(Debug)]
pub struct Decoder {
    provider: AssistantProvider,
    sse: SseParser,
    lines: Lines,
    stop_reason: Option<String>,
    thinking: bool,
    done: bool,
}

impl Decoder {
    pub fn new(provider: AssistantProvider) -> Decoder {
        Decoder {
            provider,
            sse: SseParser::default(),
            lines: Lines::default(),
            stop_reason: None,
            thinking: false,
            done: false,
        }
    }

    /// True once the reply ended (stop or failure).
    pub fn is_done(&self) -> bool {
        self.done
    }

    pub fn feed(&mut self, chunk: &[u8]) -> Vec<StreamItem> {
        let mut out = Vec::new();
        match self.provider {
            AssistantProvider::Ollama => {
                for line in self.lines.feed(chunk) {
                    self.ollama_line(&line, &mut out);
                }
            }
            _ => {
                for event in self.sse.feed(chunk) {
                    self.sse_event(&event, &mut out);
                }
            }
        }
        out
    }

    /// The stream ended: flushes a last partial event and reports a reply that never finished.
    pub fn finish(&mut self) -> Vec<StreamItem> {
        let mut out = Vec::new();
        match self.provider {
            AssistantProvider::Ollama => {
                if let Some(line) = self.lines.rest() {
                    self.ollama_line(&line, &mut out);
                }
            }
            _ => {
                for event in self.sse.finish() {
                    self.sse_event(&event, &mut out);
                }
            }
        }
        if !self.done {
            self.done = true;
            // OpenAI-compatible servers that skip `[DONE]` still sent a finish reason.
            if self.provider == AssistantProvider::Openai && self.stop_reason.is_some() {
                out.push(StreamItem::Stop(self.stop_reason.take()));
            } else {
                out.push(failure(
                    AssistantFailureKind::Network,
                    "The connection closed before the reply finished",
                ));
            }
        }
        out
    }

    fn push(&mut self, item: StreamItem, out: &mut Vec<StreamItem>) {
        if self.done {
            return;
        }
        match &item {
            StreamItem::Thinking => {
                if self.thinking {
                    return;
                }
                self.thinking = true;
            }
            StreamItem::Text(t) if t.is_empty() => return,
            StreamItem::Text(_) => self.thinking = false,
            StreamItem::Stop(_) | StreamItem::Failed(_) => self.done = true,
            StreamItem::Notice(_) => {}
        }
        out.push(item);
    }

    fn sse_event(&mut self, event: &SseEvent, out: &mut Vec<StreamItem>) {
        if self.provider == AssistantProvider::Openai && event.data.trim() == "[DONE]" {
            let reason = self.stop_reason.take();
            self.push(StreamItem::Stop(reason), out);
            return;
        }
        let Ok(json) = serde_json::from_str::<Json>(&event.data) else {
            // Keep-alives and anything else that is not JSON.
            return;
        };
        match self.provider {
            AssistantProvider::Anthropic => self.anthropic(&json, out),
            _ => self.openai(&json, out),
        }
    }

    fn anthropic(&mut self, json: &Json, out: &mut Vec<StreamItem>) {
        match str_at(json, &["type"]).unwrap_or("") {
            "content_block_start" => {
                let block = &json["content_block"];
                match str_at(block, &["type"]).unwrap_or("") {
                    "thinking" | "redacted_thinking" => self.push(StreamItem::Thinking, out),
                    "text" => {
                        let text = str_at(block, &["text"]).unwrap_or("").to_string();
                        self.push(StreamItem::Text(text), out);
                    }
                    "fallback" => {
                        let from = str_at(block, &["from", "model"]).unwrap_or("The model");
                        let to = str_at(block, &["to", "model"]).unwrap_or("another model");
                        self.push(
                            StreamItem::Notice(format!("{from} declined; {to} continued")),
                            out,
                        );
                    }
                    _ => {}
                }
            }
            "content_block_delta" => {
                let delta = &json["delta"];
                match str_at(delta, &["type"]).unwrap_or("") {
                    "text_delta" => {
                        let text = str_at(delta, &["text"]).unwrap_or("").to_string();
                        self.push(StreamItem::Text(text), out);
                    }
                    "thinking_delta" => self.push(StreamItem::Thinking, out),
                    _ => {}
                }
            }
            "message_delta" => {
                if let Some(reason) = str_at(json, &["delta", "stop_reason"]) {
                    self.stop_reason = Some(reason.to_string());
                    if reason == "refusal" {
                        let detail = str_at(json, &["delta", "stop_details", "explanation"])
                            .map(|e| format!(": {e}"))
                            .unwrap_or_default();
                        self.push(
                            failure(
                                AssistantFailureKind::Refusal,
                                format!("The model declined this request{detail}"),
                            ),
                            out,
                        );
                    }
                }
            }
            "message_stop" => {
                let reason = self.stop_reason.take();
                self.push(StreamItem::Stop(reason), out);
            }
            "error" => {
                let kind = anthropic_error_kind(str_at(json, &["error", "type"]).unwrap_or(""));
                let message = str_at(json, &["error", "message"])
                    .unwrap_or("The server reported an error")
                    .to_string();
                self.push(failure(kind, message), out);
            }
            _ => {}
        }
    }

    fn openai(&mut self, json: &Json, out: &mut Vec<StreamItem>) {
        if let Some(err) = json.get("error") {
            let message = err
                .get("message")
                .and_then(Json::as_str)
                .or_else(|| err.as_str())
                .unwrap_or("The server reported an error")
                .to_string();
            self.push(failure(AssistantFailureKind::Other, message), out);
            return;
        }
        let Some(choice) = json.get("choices").and_then(|c| c.get(0)) else {
            return;
        };
        let delta = &choice["delta"];
        if str_at(delta, &["reasoning_content"]).is_some_and(|r| !r.is_empty())
            || str_at(delta, &["reasoning"]).is_some_and(|r| !r.is_empty())
        {
            self.push(StreamItem::Thinking, out);
        }
        if let Some(text) = str_at(delta, &["content"]) {
            self.push(StreamItem::Text(text.to_string()), out);
        }
        if let Some(reason) = str_at(choice, &["finish_reason"]) {
            if reason == "content_filter" {
                self.push(
                    failure(
                        AssistantFailureKind::Refusal,
                        "The provider's content filter stopped the reply",
                    ),
                    out,
                );
            } else {
                self.stop_reason = Some(reason.to_string());
            }
        }
    }

    fn ollama_line(&mut self, line: &str, out: &mut Vec<StreamItem>) {
        if line.trim().is_empty() {
            return;
        }
        let Ok(json) = serde_json::from_str::<Json>(line) else {
            return;
        };
        if let Some(err) = str_at(&json, &["error"]) {
            self.push(failure(AssistantFailureKind::BadRequest, err), out);
            return;
        }
        if str_at(&json, &["message", "thinking"]).is_some_and(|t| !t.is_empty()) {
            self.push(StreamItem::Thinking, out);
        }
        if let Some(text) = str_at(&json, &["message", "content"]) {
            self.push(StreamItem::Text(text.to_string()), out);
        }
        if json.get("done").and_then(Json::as_bool) == Some(true) {
            let reason = str_at(&json, &["done_reason"]).map(str::to_string);
            self.push(StreamItem::Stop(reason), out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feeds `fixture` in chunks of `size` bytes (splitting lines and UTF-8 sequences).
    fn decode(provider: AssistantProvider, fixture: &str, size: usize) -> Vec<StreamItem> {
        let mut d = Decoder::new(provider);
        let mut out = Vec::new();
        for chunk in fixture.as_bytes().chunks(size) {
            out.extend(d.feed(chunk));
        }
        out.extend(d.finish());
        out
    }

    fn text(items: &[StreamItem]) -> String {
        items
            .iter()
            .filter_map(|i| match i {
                StreamItem::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect()
    }

    const ANTHROPIC: &str = "event: message_start\r\n\
data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-opus-5-5\",\"content\":[]}}\r\n\
\r\n\
event: content_block_start\n\
data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\
\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"abc\"}}\n\
\n\
event: content_block_stop\n\
data: {\"type\":\"content_block_stop\",\"index\":0}\n\
\n\
: keep-alive comment\n\
event: ping\n\
data: {\"type\":\"ping\"}\n\
\n\
event: content_block_start\n\
data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\
\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"Here — \"}}\n\
\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"```sql\\nSELECT 1;\\n```\"}}\n\
\n\
event: content_block_stop\n\
data: {\"type\":\"content_block_stop\",\"index\":1}\n\
\n\
event: message_delta\n\
data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":12}}\n\
\n\
event: message_stop\n\
data: {\"type\":\"message_stop\"}\n\
\n";

    #[test]
    fn anthropic_stream_in_any_chunking() {
        for size in [1, 3, 7, 64, 10_000] {
            let items = decode(AssistantProvider::Anthropic, ANTHROPIC, size);
            assert_eq!(items.first(), Some(&StreamItem::Thinking), "chunk {size}");
            assert_eq!(
                text(&items),
                "Here — ```sql\nSELECT 1;\n```",
                "chunk {size}"
            );
            assert_eq!(
                items.last(),
                Some(&StreamItem::Stop(Some("end_turn".into()))),
                "chunk {size}"
            );
        }
    }

    #[test]
    fn anthropic_errors_refusals_and_fallbacks() {
        let overloaded = "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n\n";
        let items = decode(AssistantProvider::Anthropic, overloaded, 5);
        assert_eq!(
            items,
            vec![StreamItem::Failed(AssistantFailure::new(
                AssistantFailureKind::Server,
                "Overloaded"
            ))]
        );

        let refusal = "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"refusal\",\"stop_details\":{\"type\":\"refusal\",\"category\":\"cyber\",\"explanation\":\"Policy\"}}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
        let items = decode(AssistantProvider::Anthropic, refusal, 9);
        assert_eq!(items.len(), 1);
        match &items[0] {
            StreamItem::Failed(f) => {
                assert_eq!(f.kind, AssistantFailureKind::Refusal);
                assert!(f.message.ends_with(": Policy"));
            }
            other => panic!("unexpected {other:?}"),
        }

        let fallback = "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"fallback\",\"from\":{\"model\":\"claude-opus-5-5\"},\"to\":{\"model\":\"claude-opus-4-8\"}}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
        let items = decode(AssistantProvider::Anthropic, fallback, 4);
        assert_eq!(
            items[0],
            StreamItem::Notice("claude-opus-5-5 declined; claude-opus-4-8 continued".into())
        );
        assert_eq!(items[1], StreamItem::Stop(None));

        // Cut off mid-reply.
        let cut = &ANTHROPIC[..ANTHROPIC.find("event: message_delta").unwrap()];
        let items = decode(AssistantProvider::Anthropic, cut, 11);
        match items.last() {
            Some(StreamItem::Failed(f)) => assert_eq!(f.kind, AssistantFailureKind::Network),
            other => panic!("unexpected {other:?}"),
        }
    }

    const OPENAI: &str = "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"\"},\"finish_reason\":null}]}\n\n\
data: {\"id\":\"c1\",\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"hmm\"},\"finish_reason\":null}]}\n\n\
data: {\"id\":\"c1\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Use \"},\"finish_reason\":null}]}\n\n\
data: {\"id\":\"c1\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"an index ✓\"},\"finish_reason\":null}]}\n\n\
data: {\"id\":\"c1\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n\
data: [DONE]\n\n";

    #[test]
    fn openai_stream_in_any_chunking() {
        for size in [1, 2, 13, 10_000] {
            let items = decode(AssistantProvider::Openai, OPENAI, size);
            assert_eq!(items[0], StreamItem::Thinking);
            assert_eq!(text(&items), "Use an index ✓");
            assert_eq!(items.last(), Some(&StreamItem::Stop(Some("stop".into()))));
        }
        // No [DONE] but a finish reason: complete.
        let no_done = OPENAI.replace("data: [DONE]\n\n", "");
        let items = decode(AssistantProvider::Openai, &no_done, 50);
        assert_eq!(items.last(), Some(&StreamItem::Stop(Some("stop".into()))));
        // An in-stream error.
        let err = "data: {\"error\":{\"message\":\"model not loaded\"}}\n\n";
        match decode(AssistantProvider::Openai, err, 3).first() {
            Some(StreamItem::Failed(f)) => assert_eq!(f.message, "model not loaded"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn ollama_lines() {
        let fixture = "{\"model\":\"llama3.2\",\"message\":{\"role\":\"assistant\",\"content\":\"SELECT\"},\"done\":false}\n\
{\"model\":\"llama3.2\",\"message\":{\"role\":\"assistant\",\"content\":\" 1\"},\"done\":false}\n\
{\"model\":\"llama3.2\",\"message\":{\"role\":\"assistant\",\"content\":\"\"},\"done\":true,\"done_reason\":\"stop\"}";
        let items = decode(AssistantProvider::Ollama, fixture, 6);
        assert_eq!(text(&items), "SELECT 1");
        assert_eq!(items.last(), Some(&StreamItem::Stop(Some("stop".into()))));

        let err = "{\"error\":\"model \\\"x\\\" not found, try pulling it first\"}\n";
        match decode(AssistantProvider::Ollama, err, 4).first() {
            Some(StreamItem::Failed(f)) => {
                assert_eq!(f.kind, AssistantFailureKind::BadRequest);
                assert!(f.message.contains("not found"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn sse_parser_rules() {
        let mut p = SseParser::default();
        let mut events = p.feed(b"event: a\ndata: 1\ndata:2\nid: 7\n\n:comment\ndata");
        events.extend(p.feed(b": tail\n"));
        events.extend(p.finish());
        assert_eq!(
            events,
            vec![
                SseEvent {
                    event: Some("a".into()),
                    data: "1\n2".into()
                },
                SseEvent {
                    event: None,
                    data: "tail".into()
                }
            ]
        );
    }
}
