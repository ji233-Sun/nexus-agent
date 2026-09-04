# Scope Guard

Complete the current task with the minimum sufficient change.

## Before editing

- Read the relevant code, tests, and configuration directly. Do not work from search snippets or guesses.
- If the requirement is ambiguous or the premise is unverified, resolve that before building on it.
- State a minimal plan:
  - **Outcome** — the exact behavior requested
  - **Non-goals** — what this task will not do
  - **Files** — the smallest set expected to change
  - **Proof** — the check that will prove the change works
- Start with one implementation path. Split work only when the task has genuinely independent parts.

## While editing

- Reuse existing code, helpers, patterns, and test setup before adding anything new.
- Fix bugs at the root cause. Do not stack patches around a wrong premise.
- Add an abstraction, adapter, or config layer only for a second real caller
  in this task or a stated requirement.
- Preserve behavior outside the requested change.
- Do not design for rare or future cases nobody asked about.
- Remove code you replace. Keep an old path only when compatibility is an explicit requirement.

## Pause and confirm

Read-only discovery is always allowed. If the task has not already authorized it, get approval before:

- Materially expanding the scope or touching unrelated files
- Adding a dependency, framework, service, or new test infrastructure
- Changing a public API, schema, storage format, or wire format
- Deleting or overwriting user data, discarding uncommitted work, rewriting history, or dropping data
- Keeping two implementations of the same behavior alive

## Git checkpoints

- The user authorizes local commits in this repository after each completed,
  independently verifiable operation.
- Commit one operation at a time before starting the next so each change can be
  rolled back independently.
- Keep unrelated work out of each commit, and never push unless the user
  explicitly requests it.

## Testing

- Run the narrowest existing tests that exercise the changed behavior.
- Extend the most relevant existing test before creating a new test file.
- Add a test only when changed user-observable behavior is not covered, or when the user asks for one.
- Each new test must protect a clear acceptance criterion or regression risk.
- Do not backfill unrelated coverage or introduce test infrastructure for this task alone.
- Do not use passing tests as justification for extra abstractions or scope.

## If the plan grows

Stop when the work starts adding future-use layers, workaround stacks,
unrelated cleanup, or tests for unstated behavior. Rewrite a smaller plan
and confirm the new scope.

## Done means

- The requested behavior works and the acceptance criteria are met
- Relevant checks pass, with the exact commands and results reported
- Every touched file is necessary and the diff contains nothing unrelated
- No debug code, backup copies, dead paths, or scratch files remain
- Assumptions, limitations, and unverified runtime behavior are stated plainly
