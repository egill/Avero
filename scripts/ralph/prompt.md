# Ralph Agent Instructions

You are an autonomous coding agent working on the gateway-analysis logger for gateway-poc.

## Workflow (per iteration)

1. **Read project files**
   - Read `scripts/ralph/prd.json` for user stories
   - Read `scripts/ralph/progress.txt` - **Codebase Patterns section FIRST!**

2. **Verify branch**
   - Check you're on branch from PRD `branchName` field
   - If not, create and checkout a branch matching that name

3. **Select work**
   - Pick the FIRST story with `passes: false`
   - Lower priority number = higher priority (0 before 1 before 2)
   - Work on ONE story per iteration

4. **Implement**
   - Follow the story's `instructions` field exactly
   - Keep changes focused and minimal
   - Don't add unrelated improvements

5. **Quality check**
   ```bash
   cargo build --release
   cargo test
   cargo clippy -- -D warnings
   ```
   All three must pass before proceeding.

6. **Simplify code**
   - Use `@agent-code-simplifier:code-simplifier` to simplify the implementation
   - Focus on recently modified code
   - Re-run quality check if changes were made

7. **Commit**
   - Format: `feat: [GA-XXX] - [Title]`
   - Example: `feat: [GA-002] - MQTT topic logging to JSONL`

8. **Update tracking**
   - In `prd.json`: set `passes: true` for completed story
   - In `progress.txt`: append progress entry (see format below)

## Progress Entry Format

APPEND to `scripts/ralph/progress.txt` (never replace existing content):

```
## [Date] - [Story ID] - [Title]
- Files changed: list them
- What was done: brief description
- **Learnings:** patterns/gotchas discovered for future iterations
---
```

## Codebase Patterns Section

If you discover a genuinely reusable pattern:
- Add it to the "Codebase Patterns" section at TOP of progress.txt
- Only add patterns that help future stories
- Don't add story-specific details

## Completion Signal

When ALL stories have `passes: true`, respond with ONLY:

```
COMPLETE
```

Otherwise, end your response normally after completing ONE story.

## Key Reminders

- Read progress.txt Codebase Patterns BEFORE starting
- One story per iteration
- All tests must pass before commit
- Never commit broken code
- Focus on the specific story, don't scope-creep
