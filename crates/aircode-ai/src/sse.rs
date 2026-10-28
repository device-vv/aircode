pub struct SseDecoder {
    buffer: Vec<u8>,
    data_lines: Vec<String>,
}

impl SseDecoder {
    pub fn new() -> Self {
        Self {
            buffer: Vec::new(),
            data_lines: Vec::new(),
        }
    }

    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<String>, String> {
        self.buffer.extend_from_slice(bytes);
        let mut events = Vec::new();

        while let Some(newline) = self.buffer.iter().position(|byte| *byte == b'\n') {
            let mut line: Vec<u8> = self.buffer.drain(..=newline).collect();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let line = std::str::from_utf8(&line)
                .map_err(|_| "SSE stream contains invalid UTF-8".to_string())?;

            if line.is_empty() {
                if !self.data_lines.is_empty() {
                    events.push(self.data_lines.join("\n"));
                    self.data_lines.clear();
                }
            } else if let Some(data) = line.strip_prefix("data:") {
                self.data_lines
                    .push(data.strip_prefix(' ').unwrap_or(data).into());
            }
        }

        Ok(events)
    }

    /// Flushes what is left when the connection closes without a final blank line.
    pub fn finish(&mut self) -> Result<Vec<String>, String> {
        let mut events = if self.buffer.is_empty() {
            Vec::new()
        } else {
            self.push(b"\n")?
        };
        if !self.data_lines.is_empty() {
            events.push(self.data_lines.join("\n"));
            self.data_lines.clear();
        }
        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_fragmented_events_and_multiline_data() {
        let mut decoder = SseDecoder::new();
        assert!(decoder.push(b"data: first\nda").unwrap().is_empty());
        assert!(decoder.push(b"ta: second\r\n\r\nda").unwrap() == ["first\nsecond"]);
        assert!(decoder.push(b"ta: [DONE]\n\n").unwrap() == ["[DONE]"]);
    }

    #[test]
    fn handles_utf8_split_between_network_chunks() {
        let text = "data: Привет\n\n";
        let bytes = text.as_bytes();
        let split = bytes
            .windows(2)
            .position(|pair| pair[0] == 0xD0 && pair[1] == 0x9F)
            .unwrap()
            + 1;
        let mut decoder = SseDecoder::new();

        assert!(decoder.push(&bytes[..split]).unwrap().is_empty());
        assert_eq!(decoder.push(&bytes[split..]).unwrap(), ["Привет"]);
    }

    #[test]
    fn finish_flushes_an_unterminated_event() {
        let mut decoder = SseDecoder::new();
        assert!(decoder.push(b"data: tail").unwrap().is_empty());
        assert_eq!(decoder.finish().unwrap(), ["tail"]);
        assert!(decoder.finish().unwrap().is_empty());
    }

    #[test]
    fn ignores_comments_and_other_fields() {
        let mut decoder = SseDecoder::new();
        let events = decoder
            .push(b": keepalive\nevent: message\ndata: ok\n\n")
            .unwrap();

        assert_eq!(events, ["ok"]);
    }
}
