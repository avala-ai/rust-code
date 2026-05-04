---
description: Review changed code for reuse, quality, and efficiency, then optionally fix what's found
whenToUse: after a non-trivial implementation, before PR, when the diff feels heavier than the change required
userInvocable: true
---

Inspect the current git diff against the base branch and flag anything that can
go away or tighten without changing behavior. This skill is read-only by
default; the user opts into the cleanup pass at the end.

Steps:

1. **Read the diff** against the base branch (`git diff <base>...HEAD`).
   Don't refactor outside the diff — adjacent code is out of scope.
2. **Hunt for dead weight:**
   - Unused imports, variables, parameters.
   - Dead branches, unreachable code, redundant guards.
   - Helpers and wrappers with one caller that just rename a call.
   - Premature abstractions: a single-impl trait, a single-caller generic,
     a config knob with one valid value.
   - Speculative error handling: try/catch around infallible code,
     validation for invariants the type system already guarantees.
   - Defensive copies of immutable data.
   - Verbose names that fight the surrounding code's style.
   - Comments that restate the code instead of explaining the why.
3. **Hunt for reuse:** new code that duplicates an existing helper. Cite the
   existing helper's path and signature.
4. **Hunt for efficiency only when it's obvious:** O(n^2) where O(n)
   trivially fits, allocations in hot loops, repeated work that could be
   memoized. Don't speculate about performance you haven't measured.
5. **Print findings** as a list with `file:line — observation — proposed
   change`. Group by category (dead weight / duplication / efficiency).
   If the diff is genuinely lean, say so — do not invent findings to
   justify the review.
6. **STOP for confirmation.** Ask the user: apply all, apply some, or
   leave the findings as a checklist? Wait for explicit "go" before any
   write. If they pick "some", let them name which.
7. **Apply the approved changes** one finding at a time. After each, run
   the project's lint+test gate. If anything fails, stop and surface
   what broke — do not push through.

You're done when either (a) the user accepted the findings as a list and
no edits were made, or (b) every approved fix is applied and the gate is
green. Never touch code whose behavior is load-bearing in a way that isn't
obvious from reading it — call that out instead of changing it.
