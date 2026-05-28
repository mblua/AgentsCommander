---
name: ship
description: Ship current branch to main (commit, push, update from origin/main, merge, cleanup). Use when done with a feature/fix branch.
disable-model-invocation: true
---

# Ship Branch to Main

Ship the current branch through the full merge lifecycle. Execute each step in order. If any step fails, STOP and report the issue. Do not continue.

## Pre-flight

1. Run `rtk git status` and `rtk git branch --show-current` to identify the current branch
2. If on `main`, ABORT with: "You are already on main. Switch to a feature branch first."
3. Show the branch name and ask the user to confirm before proceeding

## Step 1: Commit & Push

1. Run `rtk git status` to check for uncommitted changes
2. If there are changes:
   - Run `rtk git diff` and `rtk git log --oneline -3` to understand context and commit style
   - Stage relevant files (prefer specific files over `git add .`)
   - Create a commit following the repo's commit message conventions
3. Push the branch to origin: `rtk git push origin <branch>`
   - If the remote branch doesn't exist yet, use `rtk git push -u origin <branch>`

## Step 2: Update from origin/main

1. Fetch latest: `rtk git fetch origin`
2. Check if origin/main has new commits: `rtk git log HEAD..origin/main --oneline`
3. If there are new commits, merge: `rtk git merge origin/main`
4. If there are merge conflicts:
   - Report the conflicting files
   - STOP and ask the user to resolve them
   - Do NOT continue until the user says conflicts are resolved
5. If merge brought new commits, push the updated branch: `rtk git push origin <branch>`

## Step 3: Merge to main

1. Switch to main: `rtk git checkout main`
2. Update local main: `rtk git merge origin/main`
3. Merge the branch with: `rtk git merge --no-ff <branch>`
   - The `--no-ff` preserves the branch history as a merge commit
4. Push main to origin: `rtk git push origin main`

## Step 4: Verify & Cleanup

1. **Verify the merge landed on origin**: `rtk git log origin/main --oneline -3`
   - Confirm the merge commit is visible
   - If NOT visible, STOP and report the issue. Do not delete anything
2. Only after confirming the merge is on origin/main:
   - Delete local branch: `rtk git branch -d <branch>`
   - Delete remote branch: `rtk git push origin --delete <branch>`
3. Final status: `rtk git branch -a` to confirm cleanup

## Safety Rules

- NEVER force-push, force-delete, or use `--force` flags
- NEVER skip verification before deleting branches
- If anything unexpected happens, STOP and ask the user
- All commands must be prefixed with `rtk` per project conventions
