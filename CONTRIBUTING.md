# Contributing to RedCouch

Thank you for your interest in contributing to RedCouch! This guide covers the development workflow, coding standards, and submission process.

## Prerequisites

- **Rust** 1.85+ (stable toolchain)
- **Redis** 8.x (for integration testing; verified on 8.4.0)
- **Python** 3.10+ (for E2E and benchmark scripts)
- **Git**

## Getting Started

```bash
# Clone the repository
git clone https://github.com/fcenedes/RedCouch.git
cd RedCouch

# Build
cargo build --release

# Run unit/protocol tests (no Redis required)
cargo test

# Check formatting and lints
cargo fmt --check
cargo clippy --all-targets -- -D warnings
```

## Development Workflow

### 1. Build and Test Locally

```bash
# Quick check (fastest feedback)
cargo check --all-targets

# Run all 146 tests (60 binary + 58 ASCII + 28 meta)
cargo test

# Lint
cargo clippy --all-targets -- -D warnings

# Format check
cargo fmt --check
```

### 2. Integration Testing (requires Redis 8+)

```bash
# Start Redis with the module loaded
redis-server --loadmodule ./target/release/libred_couch.dylib  # macOS
redis-server --loadmodule ./target/release/libred_couch.so     # Linux

# Run E2E tests (in a separate terminal)
cd tests/integration && bash run_e2e.sh
```

### 3. Benchmarks

```bash
# Run benchmark suite (requires Redis 8+ with module loaded)
cd benchmarks && bash run_benchmarks.sh

# Run stress/soak validation
cd benchmarks && bash run_stress_soak.sh
```

## Code Structure

| File | Responsibility |
|---|---|
| `src/lib.rs` | Module entry, TCP listener, connection handling, Redis dispatch |
| `src/protocol.rs` | Binary protocol parser/encoder, types, constants |
| `src/ascii.rs` | ASCII text protocol parser, meta prefix routing |
| `src/meta.rs` | Meta protocol parser, flag validation |

See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for detailed architecture documentation.

## Coding Standards

- **No `unsafe` code**: The crate uses `#![forbid(unsafe_code)]`.
- **Formatting**: Run `cargo fmt` before committing. CI enforces `cargo fmt --check`.
- **Linting**: Run `cargo clippy --all-targets -- -D warnings`. CI enforces zero warnings.
- **Documentation**: Run `cargo doc --no-deps` with `RUSTDOCFLAGS="-D warnings"`. CI enforces documentation builds cleanly.
- **Tests**: Add tests for new protocol commands or behavior changes. Tests should run without a live Redis instance (use `#[cfg(not(test))]` guards for Redis-dependent code).

## Pull Request Process

1. **Fork and branch**: Create a feature branch from `main`.
2. **Make changes**: Keep changes focused and minimal.
3. **Test locally**: Run `cargo test`, `cargo clippy`, and `cargo fmt --check`.
4. **Write tests**: Add or update tests to cover your changes.
5. **Submit PR**: Target the `main` branch. Describe what changed and why.
6. **CI checks**: Your PR must pass all CI checks (build, test, clippy, fmt, doc) on both Ubuntu and macOS.

## CI Pipeline

The CI workflow (`.github/workflows/ci.yml`) runs on every push to `main` and on pull requests:

- `cargo check --all-targets` (Ubuntu + macOS)
- `cargo test` (Ubuntu + macOS)
- `cargo clippy --all-targets -- -D warnings` (Ubuntu + macOS)
- `cargo fmt --check` (Ubuntu + macOS)
- `cargo doc --no-deps` with `-D warnings` (Ubuntu)

## Platform Support

RedCouch builds and runs on:
- **Linux** x86_64 and ARM64
- **macOS** x86_64 and ARM64

**Windows is not supported.** Redis modules require a Unix-like environment.

## What to Contribute

Areas where contributions are welcome:
- Bug fixes and protocol conformance improvements
- Test coverage expansion
- Performance improvements (with benchmark evidence)
- Documentation improvements

Please open an issue first for larger changes or new features to discuss the approach.

## License

By contributing, you agree that your contributions will be licensed under the [MIT License](LICENSE).
