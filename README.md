# Warhammer Meta Agent

A local, read-only meta tracker for Warhammer 40,000 competitive play. Automatically discovers tournament results and balance updates from public sources, stores them locally, and calculates meta statistics using AI-powered extraction and Rust-based calculations.

## Prerequisites

- **Rust**: Latest stable version (1.70+ recommended)
  - Install from [rustup.rs](https://rustup.rs/)
- **Cargo**: Included with Rust installation

## Quick Start

### Building the Project

Build the project in debug mode:
```bash
cargo build
```

Build in release mode (optimized):
```bash
cargo build --release
```

The binary will be located at:
- Debug: `target/debug/meta-agent`
- Release: `target/release/meta-agent`

### Running Tests

Run all tests:
```bash
cargo test
```

Run tests with all features enabled:
```bash
cargo test --all-features
```

Run tests with output:
```bash
cargo test -- --nocapture
```

Run a specific test:
```bash
cargo test test_name
```

### Code Quality Checks

Format code:
```bash
cargo fmt
```

Check formatting without making changes:
```bash
cargo fmt --all -- --check
```

Run clippy linter:
```bash
cargo clippy --all-targets --all-features -- -D warnings
```

Run all quality checks (format, clippy, tests):
```bash
cargo fmt --all -- --check && \
cargo clippy --all-targets --all-features -- -D warnings && \
cargo test --all-features
```

## Running the Application

The application is a CLI tool with several commands. See help:
```bash
cargo run -- --help
```

### Example Commands

Sync tournament data:
```bash
cargo run -- sync --once
```

Start the API server:
```bash
cargo run -- serve
```

Calculate statistics:
```bash
cargo run -- stats
```

## Development

### Project Structure

```
tournament-tracker/
├── src/
│   ├── main.rs          # CLI entry point
│   ├── lib.rs           # Library entry point
│   ├── agents/          # AI agent implementations
│   ├── api/             # HTTP API handlers
│   ├── calculate/       # Statistics calculation
│   ├── config/          # Configuration management
│   ├── models/          # Data models
│   └── storage/         # Storage abstractions
├── docs/                # Project documentation
├── tests/               # Integration tests
└── Cargo.toml          # Rust project configuration
```

### Features

The project supports optional features:
- `remote-ai`: Enable remote AI backends (OpenAI, Anthropic)

Build with features:
```bash
cargo build --features remote-ai
```

### Development Workflow

1. **Make changes** to the codebase
2. **Format code**: `cargo fmt`
3. **Check linting**: `cargo clippy --all-targets --all-features -- -D warnings`
4. **Run tests**: `cargo test --all-features`
5. **Build**: `cargo build` or `cargo build --release`

### Release Build

For production builds with optimizations:
```bash
cargo build --release
```

Release builds include:
- Link-time optimization (LTO)
- Single codegen unit for better optimization
- Stripped debug symbols

## Documentation

Comprehensive documentation is available in the `docs/` directory:

- **[Overview](docs/00_overview.md)** - Project vision, architecture, and core philosophy
- **[Build Plan](docs/01_build_plan.md)** - Development roadmap and milestones
- **[Data Model](docs/02_data_model.md)** - Entity definitions and relationships
- **[Agents](docs/03_agents.md)** - AI agent specifications and workflows
- **[Storage Layout](docs/04_storage_layout.md)** - File structure and data organization
- **[API Specification](docs/05_api_spec.md)** - HTTP API endpoints and schemas
- **[Operations & Scheduling](docs/06_ops_and_scheduling.md)** - Deployment and scheduling
- **[CI/CD & Quality](docs/07_cicd_and_quality.md)** - Continuous integration and quality gates

## Configuration

The application uses a TOML configuration file (default: `./config.toml`). See the [API Specification](docs/05_api_spec.md) for configuration details.

## Testing

The project includes:
- **Unit tests**: Located alongside source code in `src/`
- **Integration tests**: Located in `tests/integration/`
- **Test fixtures**: Located in `tests/fixtures/`

Run with verbose output:
```bash
cargo test --all-features -- --nocapture --test-threads=1
```

## Troubleshooting

### Build Issues

If you encounter build errors:
1. Update Rust: `rustup update`
2. Clean build artifacts: `cargo clean`
3. Rebuild: `cargo build`

### Test Failures

If tests fail:
1. Run tests individually to isolate issues
2. Check test output with `--nocapture`
3. Ensure all dependencies are up to date: `cargo update`

### Clippy Warnings

All clippy warnings are treated as errors in CI. Fix warnings by:
1. Following clippy suggestions
2. Adding `#[allow(clippy::lint_name)]` if necessary (with justification)

## License

MIT License - see LICENSE file for details

## Contributing

1. Ensure all tests pass: `cargo test --all-features`
2. Ensure code is formatted: `cargo fmt --all -- --check`
3. Ensure clippy passes: `cargo clippy --all-targets --all-features -- -D warnings`
4. Follow the project's coding standards and architecture principles outlined in the documentation
