---
name: self-handoff
description: Structure, write and shorten SELF-HANDOFF.md. Load before writing or updating a handoff for self-handoff-and-clear, self-handoff-and-switch or self-handoff-and-restart, including handoffs required by Role.md or skill rules.
---

# Self-handoff

The clear erases only the conversation. Your context file, `Role.md` and skill index reload on resume; memory and canonical files stay on disk. The handoff carries only session state they lack.

## Sections

Use these headings in this order. Fill each with concrete facts or `none`.

1. **First action**: one concrete step with the exact path or command.
2. **Active task**: issue or ticket, one-line goal, and the current workflow step, named, not explained.
3. **Where the work is**: repo, branch, last commit, PR, and paths to canonical state (plan, status files). Point to them; do not copy them.
4. **In flight**: messages awaiting reply (peer, filename, what is expected), running CI or commands, requested reservations or locks.
5. **Coordination**: who gives you orders now (room or FQN) and active delegations, with their source.
6. **Pending decisions and temporary holds**: what awaits the user or coordinator; temporary holds with their source and expiry; lessons not yet saved, as `Pending to save:`.
7. **Next**: the next queued task, named only.

## Never include

- Identity, role, language or answer rules.
- Skill procedures: name the skill instead.
- Closed work, and anything already in `SELF-FORGET.md`.
- Round history, hash lists or status revision numbers: re-read live state on resume.
- Credentials.
- Durable lessons: save them to the owning skill or memory before the clear.

## Shorten

1. Delete everything listed under "Never include".
2. Count the words.
3. Rewrite to half or fewer. Paths, commands, SHAs, message filenames and quoted evidence are not counted and not cut. Never cut a fact; stop at the smallest size that keeps every fact.
4. Shorten by removing, never by compressing: plain sentences, no glued words, no invented abbreviations.
5. End the file with `Words: draft <N>, final <M>.`

More than 400 final words usually means content copied from elsewhere: recheck "Never include".
