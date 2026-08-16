# In-code comment convention

Use this page when you add or update comments in hand-authored source, test, or fixture code.

## Scope

Apply rules 1-3 in every language and rules 4-7 only in Rust. Change generated or vendored comments at their source instead of editing generated output. Treat comment-like text inside documentation examples as documentation.

## Rules for every language

1. **Comment only non-inferable information.** Record rationale, a contract, a constraint, or a correctness fact that is expensive to infer from nearby code. Do not narrate syntax or visible control flow, and do not comment self-explanatory code.
2. **Put local correctness information next to the relevant code.** Use the language's native comment syntax for design rationale, a safety proof such as Rust `// SAFETY:`, a concurrency invariant, a wire or input-format fact, or a sequencing constraint. Explain why a tempting alternative is wrong when that distinction matters.
3. **Keep history and comments current.** Delete commented-out code because version control retains it. When a behavior change invalidates a comment, update or remove that comment in the same change. Prefer removal over a claim you cannot keep accurate.

## Rust rules

4. **Attach documentation to its subject.** Use `///` on an item instead of a floating `//` when the text documents that item. Use `//!` only for crate-level or module-level documentation.
5. **Bound the summary and avoid redundant restatement.** Start a doc comment with at most one concise summary sentence or noun phrase, then spend words on relevant Why, How-to-use, and Property information. Do not repeat facts already clear from the signature or body. Name a parameter when you document non-inferable units, valid ranges, sentinel values, ownership, escaping, platform behavior, or another externally relevant semantic contract.
6. **Make standard sections earn their cost.** Add `# Errors`, `# Panics`, and `# Safety` only when they state externally relevant, non-inferable conditions or obligations; do not add boilerplate or empty headings. For a public, deterministic, setup-light API with a non-obvious protocol, add a minimal runnable `# Examples` doctest. For a private, `pub(crate)`, runtime-bound, or OS-bound API that cannot use a self-contained doctest, document the protocol precisely in prose and cover the behavior with the nearest unit or integration test. Do not omit a necessary protocol because a runnable doctest is impractical, and do not copy a large test setup into the doc comment.
7. **Keep module docs contractual when present.** State the module's responsibility in a module-level `//!` comment. Add only invariants and call-graph or call-order relationships that exist, matter, and are non-obvious. Do not inventory every item or invent a relationship to fill a template.
