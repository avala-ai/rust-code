---
description: Step back, re-evaluate, and pick a fresh approach when the agent has been spinning on the same problem
whenToUse: when the same fix has been retried multiple times, when error messages keep changing in shape but not in essence, when the user asks the agent to "try something different"
userInvocable: true
---

You're stuck. The purpose of this skill is to break a doom loop, not to give
up — step back, find the assumption that's wrong, propose alternatives, and
let the user pick a fresh path.

Steps:

1. **Reconstruct what was tried.** Read the last ~10 messages of this
   conversation and produce a numbered list of every distinct attempt:
   what was changed, what the result was, why it was thought to fix
   things.
2. **Find the shared assumption.** Every attempt above was built on some
   premise — the bug is in module X, the failure is a config issue, the
   test is correct and the code is wrong, etc. Name that premise in one
   sentence. That premise is usually the thing that's wrong.
3. **Propose 3 alternative approaches** that don't rely on that premise.
   Make them genuinely different in kind, not three flavors of the same
   idea:
   - A different file or module to investigate.
   - A different abstraction level (add logs vs read code; rebuild vs
     patch-fix; reproduce in isolation vs trace in place).
   - A different tool (run the program under a debugger; bisect the
     history; ask whether the spec is right rather than the code).
4. **STOP and ask the user to pick.** Show all three and a one-line cost
   for each ("approach A: 5 minutes, low risk; approach B: 30 minutes,
   requires rebuilding container"). Wait for their pick. Do NOT auto-
   select; the point is to reset judgement, not to keep moving.
5. **Take exactly one concrete step on the chosen approach** and stop
   for a check-in. Do not chain into a new doom loop on the new
   approach — confirm the first step actually moves the needle before
   continuing.

You're done when the user has picked an approach and you've taken one
step that produced new information (a log line, a different error, a
ruled-out hypothesis). This is not "give up" — it's "stop digging in the
same spot, sample a new spot." Never abandon the goal; only abandon the
current path.
