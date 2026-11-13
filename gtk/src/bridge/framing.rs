use super::*;

impl Connection {
    /// The next message; a line longer than the contract allows ends the
    /// connection.
    pub(super) async fn read(&mut self) -> io::Result<Value> {
        let mut line = Vec::new();
        (&mut self.reader)
            .take(MAX_REPLY_BYTES as u64 + 2)
            .read_until(b'\n', &mut line)
            .await?;
        if line.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the core closed the connection",
            ));
        }
        if line.last() != Some(&b'\n') || line.len() - 1 > MAX_REPLY_BYTES {
            return Err(invalid(
                "the core sent a line longer than the contract allows",
            ));
        }
        match serde_json::from_slice(&line) {
            Ok(value @ Value::Object(_)) => Ok(value),
            _ => Err(invalid("the core sent something that is not a message")),
        }
    }
}
