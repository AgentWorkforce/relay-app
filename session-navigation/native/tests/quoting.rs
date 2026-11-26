//! POSIX quoting: generated argv vectors round-trip through a reference
//! un-quoter; NUL and newlines are refused.
use session_navigation_native::common::quote::{
    command_line, quote, unquote_reference, QuoteError,
};

const ATOMS: &[&str] = &[
    "",
    " ",
    "  ",
    "'",
    "''",
    "\"",
    "$",
    "$HOME",
    "${x}",
    "`id`",
    "$(id)",
    ";",
    "&&",
    "|",
    ">",
    "<",
    "*",
    "?",
    "[a]",
    "~",
    "#",
    "!",
    "\\",
    "\\'",
    "\t",
    "-",
    "--",
    "-rf",
    "--flag=value",
    "é",
    "日本",
    "🙂",
    "a b",
    "x=y",
    "%s",
    "{}",
    "(",
    ")",
    "ses_0123456789abAAAAAAAAAAAAAA",
    "01900000-0000-7000-8000-000000000002",
];

struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        usize::try_from((self.0 >> 33) % u64::try_from(n).unwrap_or(1)).unwrap_or(0)
    }
}

#[test]
fn generated_argv_round_trips() {
    let mut rng = Rng(42);
    for _ in 0..5000 {
        let mut argv = vec!["/Users/a b/bin/co'dex".to_owned()];
        for _ in 0..rng.below(6) {
            let word: String = (0..rng.below(4))
                .map(|_| ATOMS[rng.below(ATOMS.len())])
                .collect();
            argv.push(word);
        }
        let line = command_line(&argv[0], &argv[1..]).unwrap_or_default();
        assert_eq!(unquote_reference(&line), Some(argv.clone()), "{line}");
    }
}

#[test]
fn quote_shapes() {
    assert_eq!(quote("").as_deref(), Ok("''"));
    assert_eq!(quote("it's").as_deref(), Ok("'it'\\''s'"));
    assert_eq!(quote("$(rm -rf /)").as_deref(), Ok("'$(rm -rf /)'"));
    for bad in ["a\nb", "a\0b", "a\rb"] {
        assert_eq!(quote(bad), Err(QuoteError::Unquotable));
        assert_eq!(
            command_line("/bin/x", &[bad.to_owned()]),
            Err(QuoteError::Unquotable)
        );
    }
    assert_eq!(command_line("codex", &[]), Err(QuoteError::RelativeProgram));
    assert_eq!(
        command_line("/x", &["a".repeat(70_000)]),
        Err(QuoteError::TooLong)
    );
}

#[test]
fn reference_unquoter_rejects_unquoted_text() {
    for bad in ["a", "'a' b", "'a", "'a'\\x", "\\'"] {
        assert_eq!(unquote_reference(bad), None, "{bad}");
    }
}
