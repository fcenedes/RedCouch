# Contributing

Thank you for your interest in contributing to RedCouch!

## Prerequisites

- **Rust** 1.85+ (stable toolchain)
- **Redis** 8.x (for integration testing; verified on 8.4.0)
- **Python** 3.10+ (for E2E and benchmark scripts)
- **Git**

## Getting Started

```bash
git clone https://github.com/fcenedes/RedCouch.git
cd RedCouch
cargo build --release
cargo test
cargo fmt --check
cargo clippy --all-targets -- -D warnings
```

## Development Workflow

### 1. Build and Test Locally

```bash
cargo check --all-targets     # Quick check
cargo test                     # All 221 tests
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

### 2. Integration Testing (requires Redis 8+)

```bash
redis-server --loadmodule ./target/release/libred_couch.dylib  # macOS
redis-server --loadmodule ./target/release/libred_couch.so     # Linux

# In a separate terminal:
cd tests/integration && bash run_e2e.sh
```

### 3. Benchmarks

```bash
cd benchmarks && bash run_benchmarks.sh
cd benchmarks && bash run_stress_soak.sh
```

## Code Structure

| File | Responsibility |
|---|---|
| `src/lib.rs` | Module entry, TCP listener, connection handling, Redis dispatch |
| `src/protocol.rs` | Binary protocol parser/encoder, types, constants |
| `src/ascii.rs` | ASCII text protocol parser, meta prefix routing |
| `src/meta.rs` | Meta protocol parser, flag validation |

See the [Architecture](../reference/architecture.md) chapter for detailed architecture documentation.

## Coding Standards

- **No `unsafe` code**: The crate uses `#![forbid(unsafe_code)]`.
- **Formatting**: Run `cargo fmt` before committing. CI enforces `cargo fmt --check`.
- **Linting**: Run `cargo clippy --all-targets -- -D warnings`. CI enforces zero warnings.
- **Documentation**: Run `cargo doc --no-deps` with `RUSTDOCFLAGS="-D warnings"`. CI enforces clean doc builds.
- **Tests**: Add tests for new protocol commands or behavior changes. Tests should run without a live Redis instance (use `#[cfg(not(test))]` guards for Redis-dependent code).

## Pull Request Process

1. **Fork and branch**: Create a feature branch from `main`.
2. **Make changes**: Keep changes focused and minimal.
3. **Test locally**: Run `cargo test`, `cargo clippy`, and `cargo fmt --check`.
4. **Write tests**: Add or update tests to cover your changes.
5. **Submit PR**: Target the `main` branch. Describe what changed and why.
6. **CI checks**: Your PR must pass all CI checks on both Ubuntu and macOS.

## What to Contribute

- Bug fixes and protocol conformance improvements
- Test coverage expansion
- Performance improvements (with benchmark evidence)
- Documentation improvements

Please open an issue first for larger changes or new features.

## License

By contributing, you agree that your contributions will be licensed under the [MIT License](https://github.com/fcenedes/RedCouch/blob/main/LICENSE).
