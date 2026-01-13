# Repository Guidelines

## Project Structure & Module Organization
- `src/` holds application code: `main.rs` for the CLI/service entrypoint, `lib.rs` for shared logic, `bin/` for extra binaries, and domain/service modules under `domain/`, `services/`, `infra/`, and `io/`.
- `tests/` contains integration tests (currently `config_test.rs`).
- `config/` stores environment-specific TOML configs (`dev.toml`, `grandi.toml`, `netto.toml`).
- `scripts/` includes deployment/diagnostics helpers and related docs.
- `grafana/` and `grafana-tasks.md` cover dashboard assets and notes.

## Build, Test, and Development Commands
- `cargo build` builds all binaries in the workspace.
- `cargo run --bin gateway-poc` runs the main gateway binary.
- `cargo run --bin gateway-tui` runs the terminal UI binary.
- `cargo run --bin gate_test` runs the gate testing helper.
- `cargo test` runs unit and integration tests; `cargo test --test config_test` targets the config test.
- `cargo fmt` formats with `rustfmt.toml`; `cargo clippy` runs lint checks with `clippy.toml`.

## Coding Style & Naming Conventions
- Rust 2021 edition with `rustfmt` max width 100; keep lines short and readable.
- Follow Rust naming conventions: `snake_case` for functions/modules, `CamelCase` for types.
- Match module filenames to module names (e.g., `foo.rs` for `mod foo`).

## Testing Guidelines
- Unit tests should live near the code they cover (`src/...` with `mod tests`).
- Integration tests go in `tests/` and should read like user flows.
- Prefer descriptive test names and cover config parsing and I/O boundaries.

## Commit & Pull Request Guidelines
- Recent history uses short subjects with Conventional Commit prefixes (`feat:`, `fix:`, `refactor:`); follow that style when possible.
- Keep commit messages under ~72 characters and focused on one change.
- PRs should include a short summary, testing notes (commands run), and links to relevant issues.
- Include screenshots or recordings for TUI or Grafana changes when applicable.

## Documentation & References
- `DEPLOYMENT.md` and `REQUIREMENTS.md` capture deployment and requirement details.
- Use `docs/` and `tasks/` for design notes and ongoing work items.

## Landing the Plane (Session Completion)

**When ending a work session**, you MUST complete ALL steps below. Work is NOT complete until `git push` succeeds.

**MANDATORY WORKFLOW:**

1. **File issues for remaining work** - Create issues for anything that needs follow-up
2. **Run quality gates** (if code changed) - Tests, linters, builds
3. **Update issue status** - Close finished work, update in-progress items
4. **PUSH TO REMOTE** - This is MANDATORY:
   ```bash
   git pull --rebase
   bd sync
   git push
   git status  # MUST show "up to date with origin"
   ```
5. **Clean up** - Clear stashes, prune remote branches
6. **Verify** - All changes committed AND pushed
7. **Hand off** - Provide context for next session

**CRITICAL RULES:**
- Work is NOT complete until `git push` succeeds
- NEVER stop before pushing - that leaves work stranded locally
- NEVER say "ready to push when you are" - YOU must push
- If push fails, resolve and retry until it succeeds
