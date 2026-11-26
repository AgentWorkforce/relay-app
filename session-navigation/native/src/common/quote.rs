//! POSIX single-quote quoting for the one place a command must be a string:
//! Ghostty's `command` and Terminal's `do script`, both run by a shell.
//!
//! Every word is wrapped in single quotes, inside which a POSIX shell
//! interprets nothing; an embedded `'` becomes `'\''`. NUL cannot be passed
//! through a C string and a newline would end the command for an
//! interactive shell, so both are refused rather than quoted.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuoteError {
    /// A word contains NUL, CR or LF.
    Unquotable,
    /// The program is not an absolute path.
    RelativeProgram,
    /// The command would be unreasonably long for a script argument.
    TooLong,
}

const MAX_COMMAND: usize = 64 * 1024;

pub fn quote(word: &str) -> Result<String, QuoteError> {
    if word.contains(['\0', '\n', '\r']) {
        return Err(QuoteError::Unquotable);
    }
    let mut out = String::with_capacity(word.len() + 2);
    out.push('\'');
    for c in word.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    Ok(out)
}

/// `'program' 'arg'…` with an absolute program.
pub fn command_line(program: &str, args: &[String]) -> Result<String, QuoteError> {
    if !program.starts_with('/') {
        return Err(QuoteError::RelativeProgram);
    }
    let mut words = vec![quote(program)?];
    for arg in args {
        words.push(quote(arg)?);
    }
    let line = words.join(" ");
    if line.len() > MAX_COMMAND {
        return Err(QuoteError::TooLong);
    }
    Ok(line)
}

/// A tiny reference un-quoter for the subset [`quote`] emits (single-quoted
/// spans, `\'` between them, words separated by single spaces). Used by the
/// tests to prove round trips; it rejects anything else.
pub fn unquote_reference(line: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut in_word = false;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                loop {
                    match chars.next()? {
                        '\'' => break,
                        other => current.push(other),
                    }
                }
            }
            '\\' => {
                if chars.next()? != '\'' || !in_word {
                    return None;
                }
                current.push('\'');
            }
            ' ' if in_word => {
                words.push(std::mem::take(&mut current));
                in_word = false;
            }
            _ => return None,
        }
    }
    if in_word {
        words.push(current);
    }
    Some(words)
}
