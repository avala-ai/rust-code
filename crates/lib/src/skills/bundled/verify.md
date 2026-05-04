---
description: Independent verification pass after a non-trivial implementation
whenToUse: after a significant change is implemented and before merge or release; when a claim was made that the agent is about to act on
userInvocable: true
---

Run an independent verification pass that doesn't trust the original
implementer's narrative. The goal is to find what broke before someone else
does, and to surface anything load-bearing that the diff implicitly relies on.

Steps:

1. **State the claim** in one sentence — "this change does X without breaking
   Y." If the claim isn't crisp, ask the user to sharpen it before
   verifying.
2. **Pick the source of truth.** For verification to mean something, the
   evidence must be primary: the code itself (not an AI summary of it),
   a test result, a command's exit code, a file's bytes, a runtime check.
   Do NOT verify one model-generated claim with another — that's circular.
3. **Read the full diff** against the base branch. Note every changed
   public surface (function signatures, exported types, config keys,
   CLI flags, tool contracts).
4. **For each changed public surface, check the blast radius:**
   - Use grep / LSP to find every caller. Does the change break any
     caller's assumptions on argument shape, return shape, error
     semantics, or side effects?
   - Is there a test that exercises the new behavior? If not, flag it.
   - Are the edge cases addressed: empty input, very long input, unicode,
     concurrent callers, partial writes, retries, cancellation?
5. **Run the gates the user named or the project's defaults** (lint,
   tests, type-check). Capture exit codes and any failure output. Do
   not paraphrase failures; cite the exact text.
6. **Cross-check the PR / commit narrative.** If the message claims
   "now faster", "also fixes X", "no behavior change" — verify each
   claim with code. If a claim has no evidence, flag it.
7. **Produce a checklist** of findings sorted by severity
   (critical / high / medium / low / info). For each: `file:line —
   one-sentence impact — proposed remediation`. If the change is
   genuinely clean, say so explicitly — do not invent findings.

STOP after step 7. This skill does NOT auto-fix what it finds; the lead
agent or the user decides what to address. You're done when every changed
public surface has been checked against its callers, the test gate has
been run, the narrative has been cross-checked, and the checklist is in
front of the user.
