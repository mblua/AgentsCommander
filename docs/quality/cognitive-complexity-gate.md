# Cognitive complexity gate

AgentsCommander caps a function's cognitive complexity at **25**. Every pull request is measured
by Clippy on Windows, and on Linux and macOS as well when the run is a full-tier run (section 6);
the measured sites are checked against a committed baseline.
This page says what the gate blocks, how to get past a failing check, and exactly what the
measurement cannot see.

The gate has two parts:

- `test-debt` runs the suppression scan and the baseline ratchet through
  `scripts/check-cognitive-complexity.mjs`.
- each of the three Rust legs - `rust-regression` (Windows), `rust-regression-linux` and
  `rust-regression-macos` - runs

  ```bash
  cargo clippy --locked --workspace --all-targets --message-format=json \
    -- -D warnings --force-warn clippy::cognitive_complexity
  ```

  and hands the JSON capture to the same script. `test-debt` and the Windows leg run on every
  pull request event; the Linux and macOS legs run only in the full tier (section 6).

The threshold comes from the root `clippy.toml`, pinned there so a future change of Clippy's
default cannot move the gate:

```toml
cognitive-complexity-threshold = 25
```

`--force-warn` makes the cognitive lint report without failing the build for it, and no lint
attribute can silence it. The baseline, `cognitive-complexity.baseline.json`, records the sites
that were already above 25 when the gate was adopted; it is JSON, so it cannot carry a comment
and this page is linked from `clippy.toml` instead.

## 1. What the gate blocks, and how to fix it

Every site that Clippy reports above 25 is either **NEW** (not in the baseline) or **STALE** (in
the baseline but no longer observed). Either one fails the leg. A NEW finding prints the Clippy
diagnostic first and then a line like the first one below; a STALE finding prints only its line,
because Clippy no longer reports that site:

```text
NEW cognitive complexity above 25: rust:src-tauri/src/agent_update.rs::impl:TargetProcessOwner::settle#a3d53f9cd47b (observed 1, baseline absent) on windows
STALE baseline entry: rust:src-tauri/src/agent_update.rs::impl:TargetProcessOwner::settle#a3d53f9cd47b (baseline 1, observed none) on windows
```

Both lines sample the message format, not a real gate run: the id and anchor name a real baselined
site, and only the parenthesised state is illustrative.

### NEW: a function that was not baselined

Fix it by **bringing the function to 25 or below**. No attribute will help:
`#[allow(clippy::cognitive_complexity)]` and `#[expect(...)]` cannot lower a force-warned lint,
and simply writing either of the spellings `cognitive_complexity` or `cognitive-complexity` in any
`.rs` file fails scan rule S1 in `test-debt` on its own. Regenerating the baseline does not
legitimise the new debt: the baseline may only shrink, and its rules are in section 3.

### STALE: a baselined entry that no longer matches

The usual cause is that the debt is gone. Then **delete or shrink the entry in the same pull
request that fixed the debt**, so the shrink is accepted (section 3). The other cause is that the
site's header text changed - a rename, a move, or a signature edit - which moves its site anchor
without anyone touching the complexity. There is no re-anchoring path for that; the way out is
again to reduce the function to 25 or below, so there is no debt left to re-anchor, and remove the
entry in the same pull request. Section 4 lists the edits that bounce this way.

To reproduce a failure locally:

```bash
npm run cog:self                                    # the detector's own tests
npm run cog:scan                                    # suppression scan, rules S1-S4
npm run cog:baseline -- --base-ref origin/main      # the baseline ratchet
```

## 2. What Clippy does not measure

The limits below are limits of the tool, measured at the pinned toolchain **rustc/Clippy
1.98.1**, not gaps this repository chose to leave. No other analyser is provided: a form listed
here is not measured by any part of the gate, and covering it would mean adding a different tool,
not changing this configuration.

### 2.1 Bodies produced by a `macro_rules!` expansion

Clippy does not report a body that an expansion produced. The qualifier is narrow, and the test is
the **origin of the item, not of the body text**:

- A **pass-through** macro - a `macro_rules!` or an attribute proc-macro that re-emits the item it
  received - **does** report normally. `pass!(fn pt() { ... })`, where the whole item is written
  at the call site and re-emitted, **is** reported.
- A macro that synthesises the item is **not** reported:

  ```rust
  macro_rules! make { ($name:ident, $body:block) => { fn $name() $body } }
  make!(f, { /* body written at the call site */ });
  ```

  is not reported even though the body came from the call site, because the `fn` item itself is
  synthesised by the expansion. A macro that generates the body outright is not reported either.

"The body was written by a human at the call site" is **not** the test, and saying so would
overstate the coverage.

### 2.2 `async` blocks and the tokio entry-point macros

Clippy does not report these forms, whatever their complexity:

- `let f = async move { ... };`
- an `async move { ... }` block inside an `async fn`; both this and the previous form emit nothing
  even at complexity 31;
- `#[tokio::test] async fn`;
- `#[tokio::main] async fn`.

An ordinary `pub async fn` **is** reported, and so is an ordinary `#[test] fn`.

The lexical weight of the excluded forms at adoption, counted with `ripgrep` over `src-tauri` and
`crates` and reported as occurrences, **not** as violations. They were measured on
`28e2180a94ee6d6da93b7f4ad0c0ebd076309471`, the phase 7 merge that adopted the gate; the counts
move with every pull request, so they are a snapshot of that commit, not a property the gate
maintains:

```bash
rg -o '#\[tokio::test' src-tauri crates | wc -l     # 1047
rg -o '#\[test\b' src-tauri crates | wc -l          # 4418
rg -o 'async move \{' src-tauri crates | wc -l      # 503
rg -o 'async \{' src-tauri crates | wc -l           # 140
rg -o '#\[tokio::main' src-tauri crates | wc -l     # 2
```

Nobody has measured how many of those occurrences would exceed 25; they are counts of text.

### 2.3 Code the platform does not compile

Clippy only measures code that the leg compiles, so `cfg`-excluded code is invisible on the
platform that excludes it. Every leg measures the whole workspace and all targets (section 6), so
in a full-tier run the residual here is code excluded on all three platforms at once. In a
light-tier run only Windows measures, so code that Windows excludes is not measured by that run;
section 6.2 says what that means for a pull request.

## 3. The baseline may only shrink

`cognitive-complexity.baseline.json` records accepted debt; it may not grow. The ratchet allows
exactly one exemption.

- **An addition fails.** Any anchor in the head baseline that was not in the base prints
  `Baseline grew without a toolchain refresh: <id>#<anchor> (<platform>)` followed by
  `The baseline may only shrink. Regenerating it does not legitimise new debt.`
- **A shrink must be earned.** Removing an entry is accepted only when the pull request touched
  the entry's `.rs` file (it appears in `git diff --name-only <base>...HEAD`) or the file is gone
  from the working tree. Otherwise the step prints
  `Baseline entry removed without touching its file: <id>` followed by
  `A shrink is only credible where the pull request changed the code. Include that file, or explain the removal in a toolchain-refresh pull request.`
- **The single exemption is a toolchain refresh.** If the diff contains **no `.rs` path** and the
  head baseline's `toolchain` is **strictly higher** than the base's (numeric comparison of the
  dotted components, left to right), additions are accepted and the step prints
  `TOOLCHAIN REFRESH: <base> -> <head> (<n> additions, <m> removals)`. The reason is that a
  higher Clippy can measure more, so
  the recorded set legitimately grows with it - and only then may the baseline grow.
- **A downgrade does not qualify.** An equal or lower version is not a refresh: the exemption
  exists for a *stricter* measurement, and a lower toolchain is a weaker one, so growth under it
  would bless new debt with an older tool rather than account for new rules. In practice the run
  toolchain must also equal the baseline's `toolchain` field, so lowering the pin without editing
  the baseline makes every leg fail with `baseline toolchain must be ...`; editing it does not make
  the diff exempt, because the direction test still rejects it.

## 4. Renames, moves and header edits bounce by design

The baseline matches **sites**, not names. These edits change the id or the site anchor of a
baselined site, so the old entry becomes STALE and the new one is NEW - one of each, and the gate
asks for a fix:

- renaming the function, or any scope in its container path;
- moving it to another file;
- moving it between two `impl` blocks, or in or out of a `trait`;
- editing its signature;
- editing a closure's parameter list.

There is no automatic re-anchoring. The remedy is section 1: reduce the function to 25 or below
and remove the entry in the same pull request that touched the file.

Growth is deliberately different: a baselined function may keep growing **in its body**, at any
complexity, without bouncing. The anchor is computed from the site's header text and stops at the
body brace, so accepted debt stays accepted while it is being paid down (or not). Section 5.5
gives the same story for whitespace.

## 5. Identity: the id, the container and the site anchor

### 5.1 The id shape

Every measured site gets a stable id:

```text
rust:<file>::<container>::<name>
```

- `<file>` is the repository-relative POSIX path of the source file.
- `<container>` is the `::`-joined path of the scopes that **really enclose** the site, outermost
  first: `mod:<name>`, `trait:<name>`, `fn:<name>`, and `impl:<header>`. The `impl` token is the
  normalised `impl` header - trait, generics, bounds and lifetimes all included, with a trailing
  `where ...` clause removed. It is what tells `impl A` and `impl B` apart, and `impl Display for
  F` from `impl Debug for F`. Nothing is cut at `<`.
- `<name>` is the function or method name; a closure is `{closure}`.

A closure inside `run` in `src-tauri/src/lib.rs` therefore ids as:

```text
rust:src-tauri/src/lib.rs::fn:run::{closure}
```

### 5.2 The site anchor

An id names a bucket; the **site anchor** names one site inside it. The anchor is the first 12
lowercase hex characters of the SHA-256 of the site's own header text: the text from the site's
position to the `{` that opens its body, with whitespace runs collapsed to one space and comments
and strings contributing nothing. The **body is not in it** - that is what lets a baselined
function grow (section 4). A signature edit **does** move it.

The baseline stores, for each id, the sorted list of anchors observed per platform; a platform that
observed no site of an id has no entry for it there. For example:

```json
{
  "id": "rust:src-tauri/src/agent_update.rs::impl:TargetProcessOwner::settle",
  "sites": {
    "windows": ["a3d53f9cd47b"],
    "linux": ["a3d53f9cd47b"],
    "macos": ["a3d53f9cd47b"]
  }
}
```

Comparing anchors as a multiset, not by count, is what closes R-C1: replacing one site with
another inside one id is one STALE plus one NEW, and the ratchet refuses the addition.

### 5.3 R-C1 is closed; R-C1' is the one open limitation

- **R-C1 is closed.** Fixing one site and adding another above 25 in the same function is one
  STALE plus one NEW and fails.
- **R-C1' is open and declared.** Two sites of one id whose header text is byte-identical after
  whitespace collapsing share one anchor, so a swap between them is invisible to the ratchet. The
  shape a reader will actually meet is **two bare `|| {` closures in one function**: a closure's
  span starts at the `|`, so `move` sits outside the text and every parameterless closure of that
  function carries the same header text. The same holds for a repeated parameter name:
  `|_| {` or `|e| {`, repeated in one function, collides with itself. The typed
  `|x: u32|` example is not the common case.
  The prevalence is **unmeasured**: nobody has counted how many multi-closure ids hold two
  byte-identical headers, so no rate is quoted here. The price of this design is stated in section
  4: editing a signature or a closure's parameter list on baselined debt now bounces.

### 5.4 Two items sharing one id across `cfg`

Two items that share a path under mutually exclusive `cfg` attributes share an id. The
per-platform anchor arrays keep them apart, because each platform compiles only its own site.
The exception is a `#[cfg(test)]` and `#[cfg(not(test))]` pair: both compile on one platform
under `--all-targets`, so that id holds two sites there. When their headers read the same, the two
sites also share one anchor - an instance of R-C1' above.

### 5.5 `impl` headers and site headers are whitespace-sensitive

Editing an `impl` header renames **every** id inside the block, so it bounces like a rename
(section 4). The normalisation collapses whitespace runs but does **not** remove a trailing comma,
so a multi-line generic list

```rust
impl<
  A,
  B,
> Foo<A, B> { ... }
```

normalises to `< A, B, > Foo<A, B>` while the one-line spelling `impl<A, B> Foo<A, B> { ... }`
gives `<A, B> Foo<A, B>`. Reformatting such a header therefore renames every id in the block with
no change of meaning.

The site anchor normalises the same way: `f(a: u32) {` and a multi-line header with `a: u32,`
before `) {` give different anchors, so reflowing a baselined site's own header bounces it exactly
as an edit would.

No case pins CRLF against LF, and none needs to: each platform captures and enforces against its
own checkout, so a line ending never crosses between the two sides of a comparison.

## 6. Coverage: three platforms, the whole workspace, two tiers

### 6.1 Which leg runs when

All three Rust legs run the same command over `--workspace --all-targets`, so in a run where a leg
runs, a function above 25 is measured on that platform wherever it compiles, tests and benches
included, and macOS is no longer narrower than Windows and Linux.

Not every run has all three legs. `.github/workflows/pr-regression-gates.yml` splits its jobs into
two tiers, decided by its `ci-tier` job:

- **Light tier, on every event:** `test-debt` (suppression scan and baseline ratchet),
  `rust-regression` (Windows leg) and `rust-fmt`.
- **Full tier, only when `ci-tier` sets `full=true`:** everything else, including
  `rust-regression-linux` and `rust-regression-macos`.

`ci-tier` sets `full=true` when any of these holds:

- the run was started by `workflow_dispatch`;
- the pull request carries the `ci:full` label. Adding the label starts a run by itself, because
  the workflow also triggers on `labeled`, and while the label stays on, every later push runs the
  full tier too;
- the workflow's `github.run_number` is a multiple of 10. The counter is shared by every run of
  this workflow in the repository, so it is roughly one run in ten, not one pull request in ten,
  and nobody chooses which.

So on every pull request the Windows leg and the baseline ratchet enforce, and the Linux and macOS
legs enforce only in full-tier runs. Issue #2423 tracks a planned change to measure Linux, and if
possible macOS, on every pull request; until it lands, the tiers above are the behaviour.

### 6.2 A Linux- or macOS-only finding can surface on someone else's pull request

A site that only Linux or macOS compiles - typically code under a `cfg` for that platform - is not
measured by a light-tier run. A pull request that adds such a site above 25, or that fixes or
moves a baselined site only those platforms observe without removing its entry, can therefore pass
its light-tier checks and be merged. The finding then appears as `NEW` or `STALE ... on linux` or `on macos` in the next
full-tier run of **any** pull request based on that main, which did not cause it.

If that happens to your pull request:

1. Check whether your diff touches the reported file. If it does not, the finding came from main,
   and your change is not the cause.
2. Do not grow the baseline to get past it: the ratchet in section 3 refuses the addition. A
   `STALE` entry for a file your pull request did not touch cannot be removed there either, because
   a shrink is only accepted where the pull request touched that file.
3. Fix it at the source: a separate pull request that brings the function to 25 or below (for
   `NEW`), or that touches the file and removes the entry (for `STALE`), labelled `ci:full` so its
   Linux and macOS legs run. Once it merges, merge main into your pull request and rerun.

To avoid causing this, add the `ci:full` label **before merging** any pull request that changes
code under a Linux- or macOS-only `cfg`, or that edits or removes baseline entries whose sites only
those platforms observe. Its checks then run all three legs.

### 6.3 One invocation on macOS

macOS carries one invocation, not two: the workspace measured clean under `-D warnings` there, so
enforcement and measurement share a single compile. The alternative shape - keeping the enforcing
invocation on `src-tauri` only and adding a second, measurement-only invocation over the whole
workspace - would cost **one extra macOS compile per full-tier run**. It is not in use.

Because a leg sees only what it compiles, the residual scope of a full-tier run is the code
excluded on all three platforms at once (section 2.3).

## 7. Bumping the pinned Rust version

The pin is local to five CI invocations and frozen nowhere else:

- three `toolchain:` inputs in `.github/workflows/pr-regression-gates.yml`, for `rust-regression`,
  `rust-regression-linux` and `rust-regression-macos`;
- two in `.github/workflows/cache-warm.yml`, for `warm-debug` and `verify-debug-cache`.

No `rust-toolchain.toml` exists: local development, `release.yml`, `bundle-validation.yml` and the
other workflows stay on floating stable, and the pin must not be widened to them.

To bump:

1. Change the version in the five places above, and in the three run-time assertions in
   `pr-regression-gates.yml` that compare `rustc -vV` with the pin - otherwise each leg fails with
   `toolchain pin did not take effect`.
2. Regenerate the baseline from the three platforms' emissions, merged by the script's `--merge`
   (it refuses unless all three agree on commit and rustc version). A light-tier run produces only
   the Windows emission, so label the pull request `ci:full` to get all three. The toolchain-refresh
   exemption of section 3 accepts the resulting additions.
3. Touch no `.rs` file in that pull request. The exemption requires a diff with no `.rs` path;
   that is what keeps a toolchain bump from smuggling source changes.
4. When the new toolchain's lints demand `.rs` changes, split them into their own, earlier pull
   request. The pin and the regenerated baseline follow in the second one.
