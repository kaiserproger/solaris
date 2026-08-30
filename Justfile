# Solaris developer entrypoints. Keep expensive/long-running work explicit.

default:
    @just --list

# Fast compile/type surface.
check-fast:
    cargo check --workspace --all-targets

# Default test runner for local iteration.
test:
    cargo nextest run --workspace

# Canonical Rust test semantics, including doctests where packages expose them.
test-all:
    cargo test --workspace

# Repository-defined release/milestone L2 gate from AGENTS.md.
l2:
    cargo run -p xtask -- code-health
    cargo test --workspace
    cargo clippy --workspace --all-targets -- -D warnings
    cargo fmt --all -- --check

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all -- --check

clippy:
    cargo clippy --workspace --all-targets -- -D warnings

code-health:
    cargo run -p xtask -- code-health

# Coverage is intentionally explicit; it can be substantially slower than iteration tests.
coverage:
    cargo llvm-cov nextest --workspace --html

# Supply-chain / dependency hygiene. These are not part of every edit loop.
audit:
    cargo audit
    cargo deny check
    cargo machete

bench:
    cargo bench

# System-level benchmark helper; callers supply the command being measured.
hyperfine *ARGS:
    hyperfine {{ARGS}}

# Profiling may require host perf permissions; never invoke it implicitly.
flamegraph *ARGS:
    cargo flamegraph --release {{ARGS}}

# Fuzzing is opt-in and requires a named target. Example: `just fuzz varint`.
fuzz TARGET *ARGS:
    cargo +nightly fuzz run {{TARGET}} {{ARGS}}

run:
    RUST_LOG=info cargo run --release -p mc-server

run-debug:
    RUST_LOG=debug RUST_BACKTRACE=1 cargo run -p mc-server

# Rebuild a fresh high-context bundle for the planning-only Pro whole-core audit.
pro-audit-context:
    bash tools/prepare-pro-core-audit.sh
