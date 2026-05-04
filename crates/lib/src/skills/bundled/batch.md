---
description: Apply the same change across multiple files or branches with a preview/confirm step
whenToUse: when the user wants to perform the same edit across many files (rename, import, header) or replay a fix across multiple branches/worktrees
userInvocable: true
---

Apply the requested change uniformly across the targets the user named — many files
in the current tree, or many branches/worktrees. Preview every target before any
write, and stop on the first failure rather than papering over it.

Steps:

1. **Resolve the target set.** Ask the user to confirm the glob, directory, or
   branch list if it isn't already explicit. Print the resolved list (one per
   line, with a count). If the count looks unexpected (zero, or far more than
   the user implied), STOP and confirm before proceeding.
2. **Compute the per-target diff.** For a file-level batch, build the patch
   for each file using a single rule but adapted per file (e.g. import paths
   may differ, neighboring code may differ). For a branch-level batch, plan
   to enter each worktree fresh — never mutate the current working tree.
3. **STOP for confirmation.** Show the user the first 3 diffs in full plus a
   summary of the rest ("23 more files, all matching pattern X"). Wait for
   explicit "go" before any write. If the user wants to refine the rule,
   restart from step 2.
4. **Apply, one target at a time.** After each write, run the project's test
   and lint gate (or at minimum a syntax check). If a target fails, stop on
   that target, record what broke, and surface it to the user — do NOT keep
   going. Never force the same patch onto a target where it doesn't apply
   cleanly.
5. **Report.** Print a table: target | result (changed/unchanged/failed) |
   files touched | commit SHA if committed. Leave any failed worktrees in
   place for inspection.

You're done when every target is either successfully changed and verified,
or explicitly recorded as "skipped" or "failed" with a reason the user can
act on. Never claim success for a target you didn't actually verify.
