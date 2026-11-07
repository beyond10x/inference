---
format: aep.planning-md/3
id: decision-blocker:wave-branch-committer
kind: decision-blocker
status: cleared
title: The third SDK wave branch carries a commit Gates refuses
relations:
- blocks: story:sdk-recipes
revision: 3
transitions:
- {from: "open", to: "cleared", at: "2026-11-07T17:28:21Z", actor: "human:timo", revision: 3}
---
The pre-push Gates check refuses the third SDK wave's branch: commit 2ce2b5b, a copy of main's 1a25038 (same patch) placed under the wave's base, carries a personal committer identity. It was never pushed. Dropping it means rebuilding the unpushed branch from `origin/main` without it, which is a history rewrite and needs the owner's word; the alternative is a Gates policy exception.

Decision needed: rebuild the branch without 2ce2b5b, or admit the commit by exception.

## Decided

Rebuild the branch from `origin/main` without 2ce2b5b, by bot cherry-pick of the other 21 commits. The rebuilt head 122f0eb has tree b05b179, the tree the full check passed on (as ca819a6), so that run stands. Old to new: 45400bf→61ef983, 380791d→0327137, 90bd6cc→5047c8c, 8e8c87b→1717b5f, e8ea264→792f2eb, a60e8df→22435b4, 18fe355→65c1942, 6a98498→0cc999b, f2baecb→5e66c9f, ca819a6→122f0eb.
