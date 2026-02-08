# CI/CD and Quality Gates

This document defines the continuous integration pipeline and quality requirements.

## Quality Standards

| Metric | Requirement |
|--------|-------------|
| Code Formatting | `cargo fmt --check` must pass |
| Linting | `cargo clippy -- -D warnings` (zero warnings) |
| Tests | All tests must pass |
| Coverage | Minimum 80% line coverage |

---

## GitHub Actions Workflow

### Triggers

- **Pull Request**: Run on all PRs to `main`
- **Push to Main**: Run on direct pushes to `main`

### Workflow File

`.github/workflows/ci.yml`:

```yaml
name: CI

on:
  push:
    branches: [main]
  pull_request:
    branches: [main]

env:
  CARGO_TERM_COLOR: always
  RUSTFLAGS: "-Dwarnings"

jobs:
  check:
    name: Check
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Install Rust toolchain
        uses: dtolnay/rust-action@stable
        with:
          components: rustfmt, clippy

      - name: Cache cargo registry
        uses: actions/cache@v4
        with:
          path: |
            ~/.cargo/registry
            ~/.cargo/git
            target
          key: ${{ runner.os }}-cargo-${{ hashFiles('**/Cargo.lock') }}
          restore-keys: |
            ${{ runner.os }}-cargo-

      - name: Check formatting
        run: cargo fmt --all -- --check

      - name: Run clippy
        run: cargo clippy --all-targets --all-features -- -D warnings

  test:
    name: Test
    runs-on: ubuntu-latest
    needs: check
    steps:
      - uses: actions/checkout@v4

      - name: Install Rust toolchain
        uses: dtolnay/rust-action@stable

      - name: Cache cargo registry
        uses: actions/cache@v4
        with:
          path: |
            ~/.cargo/registry
            ~/.cargo/git
            target
          key: ${{ runner.os }}-cargo-${{ hashFiles('**/Cargo.lock') }}
          restore-keys: |
            ${{ runner.os }}-cargo-

      - name: Run tests
        run: cargo test --all-features --verbose -- --nocapture
        env:
          RUST_BACKTRACE: 1

  coverage:
    name: Coverage
    runs-on: ubuntu-latest
    needs: check
    steps:
      - uses: actions/checkout@v4

      - name: Install Rust toolchain
        uses: dtolnay/rust-action@stable
        with:
          components: llvm-tools-preview

      - name: Install cargo-llvm-cov
        uses: taiki-e/install-action@cargo-llvm-cov

      - name: Cache cargo registry
        uses: actions/cache@v4
        with:
          path: |
            ~/.cargo/registry
            ~/.cargo/git
            target
          key: ${{ runner.os }}-cargo-coverage-${{ hashFiles('**/Cargo.lock') }}
          restore-keys: |
            ${{ runner.os }}-cargo-coverage-

      - name: Generate coverage report
        run: cargo llvm-cov --all-features --workspace --lcov --output-path lcov.info

      - name: Generate HTML report
        run: cargo llvm-cov --all-features --workspace --html --output-dir coverage-html

      - name: Check coverage threshold
        run: |
          COVERAGE=$(cargo llvm-cov --all-features --workspace --json | jq '.data[0].totals.lines.percent')
          echo "Coverage: $COVERAGE%"
          if (( $(echo "$COVERAGE < 80" | bc -l) )); then
            echo "::error::Coverage $COVERAGE% is below 80% threshold"
            exit 1
          fi

      - name: Upload coverage report
        uses: actions/upload-artifact@v4
        with:
          name: coverage-report
          path: coverage-html/
          retention-days: 30

      - name: Upload lcov report
        uses: actions/upload-artifact@v4
        with:
          name: lcov-report
          path: lcov.info
          retention-days: 30

  # Summary job that depends on all others
  ci-success:
    name: CI Success
    runs-on: ubuntu-latest
    needs: [check, test, coverage]
    steps:
      - name: All checks passed
        run: echo "All CI checks passed!"
```

---

## Local Development Commands

### Install Coverage Tool

```bash
# Install cargo-llvm-cov (recommended)
cargo install cargo-llvm-cov

# Alternative: cargo-tarpaulin (less accurate but simpler)
cargo install cargo-tarpaulin
```

### Run Formatting Check

```bash
# Check formatting
cargo fmt --all -- --check

# Auto-fix formatting
cargo fmt --all
```

### Run Clippy

```bash
# Run with deny warnings (same as CI)
cargo clippy --all-targets --all-features -- -D warnings

# Run with suggestions
cargo clippy --all-targets --all-features
```

### Run Tests

```bash
# Run all tests
cargo test --all-features

# Run with output (see println! statements)
cargo test --all-features -- --nocapture

# Run specific test
cargo test test_epoch_mapping --all-features

# Run tests matching pattern
cargo test agent:: --all-features
```

### Run Coverage

```bash
# Quick coverage summary
cargo llvm-cov --all-features --workspace

# Generate HTML report and open in browser
cargo llvm-cov --all-features --workspace --html --open

# Check 80% threshold (same as CI)
cargo llvm-cov --all-features --workspace --fail-under-lines 80

# Generate lcov format (for IDE integration)
cargo llvm-cov --all-features --workspace --lcov --output-path lcov.info

# Show uncovered lines
cargo llvm-cov --all-features --workspace --show-missing-lines
```

### Pre-Commit Hook (Optional)

Create `.git/hooks/pre-commit`:

```bash
#!/bin/bash
set -e

echo "Running pre-commit checks..."

echo "Checking formatting..."
cargo fmt --all -- --check

echo "Running clippy..."
cargo clippy --all-targets --all-features -- -D warnings

echo "Running tests..."
cargo test --all-features

echo "All checks passed!"
```

Make executable: `chmod +x .git/hooks/pre-commit`

---

## Coverage Strategy

### What to Test

| Component | Testing Approach |
|-----------|-----------------|
| ID hashing | Unit tests with known inputs |
| Epoch mapping | Unit tests for edge cases |
| Storage read/write | Integration tests with temp dirs |
| API endpoints | Integration tests with test server |
| Agent extraction | Fixture-based tests (no network) |
| Parquet operations | Unit tests with sample data |

### What NOT to Test (Excluded from Coverage)

```rust
// In lib.rs or specific modules
#[cfg(not(tarpaulin_include))]
mod integration_helpers;
```

Exclusions:
- Main function (minimal bootstrap code)
- Debug/development utilities
- Generated code (if any)

### Coverage Targets by Module

| Module | Target | Rationale |
|--------|--------|-----------|
| `models` | 95% | Core data structures, must be solid |
| `storage` | 90% | Critical path, file operations |
| `agents` | 80% | Complex AI interactions, some paths hard to test |
| `api` | 85% | User-facing, needs good coverage |
| `cli` | 70% | Thin wrapper, less critical |

---

## CI Failure Debugging

### Formatting Failures

```
error: Diff in /src/main.rs
```

**Fix**: Run `cargo fmt --all` locally and commit.

### Clippy Failures

```
error: unused variable: `x`
  --> src/lib.rs:10:9
```

**Fix**: Address the warning or add `#[allow(unused)]` with justification.

### Test Failures

```
test agents::test_event_scout ... FAILED
```

**Debug**:
1. Check the full output in CI logs
2. Run locally: `cargo test test_event_scout -- --nocapture`
3. Check for environment differences

### Coverage Failures

```
::error::Coverage 75.2% is below 80% threshold
```

**Fix**:
1. Download coverage report artifact from CI
2. Open `coverage-html/index.html`
3. Find uncovered lines
4. Add tests for critical paths

---

## Dependabot Configuration

`.github/dependabot.yml`:

```yaml
version: 2
updates:
  - package-ecosystem: "cargo"
    directory: "/"
    schedule:
      interval: "weekly"
    commit-message:
      prefix: "deps"
    labels:
      - "dependencies"
    open-pull-requests-limit: 5

  - package-ecosystem: "github-actions"
    directory: "/"
    schedule:
      interval: "weekly"
    commit-message:
      prefix: "ci"
    labels:
      - "ci"
```

---

## Branch Protection Rules

Recommended settings for `main` branch:

- [x] Require pull request before merging
- [x] Require status checks to pass
  - [x] check
  - [x] test
  - [x] coverage
- [x] Require branches to be up to date
- [x] Do not allow bypassing the above settings

---

## Release Process (Future)

When ready for releases:

```yaml
# .github/workflows/release.yml
name: Release

on:
  push:
    tags:
      - 'v*'

jobs:
  build:
    # Build binaries for multiple platforms
    # Create GitHub release with artifacts
```
