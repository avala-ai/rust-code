---
description: Run a prompt or slash command on a recurring interval until a condition holds
whenToUse: when the user wants to poll for a status, retry an operation, or repeat a check on a cadence (deploy status, CI green, file appears)
userInvocable: true
---

Loop on the condition the user described until it holds, the ceiling is reached,
or a non-transient error stops you. This skill is the CLI-side ergonomic — it
describes the loop the agent runs in this session. For a true daemon-side
recurring schedule that survives the session, use the cron / scheduled-routines
tools instead; this skill is prompt-only and lives only as long as the chat.

Before starting:

1. State the **exit condition** in one sentence — what is true when we're done.
2. State the **check** — the exact command, tool call, or expression that
   evaluates the condition.
3. State the **interval** (default 10s) and the **ceiling** (default 60
   iterations). Exit early if the ceiling is reached; never loop past it
   without explicit user approval.

Then loop:

   a. Run the check.
   b. If the condition holds, stop and report success with the final state.
   c. If it doesn't, print one compact line (iteration N, what the check
      returned), sleep, and continue.

On a transient error (network, 5xx, rate limit), back off exponentially with a
60s cap and keep counting — do not reset the iteration counter. On a
non-transient error (auth failure, 4xx, permission denied), STOP — a loop
won't fix it; surface the error to the user.

You're done when:
- The condition holds and you've printed the success line, or
- The ceiling is hit and you've printed `ceiling reached, last state was X`, or
- A non-transient error stopped you and you've surfaced it.

Never silently extend the ceiling. Never claim success without running the
final check.
