use super::*;

/// Locate fields without interpreting their values; the JSON5 parser validates
/// the document both before and after the edit. Duplicate edited keys fail closed.
pub(super) fn replace(source: &str, path: &[&str], value: &Value) -> Result<String> {
    let (key, rest) = path.split_first().ok_or("Missing JSON edit path")?;
    let fields = fields(source)?;
    let matches: Vec<_> = fields.iter().filter(|(name, _, _)| name == key).collect();
    if matches.len() > 1 {
        return Err("Duplicate MCP configuration keys".into());
    }
    if let Some((_, start, end)) = matches.first() {
        let replacement = if rest.is_empty() {
            serde_json::to_string_pretty(value).map_err(|_| "Cannot encode MCP configuration")?
        } else {
            replace(&source[*start..*end], rest, value)?
        };
        return Ok(format!(
            "{}{}{}",
            &source[..*start],
            replacement,
            &source[*end..]
        ));
    }
    let mut nested = value.clone();
    for key in rest.iter().rev() {
        nested = json!({*key: nested});
    }
    let encoded =
        serde_json::to_string_pretty(&nested).map_err(|_| "Cannot encode MCP configuration")?;
    // Insert directly after the opening brace. Existing trailing commas/comments
    // remain intact, and a leading comma is needed only for an existing field.
    let open = skip(source.as_bytes(), 0)?;
    let comma = if fields.is_empty() { "" } else { "," };
    Ok(format!(
        "{}\n{}: {}{}\n{}",
        &source[..open + 1],
        serde_json::to_string(key).map_err(|_| "Cannot encode MCP field name")?,
        encoded,
        comma,
        &source[open + 1..]
    ))
}

fn fields(source: &str) -> Result<Vec<(String, usize, usize)>> {
    let bytes = source.as_bytes();
    let mut cursor = skip(bytes, 0)?;
    if bytes.get(cursor) != Some(&b'{') {
        return Err("Expected JSON configuration object".into());
    }
    cursor += 1;
    let mut result = Vec::new();
    loop {
        cursor = skip(bytes, cursor)?;
        if bytes.get(cursor) == Some(&b'}') {
            return Ok(result);
        }
        let start = cursor;
        if matches!(bytes.get(cursor), Some(b'\'' | b'"')) {
            cursor = string_end(bytes, cursor)?;
        } else {
            while bytes
                .get(cursor)
                .is_some_and(|b| !b.is_ascii_whitespace() && *b != b':')
            {
                cursor += 1;
            }
        }
        let raw = &source[start..cursor];
        let key: String = if raw.starts_with(['\'', '"']) {
            json5::from_str(raw).map_err(|_| "Invalid JSON field name")?
        } else {
            raw.into()
        };
        cursor = skip(bytes, cursor)?;
        if bytes.get(cursor) != Some(&b':') {
            return Err("Invalid JSON field separator".into());
        }
        let value_start = skip(bytes, cursor + 1)?;
        let value_end = end(bytes, value_start)?;
        result.push((key, value_start, value_end));
        cursor = skip(bytes, value_end)?;
        match bytes.get(cursor) {
            Some(b',') => cursor += 1,
            Some(b'}') => return Ok(result),
            _ => return Err("Invalid JSON object separator".into()),
        }
    }
}

fn string_end(bytes: &[u8], mut cursor: usize) -> Result<usize> {
    let quote = bytes[cursor];
    cursor += 1;
    while let Some(byte) = bytes.get(cursor) {
        if *byte == quote {
            return Ok(cursor + 1);
        }
        cursor += if *byte == b'\\' { 2 } else { 1 };
    }
    Err("Unterminated JSON string".into())
}

fn skip(bytes: &[u8], mut cursor: usize) -> Result<usize> {
    loop {
        while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        if bytes.get(cursor..cursor + 2) == Some(b"//") {
            while bytes.get(cursor).is_some_and(|b| *b != b'\n') {
                cursor += 1;
            }
        } else if bytes.get(cursor..cursor + 2) == Some(b"/*") {
            cursor += 2;
            while bytes.get(cursor..cursor + 2) != Some(b"*/") {
                if cursor >= bytes.len() {
                    return Err("Unterminated JSON comment".into());
                }
                cursor += 1;
            }
            cursor += 2;
        } else {
            return Ok(cursor);
        }
    }
}

fn end(bytes: &[u8], start: usize) -> Result<usize> {
    let mut cursor = start;
    let mut depth = 0usize;
    loop {
        cursor = skip(bytes, cursor)?;
        match bytes.get(cursor) {
            Some(b'\'' | b'"') => cursor = string_end(bytes, cursor)?,
            Some(b'{' | b'[') => {
                depth += 1;
                cursor += 1;
            }
            Some(b'}' | b']') if depth > 0 => {
                depth -= 1;
                cursor += 1;
                if depth == 0 {
                    return Ok(cursor);
                }
            }
            Some(b',' | b'}') if depth == 0 => return Ok(cursor),
            Some(_) => cursor += 1,
            None => return Err("Unterminated JSON value".into()),
        }
    }
}
