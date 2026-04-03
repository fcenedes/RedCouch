# Release Process

This chapter covers how RedCouch releases are built, published, and verified. It serves as the maintainer runbook for cutting releases.

## Distribution Channels

| Channel | Status | Output |
|---|---|---|
| **GitHub Releases** | Primary | Pre-built `.tar.gz` archives with SHA-256 checksums for 4 targets |
| **crates.io** | Secondary, policy-gated | Source crate (requires explicit opt-in) |

GitHub Releases is the primary distribution channel. Users download pre-built module binaries. crates.io publication is optional and not required.

## Release Artifacts

Each release produces artifacts for all supported platforms:

| Target | Artifact | OS | Architecture |
|---|---|---|---|
| `x86_64-unknown-linux-gnu` | `libred_couch.so` | Linux | x86_64 |
| `aarch64-unknown-linux-gnu` | `libred_couch.so` | Linux | ARM64 |
| `x86_64-apple-darwin` | `libred_couch.dylib` | macOS | x86_64 |
| `aarch64-apple-darwin` | `libred_couch.dylib` | macOS | ARM64 |

Each artifact is packaged as a `.tar.gz` archive with a matching `.tar.gz.sha256` checksum file.

## Cutting a Release (Maintainer Runbook)

### Pre-release checklist

- [ ] Latest `main` commit passes CI (check, test, clippy, fmt, doc)
- [ ] Version in `Cargo.toml` is updated to the new version
- [ ] `cargo check` succeeds (updates `Cargo.lock`)
- [ ] `CHANGELOG` or release notes are prepared (GitHub auto-generates from PR titles)

### Steps

1. **Verify CI is green** on the latest `main` commit:
   ```bash
   # Check CI status on GitHub, or run locally:
   cargo check --all-targets
   cargo test
   cargo clippy --all-targets -- -D warnings
   cargo fmt --check
   RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
   ```

2. **Set the version** in `Cargo.toml` and commit:
   ```bash
   # Edit Cargo.toml: version = "0.2.0"
   cargo check  # Updates Cargo.lock
   git add Cargo.toml Cargo.lock
   git commit -m "chore: bump version to 0.2.0"
   git push origin main
   ```

3. **Create and push the tag** (must match `Cargo.toml` version):
   ```bash
   git tag v0.2.0
   git push origin v0.2.0
   ```

4. **The release workflow runs automatically** (`.github/workflows/release.yml`):

   | Job | What it does | Failure behavior |
   |---|---|---|
   | `validate-tag` | Confirms tag version matches `Cargo.toml` | Fails fast — prevents mismatched releases |
   | `test` | Runs `cargo test`, clippy, fmt | Blocks build and publish |
   | `build` | Cross-compiles for all 4 targets | Blocks publish |
   | `publish-github` | Creates GitHub Release with artifacts | — |
   | `publish-crate` | Publishes to crates.io (if enabled) | Non-blocking — GitHub Release still succeeds |

5. **Verify the release** at `https://github.com/fcenedes/RedCouch/releases`:
   - All 4 target archives present
   - SHA-256 checksum files present
   - Release notes auto-generated

### What the workflow does NOT do

- It does **not** deploy the module to any running Redis instance.
- It does **not** publish to crates.io unless `PUBLISH_CRATE` is explicitly enabled (see below).
- It does **not** run integration tests or benchmarks — only unit tests, clippy, and fmt.

## crates.io Publication (Optional)

crates.io publication is **policy-gated** and disabled by default. To enable:

1. Set repository variable `PUBLISH_CRATE` to `true` in GitHub Settings → Variables.
2. Add a `CARGO_REGISTRY_TOKEN` secret with a valid crates.io API token.
3. The `publish-crate` job runs automatically after a successful GitHub Release.

This gate exists because crates.io publication is irreversible — once a version is published, it cannot be unpublished (only yanked).

## CI Pipeline

The CI workflow (`.github/workflows/ci.yml`) runs on every push to `main` and on pull requests:

| Check | Platforms | Command |
|---|---|---|
| Build check | Ubuntu + macOS | `cargo check --all-targets` |
| Tests | Ubuntu + macOS | `cargo test` |
| Linting | Ubuntu + macOS | `cargo clippy --all-targets -- -D warnings` |
| Formatting | Ubuntu + macOS | `cargo fmt --check` |
| Documentation | Ubuntu | `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps` |

All checks must pass before a PR can be merged.

## Verification Commands

```bash
# Build check
cargo check --all-targets

# Unit/protocol tests (221 tests — no Redis required)
cargo test

# Integration tests (requires Redis 8+ with module loaded)
cd tests/integration && bash run_e2e.sh

# Benchmark suite
cd benchmarks && bash run_benchmarks.sh

# Stress/soak validation
cd benchmarks && bash run_stress_soak.sh

# Cross-system comparison (requires Docker)
bash benchmarks/run_cross_system.sh
```
