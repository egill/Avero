0a. Study `specs/*` with up to 500 parallel Sonnet subagents to learn the application specifications.
0b. Study @IMPLEMENTATION_PLAN.md or @prd.md (if present) to understand the task list.
0c. For reference, the application source code is in `src/*`.
0d. **IMPORTANT:** Before making any changes, read `.ralph/errors.log` (if it exists) to learn from previous iteration failures. Address any recurring issues before starting new work.
0e. **GUARDRAILS:** Read `.ralph/guardrails.md` to understand active Signs (constraints and warnings). These Signs represent learnings from past mistakes - follow them before making any code changes.

## Story Selection
If using a PRD file with user stories in `### [ ] US-XXX: Title` format:
- Auto-select the FIRST unchecked story (marked with `[ ]`) to work on
- Stories marked with `[x]` are complete and should be skipped
- Work on ONE story per iteration

1. Your task is to implement functionality per the specifications using parallel subagents. Follow @IMPLEMENTATION_PLAN.md and choose the most important item to address. Before making changes, search the codebase (don't assume not implemented) using Sonnet subagents. You may use up to 500 parallel Sonnet subagents for searches/reads and only 1 Sonnet subagent for build/tests. Use Opus subagents when complex reasoning is needed (debugging, architectural decisions).
2. After implementing functionality or resolving problems, run the tests for that unit of code that was improved. If functionality is missing then it's your job to add it as per the application specifications. Ultrathink.
3. When you discover issues, immediately update @IMPLEMENTATION_PLAN.md with your findings using a subagent. When resolved, update and remove the item.
4. When the tests pass, update @IMPLEMENTATION_PLAN.md, then `git add -A` then `git commit` with a message describing the changes. After the commit, `git push`.
5. **ADDING SIGNS:** When you encounter an error pattern more than once (same mistake repeated), add a new Sign to `.ralph/guardrails.md`. Follow this format:
   ```
   ## 🚧 [Short Title]
   **Trigger:** [When this applies]
   **Instruction:** [What to do]
   **Added after:** [What mistake led to this]
   **Example:** [Concrete example]
   ```
   This prevents future iterations from repeating the same mistakes.

## Story Completion
When a story's acceptance criteria are ALL satisfied:
- Mark the story as complete by changing `### [ ] US-XXX:` to `### [x] US-XXX:` in the PRD file
- If ALL stories in the PRD are marked complete (all have `[x]`), output the following signal:
  `<promise>COMPLETE</promise>`
- This signal tells the loop that all work is done

99999. Important: When authoring documentation, capture the why — tests and implementation importance.
999999. Important: Single sources of truth, no migrations/adapters. If tests unrelated to your work fail, resolve them as part of the increment.
9999999. As soon as there are no build or test errors create a git tag. If there are no git tags start at 0.0.0 and increment patch by 1 for example 0.0.1  if 0.0.0 does not exist.
99999999. You may add extra logging if required to debug issues.
999999999. Keep @IMPLEMENTATION_PLAN.md current with learnings using a subagent — future work depends on this to avoid duplicating efforts. Update especially after finishing your turn.
9999999999. When you learn something new about how to run the application, update @AGENTS.md using a subagent but keep it brief. For example if you run commands multiple times before learning the correct command then that file should be updated.
99999999999. For any bugs you notice, resolve them or document them in @IMPLEMENTATION_PLAN.md using a subagent even if it is unrelated to the current piece of work.
999999999999. Implement functionality completely. Placeholders and stubs waste efforts and time redoing the same work.
9999999999999. When @IMPLEMENTATION_PLAN.md becomes large periodically clean out the items that are completed from the file using a subagent.
99999999999999. If you find inconsistencies in the specs/* then use an Opus 4.5 subagent with 'ultrathink' requested to update the specs.
999999999999999. IMPORTANT: Keep @AGENTS.md operational only — status updates and progress notes belong in `IMPLEMENTATION_PLAN.md`. A bloated AGENTS.md pollutes every future loop's context.
