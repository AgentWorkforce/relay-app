#!/usr/bin/env bash
# Run tests with the production platform-only trust environment. Cargo's Linux
# libgit2 initialization adds SSL_CERT_* to its children even with a clean shell;
# remove only those additions at the test-executable boundary. Refusal tests
# start their own children directly and keep the overrides they deliberately set.
# The refusal of an inherited SSL_CERT_FILE or SSL_CERT_DIR is deliberate on every
# OS, macOS included: Cargo passes the environment through to the test children,
# so an inherited override would reach them anywhere, and masking it silently is
# worse than stopping. Clear the variable for the test run.
set -euo pipefail

for name in SSL_CERT_FILE SSL_CERT_DIR; do
  if [ -n "${!name:-}" ]; then
    printf '%s\n' "Refusing tests with inherited $name; its value was not printed or changed." >&2
    exit 1
  fi
done

if [ "$(uname -s)" = Linux ]; then
  host=$(rustc -vV | sed -n 's/^host: //p')
  case "$host" in
    x86_64-unknown-linux-gnu|aarch64-unknown-linux-gnu) ;;
    *) printf '%s\n' 'Unsupported Linux test host; no Cargo runner changed.' >&2; exit 1 ;;
  esac
  runner="CARGO_TARGET_$(printf '%s' "$host" | tr 'a-z-' 'A-Z_')_RUNNER"
  if [ -n "${!runner:-}" ]; then
    printf '%s\n' 'Refusing to replace an existing Cargo test runner.' >&2
    exit 1
  fi
  export "$runner=env -u SSL_CERT_FILE -u SSL_CERT_DIR"
fi

exec cargo test "$@"
