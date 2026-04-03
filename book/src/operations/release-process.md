# Release Process

## GitHub Releases (Primary)

**GitHub Releases is the primary distribution channel for RedCouch.** Pre-built module artifacts are published as GitHub Releases with per-target `.tar.gz` archives and SHA-256 checksums.

### Maintainer Steps to Cut a Release

1. **Ensure `main` is green**: verify the latest commit passes CI (`ci.yml`: check, test, clippy, fmt, doc).
2. **Set the version** in `Cargo.toml` and run `cargo check` to update `Cargo.lock`.
3. **Create and push a `v*` tag** whose version matches `Cargo.toml`:
   ```bash
   git tag v0.1.0
   git push origin v0.1.0
   ```
4. **The release workflow runs automatically** (`.github/workflows/release.yml`):
   - **`validate-tag`** — confirms the tag version matches `Cargo.toml` (fails fast on mismatch).
   - **`test`** — runs `cargo test`, `cargo clippy --all-targets -- -D warnings`, and `cargo fmt --check`.
   - **`build`** — cross-compiles for all 4 supported targets.
   - **`publish-github`** — creates a GitHub Release with auto-generated notes and attaches artifacts.
5. **Verify the GitHub Release page** at `https://github.com/fcenedes/RedCouch/releases`.

### What the Workflow Does Not Do

- It does **not** publish to crates.io unless `PUBLISH_CRATE` is explicitly enabled.
- It does **not** deploy the module to any running Redis instance.

## crates.io (Secondary — Policy-Gated)

Source publication to crates.io is **optional and policy-gated**. It is not required for the primary GitHub Release path.

To enable crates.io publication:
1. Set the repository variable `PUBLISH_CRATE` to `true` in GitHub Settings → Variables.
2. Add a `CARGO_REGISTRY_TOKEN` secret with a valid crates.io API token.
3. The `publish-crate` job in the release workflow will then run automatically after the GitHub Release.

## CI Pipeline

The CI workflow (`.github/workflows/ci.yml`) runs on every push to `main` and on pull requests:

- `cargo check --all-targets` (Ubuntu + macOS)
- `cargo test` (Ubuntu + macOS)
- `cargo clippy --all-targets -- -D warnings` (Ubuntu + macOS)
- `cargo fmt --check` (Ubuntu + macOS)
- `cargo doc --no-deps` with `-D warnings` (Ubuntu)

## Verification Commands

```bash
cargo check                                      # Build check
cargo test                                       # 221 unit/protocol tests
cd tests/integration && bash run_e2e.sh          # E2E (requires Redis 8+)
cd benchmarks && bash run_benchmarks.sh          # Benchmark suite
cd benchmarks && bash run_stress_soak.sh         # Stress/soak suite
bash benchmarks/run_cross_system.sh              # Cross-system comparison (Docker)
```
