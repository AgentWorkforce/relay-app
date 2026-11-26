//! Windows command lines. A Windows process receives one string, and each
//! program splits it itself; nearly all of them (the MSVC CRT, Rust's std,
//! libuv/Node, `CommandLineToArgvW`) follow the same rules, which [`quote_arg`]
//! inverts and [`split`] implements:
//!
//! - arguments are separated by spaces and tabs outside quotes;
//! - `2n` backslashes before a `"` are `n` backslashes and the quote toggles
//!   quoting; `2n + 1` backslashes before a `"` are `n` backslashes and a
//!   literal `"`; backslashes not before a `"` are literal;
//! - inside quotes, `""` is a literal `"` (quoting continues);
//! - the first word (the program) is special: it ends at the next quote or
//!   whitespace, and backslashes are never escapes in it.
//!
//! Two hosts re-read the line with other rules, and both are refused rather
//! than escaped whenever an argument is not plainly safe:
//!
//! - `cmd.exe` runs every `.cmd`/`.bat` program (npm shims), expanding
//!   `%VAR%` even inside quotes and treating `^ & | < > ( ) !` specially:
//!   [`cmd_safe`] admits only letters, digits, whitespace and `_-.,:/\=+@{}#~`.
//! - Windows Terminal splits its command line on `;` before the child's
//!   command line (even after `--`) and rebuilds the child's line by
//!   wrapping words that contain spaces in quotes: [`wt_safe`] refuses `;`,
//!   `"`, `%`, empty words and a quoted word ending in `\`. UNVERIFIED (the
//!   rebuild rule is read from the GTK adapter's notes, not observed).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuoteError {
    /// NUL (or any control character, for launches) cannot be passed.
    Unquotable,
    /// The program is not a plain absolute path (`"` or empty).
    Program,
    /// `cmd.exe` would reinterpret a character of this word.
    CmdUnsafe,
    /// Windows Terminal would reinterpret this word.
    WtUnsafe,
    /// Longer than a Windows command line may be (32 767 UTF-16 units).
    TooLong,
}

/// The CreateProcess limit, in UTF-16 units, with room for the program.
pub const MAX_COMMAND_LINE: usize = 32_000;

fn push_backslashes(out: &mut String, n: usize) {
    out.extend(std::iter::repeat_n('\\', n));
}

/// One argument, quoted only when it must be.
pub fn quote_arg(arg: &str) -> Result<String, QuoteError> {
    if arg.contains('\0') {
        return Err(QuoteError::Unquotable);
    }
    if !arg.is_empty() && !arg.contains([' ', '\t', '\n', '\x0b', '"']) {
        return Ok(arg.to_owned());
    }
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('"');
    let mut slashes = 0usize;
    for c in arg.chars() {
        match c {
            '\\' => slashes += 1,
            '"' => {
                push_backslashes(&mut out, slashes * 2 + 1);
                out.push('"');
                slashes = 0;
            }
            _ => {
                push_backslashes(&mut out, slashes);
                out.push(c);
                slashes = 0;
            }
        }
    }
    push_backslashes(&mut out, slashes * 2);
    out.push('"');
    Ok(out)
}

/// The program word: quoted when it holds whitespace. A Windows path never
/// holds `"`, and backslashes are not escapes in the first word.
pub fn quote_program(program: &str) -> Result<String, QuoteError> {
    if program.is_empty() || program.contains(['"', '\0']) {
        return Err(QuoteError::Program);
    }
    if program.contains([' ', '\t']) {
        Ok(format!("\"{program}\""))
    } else {
        Ok(program.to_owned())
    }
}

/// The arguments only, joined by single spaces (what `Start-Process
/// -ArgumentList` and `ProcessStartInfo.Arguments` take).
pub fn join_args(args: &[String]) -> Result<String, QuoteError> {
    let words: Result<Vec<String>, QuoteError> = args.iter().map(|a| quote_arg(a)).collect();
    let line = words?.join(" ");
    if line.encode_utf16().count() > MAX_COMMAND_LINE {
        return Err(QuoteError::TooLong);
    }
    Ok(line)
}

/// `program arg…` as one command line.
pub fn command_line(program: &str, args: &[String]) -> Result<String, QuoteError> {
    let program = quote_program(program)?;
    let rest = join_args(args)?;
    let line = if rest.is_empty() {
        program
    } else {
        format!("{program} {rest}")
    };
    if line.encode_utf16().count() > MAX_COMMAND_LINE {
        return Err(QuoteError::TooLong);
    }
    Ok(line)
}

fn is_space(c: char) -> bool {
    c == ' ' || c == '\t'
}

/// The reference splitter (the post-2008 MSVC CRT rules). Used to read
/// `Win32_Process.CommandLine` into argv, and by the tests to prove that
/// [`command_line`] round-trips.
pub fn split(line: &str) -> Vec<String> {
    let mut chars = line.chars().peekable();
    let mut argv = Vec::new();
    // The program word.
    while chars.peek().copied().is_some_and(is_space) {
        chars.next();
    }
    if chars.peek().is_none() {
        return argv;
    }
    let mut program = String::new();
    if chars.peek() == Some(&'"') {
        chars.next();
        for c in chars.by_ref() {
            if c == '"' {
                break;
            }
            program.push(c);
        }
    } else {
        while let Some(c) = chars.peek().copied() {
            if is_space(c) {
                break;
            }
            program.push(c);
            chars.next();
        }
    }
    argv.push(program);
    loop {
        while chars.peek().copied().is_some_and(is_space) {
            chars.next();
        }
        if chars.peek().is_none() {
            return argv;
        }
        let mut word = String::new();
        let mut quoted = false;
        loop {
            let mut slashes = 0usize;
            while chars.peek() == Some(&'\\') {
                chars.next();
                slashes += 1;
            }
            match chars.peek().copied() {
                Some('"') => {
                    push_backslashes(&mut word, slashes / 2);
                    chars.next();
                    if slashes % 2 == 1 {
                        word.push('"');
                    } else if quoted && chars.peek() == Some(&'"') {
                        chars.next();
                        word.push('"');
                    } else {
                        quoted = !quoted;
                    }
                }
                Some(c) if is_space(c) && !quoted => {
                    push_backslashes(&mut word, slashes);
                    break;
                }
                Some(c) => {
                    push_backslashes(&mut word, slashes);
                    word.push(c);
                    chars.next();
                }
                None => {
                    push_backslashes(&mut word, slashes);
                    break;
                }
            }
        }
        argv.push(word);
    }
}

/// Whether `cmd.exe` passes this word through unchanged (see the module
/// notes). Deliberately narrow: session ids, flags and ordinary paths pass.
pub fn cmd_safe(word: &str) -> bool {
    word.chars().all(|c| {
        c.is_alphanumeric()
            || c == ' '
            || matches!(
                c,
                '_' | '-' | '.' | ',' | ':' | '/' | '\\' | '=' | '+' | '@' | '{' | '}' | '#' | '~'
            )
    })
}

/// Whether Windows Terminal passes this word through unchanged.
pub fn wt_safe(word: &str) -> bool {
    !word.is_empty()
        && !word.contains([';', '"', '%'])
        && !word.chars().any(char::is_control)
        && !(word.contains([' ', '\t']) && word.ends_with('\\'))
}

/// Whether `program` is run through `cmd.exe` by CreateProcess/ShellExecute.
pub fn is_batch(program: &str) -> bool {
    let lower = program.to_ascii_lowercase();
    lower.ends_with(".cmd") || lower.ends_with(".bat")
}
