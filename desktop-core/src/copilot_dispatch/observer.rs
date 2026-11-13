//! No-op SDK observer: no mutations, proof, credentials, or input logs.
use std::io::{Read, Write};

pub fn run(args: &[String]) -> i32 {
    observe(args, std::io::stdin().lock(), std::io::stdout().lock())
}
fn observe(args: &[String], mut input: impl Read, mut output: impl Write) -> i32 {
    if args == ["--help"] || args == ["-h"] {
        return i32::from(
            output
                .write_all(b"copilot-mcp-observe [--dev]\nObserve native Copilot MCP calls without modifying them.\n")
                .and_then(|_| output.flush())
                .is_err(),
        );
    }
    if !(args.is_empty() || args == ["--dev"]) {
        return 1;
    }
    // This global hook sees every MCP server, including large foreign inputs.
    // Drain through EOF rather than closing early at an arbitrary payload cap:
    // a writer could otherwise see EPIPE and fail an unrelated MCP operation.
    // Memory is fixed; the installed SDK hook timeout bounds process lifetime.
    let mut buffer = [0u8; 8192];
    loop {
        match input.read(&mut buffer) {
            Ok(0) => break,
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    if output
        .write_all(b"{}\n")
        .and_then(|_| output.flush())
        .is_err()
    {
        return 1;
    }
    0
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn help_answers_without_reading_hook_input() {
        for flag in ["--help", "-h"] {
            let mut input = &b"untouched-hook-input"[..];
            let mut output = Vec::new();
            assert_eq!(observe(&[flag.into()], &mut input, &mut output), 0);
            assert_eq!(input, b"untouched-hook-input");
            assert!(String::from_utf8(output)
                .unwrap()
                .starts_with("copilot-mcp-observe [--dev]\n"));
        }
    }
    #[test]
    fn emits_only_empty_object_without_input_mutation() {
        for args in [vec![], vec!["--dev".into()]] {
            let mut out = Vec::new();
            assert_eq!(
                observe(
                    &args,
                    &b"{\"arguments\":{\"synthetic\":\"never-echo\"}}"[..],
                    &mut out
                ),
                0
            );
            assert_eq!(out, b"{}\n");
        }
    }
    #[test]
    fn malformed_nonobject_and_empty_foreign_input_remain_noops() {
        for bytes in [b"[]".as_slice(), b"null", b"not-json", b"{} {}", b""] {
            let mut out = Vec::new();
            assert_eq!(observe(&[], bytes, &mut out), 0);
            assert_eq!(out, b"{}\n");
        }
    }
    #[test]
    fn drains_foreign_payloads_beyond_both_old_limits_in_fixed_memory() {
        for size in [128 * 1024 + 1, 64 * 1024 * 1024 + 1] {
            let mut input = std::io::repeat(b'x').take(size);
            let mut out = Vec::new();
            assert_eq!(observe(&[], &mut input, &mut out), 0);
            assert_eq!(input.limit(), 0);
            assert_eq!(out, b"{}\n");
        }
    }
    #[test]
    fn unknown_cli_args_fail_without_consuming_or_echoing_input() {
        let mut input = &b"synthetic-input"[..];
        let mut out = Vec::new();
        assert_eq!(observe(&["--token".into()], &mut input, &mut out), 1);
        assert_eq!(input, b"synthetic-input");
        assert!(out.is_empty());
    }
    #[test]
    fn input_error_cannot_become_a_hook_rejection() {
        struct Failed;
        impl Read for Failed {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("synthetic read error"))
            }
        }
        let mut out = Vec::new();
        assert_eq!(observe(&[], Failed, &mut out), 0);
        assert_eq!(out, b"{}\n");
    }
}
