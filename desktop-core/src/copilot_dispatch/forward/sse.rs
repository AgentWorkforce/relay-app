//! Incremental MCP Streamable HTTP responses. Never reconnect or replay a POST.
//! SSE framing follows https://html.spec.whatwg.org/multipage/server-sent-events.html
//! JSON-RPC server requests need a bidirectional client and fail closed here.
use super::{response_value, IN_DOUBT, MAX_REPLY_BYTES};
use serde_json::Value;
use std::{io::Read, time::Instant};

pub(super) fn receive(
    mut reader: impl Read,
    id: &Value,
    deadline: Instant,
) -> Result<Value, &'static str> {
    let mut parser = Parser::default();
    let mut bytes = [0; 4096];
    let mut total = 0;
    loop {
        if Instant::now() >= deadline || total == MAX_REPLY_BYTES {
            return Err(IN_DOUBT);
        }
        // The HTTP reader retains the same original deadline during blocking
        // reads. These checks also bound buffered/fast streams and JSON work.
        let remaining = (MAX_REPLY_BYTES - total).min(bytes.len() as u64) as usize;
        let count = reader.read(&mut bytes[..remaining]).map_err(|_| IN_DOUBT)?;
        if count == 0 || Instant::now() >= deadline {
            // EOF never dispatches a frame lacking its final blank line.
            return Err(IN_DOUBT);
        }
        total += count as u64;
        for byte in &bytes[..count] {
            if let Some(reply) = parser.byte(*byte, id)? {
                return if Instant::now() < deadline {
                    Ok(reply)
                } else {
                    Err(IN_DOUBT)
                };
            }
        }
    }
}

struct Parser {
    line: Vec<u8>,
    data: Vec<u8>,
    event: String,
    first_line: bool,
    after_cr: bool,
}
impl Default for Parser {
    fn default() -> Self {
        Self {
            line: Vec::new(),
            data: Vec::new(),
            event: String::new(),
            first_line: true,
            after_cr: false,
        }
    }
}
impl Parser {
    fn byte(&mut self, byte: u8, id: &Value) -> Result<Option<Value>, &'static str> {
        if self.after_cr {
            self.after_cr = false;
            if byte == b'\n' {
                return Ok(None);
            }
        }
        if byte == b'\r' || byte == b'\n' {
            self.after_cr = byte == b'\r';
            return self.line(id);
        }
        self.line.push(byte);
        Ok(None)
    }
    fn line(&mut self, id: &Value) -> Result<Option<Value>, &'static str> {
        let line = std::str::from_utf8(&self.line).map_err(|_| IN_DOUBT)?;
        let line = if self.first_line {
            line.strip_prefix('\u{feff}').unwrap_or(line)
        } else {
            line
        };
        self.first_line = false;
        let result = if line.is_empty() {
            // Match the MCP SDK: only the default/message event transports
            // JSON-RPC. Reset the type even on a block with no data.
            let message_event = self.event.is_empty() || self.event == "message";
            self.event.clear();
            if self.data.is_empty() || !message_event {
                self.data.clear();
                Ok(None)
            } else {
                self.data.pop(); // The final LF belongs to SSE framing.
                let frame = std::mem::take(&mut self.data);
                message(&frame, id)
            }
        } else {
            let (field, value) = line.split_once(':').unwrap_or((line, ""));
            if field == "data" {
                self.data
                    .extend_from_slice(value.strip_prefix(' ').unwrap_or(value).as_bytes());
                self.data.push(b'\n');
            } else if field == "event" {
                self.event = value.strip_prefix(' ').unwrap_or(value).to_owned();
            }
            // Comments and SSE id/retry/unknown fields carry no MCP payload.
            // They still count toward the aggregate cap; no resume is attempted.
            Ok(None)
        };
        self.line.clear();
        result
    }
}

fn message(bytes: &[u8], id: &Value) -> Result<Option<Value>, &'static str> {
    let message: Value = serde_json::from_slice(bytes).map_err(|_| IN_DOUBT)?;
    if !message.is_object() || message["jsonrpc"] != "2.0" {
        return Err(IN_DOUBT);
    }
    if let Some(method) = message.get("method") {
        // A server request must never be silently treated as a notification.
        // This bridge has no server-request handler; refusal preserves unknown
        // send semantics rather than pretending that a call has completed.
        if !method.as_str().is_some_and(|s| !s.is_empty())
            || message.get("id").is_some()
            || message.get("result").is_some()
            || message.get("error").is_some()
            || message
                .get("params")
                .is_some_and(|p| !p.is_object() && !p.is_array())
        {
            return Err(IN_DOUBT);
        }
        return Ok(None);
    }
    let response_id = message.get("id").ok_or(IN_DOUBT)?;
    if !(response_id.is_string() || response_id.is_number() || response_id.is_null()) {
        return Err(IN_DOUBT);
    }
    // Validate nonmatching responses too. Malformed data is never evidence
    // that a call was unsent, and never triggers a reconnect or retry.
    let response = response_value(bytes, response_id)?;
    Ok((response_id == id).then_some(response))
}

#[cfg(test)]
mod tests;
