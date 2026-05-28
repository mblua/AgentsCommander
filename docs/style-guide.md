# Docs style guide

For contributors writing or editing AgentsCommander documentation. Six rules. Apply them in this order — earlier rules override later ones.

Every doc opens with a 1-sentence "who reads this and why" line under the H1. The line above this section is the example.

## 1. Lead with what the reader can do, not what we built

> ✅ "After this quickstart, you have two agents exchanging a message."
> ❌ "This quickstart explains AgentsCommander's messaging system."

If the first paragraph does not promise a concrete outcome, rewrite it.

## 2. Second person, present tense, active voice

> ✅ "You install the package."
> ❌ "The package is installed by the user."

> ✅ "AC writes the file to disk."
> ❌ "The file gets written to disk."

Consistency across docs is more important than personal preference.

## 3. One concept per section

Do not combine "install", "configure", and "run" into one wall of text. Each gets its own H2 (or its own page) so a reader scanning the TOC can jump to the part they need.

## 4. Show the exact command, not its description

> ✅
> ```bash
> agentscommander list-peers-lean --token "$AGENTSCOMMANDER_TOKEN" --root "$AGENTSCOMMANDER_ROOT"
> ```
>
> ❌ "Use `list-peers-lean` with your token and root."

The second form is true but useless. The first form copy-pastes.

When you show a command, also show what the user should expect to see — first 1–2 lines of output, or "exits 0 on success" if there is no stdout.

## 5. Be specific about failure

> ✅ "If you see `Error: ENOENT`, check that you are inside an `.ac-new/` project."
> ❌ "If there is an error, check your setup."

Name the error string. Name the directory. Name the file. Vague troubleshooting is worse than none.

## 6. Cut ruthlessly

If a sentence does not help the reader do something or understand something, delete it. Every word in a doc is paid for by every reader.

## Vocabulary

We use these terms consistently across all docs:

| Use | Not |
|---|---|
| Coding agent (Claude Code, Codex, Gemini) | "the AI" |
| Team | "Dark Factory", "crew" |
| Workgroup | "the session" (it is a directory) |
| Coordinator | "leader", "boss" |
| Brief | "the prompt" (briefs are persistent files; prompts are turn-by-turn) |
| Session | "tab" (sessions are PTYs; AC has no tabs) |
| Messaging | "chat", "comms" |
| Agents Agency picker | only for the role-template picker |

See [Glossary](glossary.md) for the full list.

## Words to avoid

These are banned in any public doc:

*revolutionary, unleash, supercharge, next-gen, AI-powered, game-changing, blazing-fast, seamless, magical, agentic*

If you find yourself reaching for one, ask: *what concrete capability or outcome am I trying to describe?* Then write that instead.

Also avoid: "simply", "just", "easily", "easy to use" — they age badly and gaslight users who hit friction.

## Code examples

- **Every snippet must run.** Test it in a clean environment before merging.
- **Use real values where safe.** A real GitHub URL beats `<your-username>/<your-repo>`.
- **Mark placeholders with `<angle-brackets>` or `{{double-braces}}` consistently.** Pick one per doc.
- **Show OS variation only when it matters.** PowerShell vs bash vs zsh examples for the same one-liner is usually wasted space.

## Markdown conventions

- Headings: `#` for the doc title, `##` for sections, `###` only when really needed.
- Tables for any list with two or more attributes per row.
- Fenced code blocks with a language tag (`bash`, `json`, `rust`, `markdown`).
- File paths: backticks for inline (`docs/quickstart.md`), code blocks for long ones.
- Cross-references: relative links between docs, absolute GitHub URLs for issues and external repos.

## When to add a new doc vs extend an existing one

A new doc is justified when:
- The topic has its own audience (someone who would land on this URL specifically).
- The topic is reusable from multiple other docs.
- The doc will be more than ~200 lines.

Otherwise, extend an existing doc. Forest of tiny pages is harder to navigate than a few well-organized ones.

## Reviewing docs

When reviewing someone else's doc PR, check:

1. Does the first paragraph promise a concrete outcome?
2. Does every code example run?
3. Are the vocabulary terms consistent with this guide?
4. Are there any banned words?
5. Could a developer who has never seen AC follow it end-to-end?

If anything fails, comment with the rule number from this guide so the author knows which constraint they hit.
