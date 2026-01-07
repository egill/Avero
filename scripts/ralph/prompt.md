# Ralph Agent Instructions

## Your Task

1. Read `prd.json` (project root)
2. Read `scripts/ralph/progress.txt` (Codebase Patterns section first!)
3. Verify you're on a `ralph/*` branch (create `ralph/code-quality` if needed)
4. Pick the FIRST story with `passes: false` (lowest priority number wins)
5. Implement that ONE story following its `instructions` field
6. Run `cargo build && cargo test && cargo clippy -- -D warnings`
7. If all pass: commit `feat: [ID] - [Title]`
8. Update prd.json: set `passes: true` for that story
9. Append learnings to progress.txt

## Progress Format

APPEND to scripts/ralph/progress.txt:

```
## [Date] - [Story ID] - [Title]
- Files changed: list them
- **Learnings:** patterns/gotchas discovered
---
```

## Codebase Patterns (READ FIRST!)

Check progress.txt Codebase Patterns section before starting.
Add NEW patterns you discover to that section.

## Verification Commands

```bash
cargo build --release     # Must succeed
cargo test                # All tests must pass
cargo clippy -- -D warnings  # No warnings allowed
```

## Stop Condition

If ALL stories have `passes: true`, reply ONLY:
<promise>COMPLETE</promise>

Otherwise, end your response normally after completing ONE story.
