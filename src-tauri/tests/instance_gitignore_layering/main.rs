//! #1273 layering guard: `crate::config::instance_gitignore` may not name
//! `crate::config::root_agent`, nor anything else that can reach the crate's
//! cyclic SCC, and the module that now holds `ROOT_AGENT_DIR_NAME`,
//! `crate::config`, may not name anything at all. #1446 added a third scanned
//! unit on the same argument: `crate::config::instance_artifacts`, the artifact
//! registry the guarded module now derives its rules from, may not name anything
//! either, because a module with zero outgoing arcs is the only kind that is
//! safe for knot members to depend on.
//!
//! WHAT THIS GUARD IS, AND WHAT IT IS NOT.
//!
//! It is a net over the *spellings* a dependency can be written in, scanned out
//! of Rust source as text. It is not a proof that the dependency cannot return,
//! and it must not be read as one: it matches text, it does not resolve names,
//! so a spelling it does not know about passes it. The authoritative check is
//! the cycle detector run over the module graph, whose
//! `coverage.graphShape.cyclicSccs` must stay at 1 with
//! `sccSize(agentscommander_lib::config::instance_gitignore) = 1` and
//! `sccSize(agentscommander_lib::config::instance_artifacts) = 1`. A green
//! result here means "no known spelling is present", never "the cycle is
//! impossible".
//!
//! WHY BOTH ANCHORS ARE ASSERTED, FROM DAY ONE. Every reference this module
//! makes is written `super::` or `super::super::`; it contains no `crate::` at
//! all. A guard that asserted only the `crate::`-anchored set would therefore
//! observe **nothing** in this module and pass everything, which is exactly the
//! failure `project_settings_layering.rs` records against itself as entry 13 of
//! its own uncovered list and issue #1268 tracks: adding
//! `use crate::session::manager::SessionManager;` to that guarded module moved
//! the knot 88 to 89 and `sccSize` 1 to 89 with that file green throughout,
//! because its `crate::`-anchored set was collected and never asserted. Here
//! the two sets are asserted by equality, both of them, and the `crate::` table
//! is deliberately **empty**: this module names nothing under `crate::` today
//! and any first entry is a decision about the crate's shape.
//!
//! WHY THE CONSTANT'S NEW HOME IS GUARDED TOO. #1273 took
//! `config::instance_gitignore` out of the knot by moving one `pub const` down
//! into `crate::config`, and the argument that the knot cannot absorb it rests
//! on `crate::config` having **zero outgoing arcs**, measured over the record
//! in `src-tauri/module-arcs.txt`. **That premise fails on an outgoing arc, not an
//! incoming one.** One `use crate::<any knot member>::...;` in `src/config/mod.rs`
//! puts `config` into the knot and drags this module straight back in with it,
//! and the assertions about `instance_gitignore` stay green throughout, because
//! that file never changed. The premise is load bearing either way: the arc
//! `config::instance_gitignore -> config` is not removed by #1273 and is not
//! removable, so `config` has to stay clean whatever else happens.
//!
//! WHAT IT READS. Not a directory. The files it scans are resolved by walking
//! `mod` declarations down from `src/lib.rs`, honouring `path = "..."` in both
//! `#[path]` and `#[cfg_attr(..., path = ...)]`, and collecting **every**
//! declaration of a segment rather than the first, so a module declared twice
//! under opposite `cfg`s contributes both files. A directory walk decides from
//! names; the compiler decides from the module tree, and code lives in the gap.
//!
//! **This resolver is not rustc and does not claim to be.** It over-reads on
//! purpose: `cfg` is not evaluated, so both arms of a platform module are
//! scanned even though only one is compiled. Reading a file rustc does not
//! compile costs a false red, which is argued about; missing one costs a false
//! green, which is believed. Where it cannot over-read safely it refuses: two
//! candidate files for one declaration, or a `mod x;` nested inside an inline
//! `mod y { ... }` block, are hard failures naming the file rather than a guess.
//!
//! The guarded module is read together with every module below it. The constant's
//! home is read **shallow**, its own `src/config/mod.rs` and no descendant,
//! because every child of `config` is a separate module in the graph with its own
//! arcs and most of them are inside the knot; descending would assert that the
//! whole `config` subtree names nothing, which is neither true nor this issue's
//! business.
//!
//! Comments and the bodies of string and character literals are removed before
//! anything is matched: neither can be a dependency, neither may hide a path
//! from the scan, and neither may feed one to it. That is why the string
//! `"ac-root-agent"` and the many `.gitignore` fixtures containing it, in this
//! module's own tests, do not trip the forbidden-name check below.
//!
//! Widening the net is the only thing a text scan can do, so this file is
//! written to be widened: the six `ALLOWED_*` tables are the whole contract,
//! and the spellings the scan is known to miss are listed below instead of being
//! left unsaid. It was widened once already, by the #1273 review, which is what
//! the CLOSED notes on entries 14 to 17 record.
//!
//! KNOWN UNCOVERED SPELLINGS.
//!
//! This list is maintained by the review loop. When a reviewer proves a spelling
//! that puts this module back within reach of the knot and still passes this
//! file, it is appended here. Appending an entry is part of reviewing #1273 and
//! is expected; it changes nothing else.
//!
//! **This file is the canonical copy.** Section 5.4 of
//! `.ac/plans/1273-extract-instance-gitignore-from-scc.md` quotes it verbatim, but
//! that quote is a snapshot taken when the plan was certified. The first appended
//! entry makes the two diverge, and that is expected: this file runs, the plan
//! does not. Append here and leave the plan alone.
//!
//! **"The detector still catches it" is not a closure.** Several entries below
//! say so and it is true and measured, but the whole reason this file exists is
//! that the detector is run by hand and is deliberately not wired to CI. An
//! entry the detector catches is still uncovered *here*, and still reaches
//! nobody until somebody remembers to run the instrument.
//!
//!   1. Re-export laundering. A third module writes
//!      `pub use crate::config::root_agent::SOMETHING;` and this module imports
//!      from there. No `root_agent` token appears in the scanned file. The
//!      detector still catches it: the laundering module carries the arc, this
//!      module reaches the knot through it, and the knot grows instead of
//!      thinning.
//!   2. Macro-generated paths. A `macro_rules!` defined elsewhere, or any
//!      procedural macro, whose expansion contains the path. The text is not in
//!      the scanned files. Whether the detector resolves it has not been
//!      measured here, so do not assume it does.
//!   3. `include!`. A file textually included from outside the module tree is
//!      pulled in without a `mod` declaration, so walking the tree does not
//!      reach it.
//!   4. Runtime indirection. A trait object, function pointer or callback whose
//!      only implementor lives in a knot member and which is wired together
//!      outside this module. No path text appears in the scanned files.
//!   5. `concat!` and friends. `concat!("root", "_agent")` builds the name out
//!      of fragments none of which contains it, and the bodies of those literals
//!      are removed before the scan in any case.
//!   6. A `mod x;` declaration nested inside an inline `mod y { ... }` block.
//!      rustc resolves it against the inline module's own directory and this
//!      resolver does not, so it would scan a file rustc does not compile. It is
//!      refused rather than read: `module_body` rejects the whole file with a
//!      hard failure naming it. The spelling is still uncovered in the sense
//!      that the reference is not read, but it cannot be read as green.
//!   7. NTFS alternate data streams. `#[path = "carrier.rs:evil"]` compiles from
//!      a stream that carries code, and a `mod` declaration hidden inside a
//!      stream of another file is not reachable. Git stores only the main
//!      stream, so a clone has no `:evil` and the build fails rather than hiding
//!      anything.
//!   8. **The fully unanchored path, and it is the important one.** The detector
//!      shares this blind spot and it is measured on production code:
//!      `src/lib.rs:1178` constructs `loops::scheduler::LoopScheduler::new()` and
//!      **no `lib -> loops` arc exists** in the record. A path that begins with
//!      neither `crate::` nor `super::` is invisible to the record AND to both
//!      equalities here. Inside this module such a path needs a name in scope,
//!      which needs an import this guard does see, with one exception: the single
//!      `use super::*;` in the test module, entry 9. `names_the_replaced_module`
//!      closes the one spelling that matters today, a bare `root_agent`, and
//!      closes it under every anchor and none. It does not close the class.
//!   9. A second `use super::*;`, or the existing one moving to the top of the
//!      file. Written at module level rather than inside `mod tests`, that glob
//!      pulls `crate::config`'s children into scope under no name this scan can
//!      follow, `root_agent` among them, and a bare `root_agent::...` would then
//!      compile with no `super::` token anywhere. Because the observed set is
//!      deduplicated, a second identical glob would not move it. The count is
//!      therefore asserted separately, at exactly one. **Moving** the one glob is
//!      not caught by the count, and it is not exploitable as it stands, because
//!      `mod tests` reaches its parent's items through that glob and would stop
//!      compiling without it. It is written down because that is an argument
//!      about today's code, not a property of the matcher.
//!  10. Aliasing beyond the spellings `aliases_a_module_group` knows.
//!      `use crate as c;`, `use crate::config as c;` and `use super as s;` are
//!      refused by name; a rename reached some other way is not.
//!  11. A path assembled across a `cfg` boundary in a way the resolver
//!      over-reads into but the equality tables do not distinguish. This
//!      resolver scans both arms of a platform module, so a forbidden reference
//!      in either arm is caught, but which arm rustc compiled is not known here
//!      and the failure message cannot say.
//!  12. A `#[cfg(test)]` reference holding an equality up on its own, and here
//!      it is a live weakness rather than a theoretical one. Whole files are
//!      read, test regions included, while the detector is run with
//!      `includeTests: false`. Everywhere else that makes this guard stricter,
//!      which is the safe direction; here it makes it laxer. Six of the eight
//!      references to `ROOT_AGENT_DIR_NAME` in this module are inside
//!      `#[cfg(test)] mod tests`, so deleting the two production references at
//!      `required_rules` would leave the pair
//!      `("src/config/instance_gitignore.rs", "ROOT_AGENT_DIR_NAME")` standing
//!      and the equality green. Unlike the equivalent entry in
//!      `project_settings_layering.rs`, **that deletion can be made to compile**:
//!      hard-coding the directory name in `required_rules` does it. The
//!      shrinking-set argument is correspondingly weaker for this one pair.
//!
//!      **This entry used to claim the module's fourteen behavioural tests hold
//!      the production references up. They do not, and the claim is struck.**
//!      Measured during the #1273 review: replacing both production references
//!      with the literal `"ac-root-agent"` leaves this guard at 3 passed AND
//!      `cargo test --lib config::instance_gitignore` at 15 passed, because
//!      those tests pin the emitted `.gitignore` bytes, not the path the name is
//!      reached by. Nothing in this repository currently holds those two
//!      references up. See entry 19, which is that measurement in full. A
//!      documented blind spot is usable; a false mitigation is what misleads.
//!  13. An unanchored path in the constant's home. `src/config/mod.rs` already
//!      writes `profile::config_dir_name()` three times, a path beginning with
//!      neither `crate::` nor `super::`, which is why it creates no arc and why
//!      that module measures zero outgoing arcs. A new unanchored path from
//!      there into a knot member would be invisible to the detector and to this
//!      guard alike, while putting `config` and this module back into the knot
//!      the moment anybody anchored it. The two equalities on that file catch
//!      the anchored forms only.
//!  14. **`use super::super::*;` at the top level of the guarded module's own
//!      file, and it is the cheapest total defeat found so far.** `GLOB_IMPORT`
//!      is matched as the literal text `use super::*;`, which
//!      `use super::super::*;` does not contain, so the count stays at 1. Both
//!      children it reports, `super` and `*`, are already allowed rows. Measured
//!      on this tree, one line added above `required_rules` in
//!      `src/config/instance_gitignore.rs`: guard **3 passed**,
//!      `cargo fmt --check` **exit 0**, and the record gains
//!      `config::instance_gitignore -> agentscommander_lib`, taking the knot
//!      87 -> 88, `sccSize(config::instance_gitignore)` 1 -> 88 and its level
//!      1 -> 3. From `crate::config::instance_gitignore` the path means
//!      `use crate::*;`, so it also puts every crate-root child in scope for
//!      unanchored use. Entry 9 anticipated a *moved* glob; this is a *second*
//!      glob the count cannot see, and unlike a moved one it costs the test
//!      module nothing.
//!
//!      **CLOSED.** The glob count is no longer textual: it counts
//!      `super::`-anchored items whose leading segment is `GLOB_CHILD`, taken
//!      before the observed set is deduplicated, so this spelling now counts 2
//!      and the assertion fires. Proven red on this tree before the closure was
//!      accepted.
//!  15. `use super::{*};` in the guarded module. Same evasion of the glob count
//!      by the same mechanism, and the two children it reports are the allowed
//!      `*`. Measured: guard **3 passed**. It is the weaker sibling of entry 14
//!      on two counts: `cargo fmt --check` goes red on it, because rustfmt
//!      rewrites it to `use super::*;` (which would then make the count 2), and
//!      it reaches only `crate::config`'s children, so a production
//!      `injected_messages::INJECTED_MESSAGES_FILENAME` written through it is
//!      invisible to this guard **and** to the detector: measured, zero arc
//!      change, knot unmoved at 87. That is entry 8's class made writable, and
//!      it is the cosmetic satisfaction the plan's Section 9.3.2 warns about,
//!      reached without touching a single anchored path.
//!
//!      **CLOSED by the same change as entry 14.** `children_under` normalizes
//!      the path and walks the brace group, so it reports `*` for this spelling
//!      exactly as it does for the bare one, and the structural count sees 2.
//!      Proven red on this tree.
//!  16. **`use super::*;` at the top level of the constant's home, and it is the
//!      most damaging spelling found.** `the_constant_home_names_nothing_at_all`
//!      asserts no glob count at all -- unlike the guarded module's test -- and
//!      `("src/config/mod.rs", "*")` is an allowed row, put there for the glob
//!      inside that file's own `#[cfg(test)] mod tests`. A text scan cannot tell
//!      the two positions apart, which is exactly the argument entry 9 makes for
//!      the guarded module, and the host test never draws the conclusion.
//!      Measured, one line added to `src/config/mod.rs`: guard **3 passed**,
//!      `cargo fmt --check` **exit 0**, and the record gains
//!      `config -> agentscommander_lib`. `config` stops being the pure sink
//!      Section 4.3 of the plan rests on, the knot goes 87 -> **91**, and
//!      `sccSize(config::instance_gitignore)` goes 1 -> **91** through arc 547,
//!      which #1273 never removed and cannot remove. This is precisely the
//!      failure this test was written to prevent, walking through the one row it
//!      allows.
//!
//!      **CLOSED.** `the_constant_home_names_nothing_at_all` now asserts the
//!      same `globs == 1` count the guarded module's test has always asserted,
//!      so the asymmetry that made this spelling free is gone. Proven red on
//!      this tree.
//!  17. **`self::` is scanned under no anchor anywhere in this file.**
//!      `children_under` is called with `crate::` and `super::` only, and
//!      `the_constant_home_names_nothing_at_all` does not assert
//!      `seen.forbidden`, so the anchorless `root_agent` check never runs
//!      against the constant's home. Measured, one line added to
//!      `src/config/mod.rs`:
//!      `use self::root_agent::ROOT_AGENT_SESSION_NAME;` -- guard **3 passed**,
//!      `cargo fmt --check` **exit 0**, record gains
//!      `config -> config::root_agent`, knot 87 -> **91**,
//!      `sccSize(config::instance_gitignore)` 1 -> **91**. Unlike entry 13 the
//!      detector is **not** blind to this one: `self::` resolves and the arc is
//!      recorded. The same spelling reaches any knot member, not only
//!      `root_agent`, so the anchorless check would not close the class even if
//!      the host test asserted it.
//!
//!      **CLOSED, and closed as a class rather than a name.** `SELF_ANCHOR` is
//!      now a third observed set, asserted against an empty table in BOTH tests,
//!      so any `self::`-anchored reference fails regardless of what it names.
//!      `the_constant_home_names_nothing_at_all` also asserts `seen.forbidden`
//!      now, which was computed and thrown away, giving the anchorless
//!      `root_agent` check a second line of defence over that file. Proven red
//!      on this tree.
//!  18. **Promoting an allowed `#[cfg(test)]`-only pair to production.** The
//!      `ALLOWED_GUARDED_SUPER_REFERENCES` row
//!      `("src/config/instance_gitignore.rs", "injected_messages")` is allowed
//!      only because its two references sit inside `#[cfg(test)] mod tests` and
//!      therefore contribute no arc. The pair is `(file, child)` and carries no
//!      position, so moving the same reference into production code leaves the
//!      observed set identical. Measured, one production function added to
//!      `src/config/instance_gitignore.rs` naming
//!      `super::injected_messages::INJECTED_MESSAGES_FILENAME`: guard
//!      **3 passed**, record gains
//!      `config::instance_gitignore -> config::injected_messages`, knot
//!      87 -> 88, `sccSize` 1 -> **88**, level 1 -> 3. The table's own doc
//!      comment states the risk; nothing in the file acts on it. Every allowed
//!      row that names a knot member is one of these.
//!
//!      **OPEN, AND DELIBERATELY NOT CLOSED HERE.** This one needs a decision,
//!      not a matcher change: either the two `injected_messages` imports move
//!      out of the target's test module, or the allowed row is dropped and the
//!      pair forbidden. Both are design decisions the frozen plan does not
//!      cover, so neither was taken during the #1273 review. It is reported
//!      upward as a follow-up. Until then, treat every allowed row that names a
//!      knot member as one line of production code away from a regression.
//!  19. **Entry 12 is live and its stated mitigation does not hold.** That entry
//!      says "the fourteen-rule behavioural tests in the module are what
//!      actually hold the production references up". Measured: they do not.
//!      Replacing both production references at `required_rules` with the
//!      literal `"ac-root-agent"` leaves the guard **3 passed** (the pair
//!      survives on the six `#[cfg(test)]` references) **and**
//!      `cargo test --lib config::instance_gitignore` at **15 passed**, because
//!      those tests pin the emitted `.gitignore` bytes, not the path the name is
//!      reached by. `the_root_agent_dir_name_constant_is_defined_exactly_once`
//!      does not catch it either: it counts `const`/`static` definitions, and a
//!      bare string literal is neither. So the "moved, not duplicated" property
//!      of criterion 8 is defeasible by duplicating the *value* rather than the
//!      definition.
//!
//!      **OPEN, AND THIS GUARD CANNOT CLOSE IT.** A hard-coded `"ac-root-agent"`
//!      is a value, not a path, and this file matches paths. Closing it would
//!      need a different assertion, that the literal appears exactly once under
//!      `src/`, which is a separate decision and was not taken here. What the
//!      #1273 review did do is strike the false mitigation from entry 12: the
//!      behavioural tests do NOT hold those references up. The blind spot stands
//!      as the honest record.
//!  20. **The `mod root_agent;` exemption does not check what precedes `mod `,
//!      and that is a hole rather than a blind spot.** `names_the_replaced_module`
//!      exempts an occurrence when `body[..at].ends_with("mod ")`, with no
//!      disqualification on the character before it. Every other `mod`-matching
//!      function in this file -- `find_declarations`, `declared_children` and
//!      `nested_module_declaration` -- refuses a hit whose preceding character is
//!      alphanumeric, `_` or `"`, precisely so that `submod x;` is not read as
//!      `mod x;`. The exemption omits that check, so any text ending in `mod `
//!      exempts the name.
//!
//!      Measured on this tree. A `macro_rules!` defined outside the guarded
//!      module, matching `(xmod $m:ident;)` and expanding to
//!      `crate::config::$m::ROOT_AGENT_SESSION_NAME`, invoked from
//!      `src/config/instance_gitignore.rs` as `probe!(xmod root_agent;);`:
//!      **guard 3 passed**. The token `root_agent` is present in the guarded
//!      module's own source and the guard deliberately skips it, while the
//!      expansion is a production reference from this module into
//!      `config::root_agent`. The arc record does not move (macro invocations are
//!      skipped by the detector), so nothing in the repository notices.
//!
//!      Note the near miss that makes this specific: written as
//!      `probe!(mod root_agent;);`, the same attack is caught -- but by
//!      `declared_children`, which reads the macro argument as a real `mod`
//!      declaration and aborts because no such file exists. Spelling the
//!      preceding token `xmod` disqualifies that hit and not the exemption's.
//!      **The fix is one clause: apply the same preceding-character
//!      disqualification the other three functions already apply.** Until then
//!      the exemption is broader than its own doc comment claims, which says
//!      only the declaration is exempt.
//!
//!      **CLOSED.** `names_the_replaced_module` now requires `mod ` to be a
//!      whole keyword: the character in front of it must not be alphanumeric,
//!      `_` or `"`, which is the same test the other three functions make.
//!      `pub mod root_agent;`, `pub(crate) mod root_agent;` and
//!      `#[cfg(unix)] mod root_agent;` keep the exemption; `xmod root_agent;`
//!      loses it. Strictly narrower than the first exemption and strictly
//!      broader than none, which is what the doc comment always claimed and now
//!      matches. Proven red on this tree with the macro probe above, and the
//!      real declaration and the anchored spellings re-confirmed unaffected.
//!  21. **`extern crate self as <name>;` inside the guarded module.** It
//!      compiles, it is rustfmt-stable, and it renames the crate root under a
//!      name no anchor in this file reads.
//!      `aliases_a_module_group` refuses `use crate as `, `::config as `,
//!      `{config as `, `,config as `, `config::{self as `, `use super as ` and
//!      `use super::{self as ` -- this spelling is none of them, and it is the
//!      most powerful of the family because it aliases the root rather than one
//!      module group. Measured, one line plus one production function in
//!      `src/config/instance_gitignore.rs`:
//!      `extern crate self as c;` then `c::config::injected_messages::...`, guard
//!      **3 passed**, `cargo fmt --check` **exit 0**, arc record unchanged
//!      (the detector does not resolve it either). Entry 10 declares the class;
//!      this is the compiling, one-line member of it, and closing it costs one
//!      more string in `aliases_a_module_group`.
//!
//!      For the record, two neighbours were tried and are caught:
//!      `use super::super::config as c;` trips the `::config as ` rename, and
//!      `use super::super::<child> as c;` reports the child pair and fails the
//!      `super::` equality. `use super::super as c;` does not compile.
//!
//!      **CLOSED.** `"extern crate self as "` is now the eighth spelling
//!      `aliases_a_module_group` refuses, so the rename is refused rather than
//!      followed, exactly as the other seven are. Proven red on this tree with
//!      the alias plus a production reference through it. Entry 10 still stands
//!      for the class: a rename reached some other way is still not covered.
//!  22. **Moving the constant home's glob rather than multiplying it.** Entry 9
//!      records this class for the guarded module; it is live on the host too,
//!      and the structural count added for entries 14 to 16 does not see it,
//!      because moving keeps the total at exactly 1. Measured: deleting
//!      `use super::*;` from the `#[cfg(test)] mod tests` of `src/config/mod.rs`
//!      and writing the same three words at the top level of that file leaves
//!      the guard at **3 passed** with `("src/config/mod.rs", "*")` still the
//!      one allowed row, while the record gains
//!      `config -> agentscommander_lib` and the knot goes 87 to **91**.
//!
//!      **It is not viable as it stands, and the reason is the compiler and not
//!      this guard.** `cargo check --all-targets` fails: that test module reaches
//!      its parent's items through the glob. Every repair turns the guard red --
//!      naming the items adds `super::` pairs, `use crate::config::*;` or
//!      `crate::config::...` adds a `crate::` pair against an empty table, and a
//!      second glob restores the count to 2. So the class is closed in practice
//!      by two mechanisms neither of which is the count. Recorded because the
//!      count is what the failure message credits, and a future test module that
//!      needs less of its parent would remove the compiler's half.
//!  23. **Entry 13 is now narrower than it reads, and the remainder is still
//!      open.** Since `the_constant_home_names_nothing_at_all` began asserting
//!      `seen.forbidden`, an unanchored path from `src/config/mod.rs` into
//!      `config::root_agent` specifically is caught: measured,
//!      `root_agent::ROOT_AGENT_SESSION_NAME` in a function there is now red,
//!      where it was green before the closure. Entry 13's sentence "invisible to
//!      the detector and to this guard alike" is therefore no longer true for
//!      that one name. It remains true for every other knot member: measured,
//!      `injected_messages::INJECTED_MESSAGES_FILENAME` in a function in
//!      `src/config/mod.rs` is **3 passed**, `cargo fmt --check` exit 0, and the
//!      arc record does not move. The anchorless check is a name, not a class,
//!      and only the anchored equalities generalise.
//!  24. **The `mod` exemption still exempts a macro argument, and in the
//!      constant's home nothing else catches it.** Entry 20's fix made
//!      `keyword_is_whole` use exactly the rule `find_declarations`,
//!      `declared_children` and `nested_module_declaration` use: the character
//!      in front of `mod ` must not be alphanumeric, `_` or `"`. That is the
//!      right rule and it closed `xmod`. It does not close the rest, because
//!      **punctuation is not disqualified**, and a macro invocation puts
//!      punctuation in front of the keyword.
//!
//!      Measured by extracting `scrub`, `normalized` and
//!      `names_the_replaced_module` verbatim from this file and running them
//!      over a table. Nine prefixes keep the exemption while carrying a
//!      `root_agent` token that is not a declaration, all of them inside a
//!      macro invocation: `probe!(mod root_agent;)`, `probe![mod root_agent;]`,
//!      `probe!{mod root_agent;}`, `probe!(a,mod root_agent;)`,
//!      `probe!(!mod root_agent;)`, `probe!(>mod root_agent;)`,
//!      `probe!(.mod root_agent;)`, `probe!(::mod root_agent;)`, and
//!      `probe!("x"mod root_agent;)`, where the literal is scrubbed to a space
//!      before the check ever sees it.
//!
//!      **In the guarded module this is harmless, and the reason is worth
//!      knowing.** `declared_children` reads exactly the same text as a real
//!      `mod root_agent;` declaration, `resolve_children` then finds no
//!      `src/config/instance_gitignore/root_agent.rs`, and the resolver aborts
//!      with a hard failure. Measured: `probe!(mod root_agent;)` in the target is
//!      RED for that reason, not for the forbidden-name reason.
//!
//!      **In the constant's home it is live.** `the_constant_home_names_nothing_at_all`
//!      observes with `Reach::OwnFilesOnly`, so `declared_children` never runs
//!      over `src/config/mod.rs` at all and only the exemption stands between
//!      that file and the check. Measured on the merged tree, with a
//!      `macro_rules!` defined in `src/lib.rs` matching `(mod $m:ident;)` and
//!      expanding to `crate::config::$m::ROOT_AGENT_SESSION_NAME`, invoked from
//!      `src/config/mod.rs` as `probe!(mod root_agent;);`: guard **3 passed**,
//!      and still 3 passed after `cargo fmt` reflows it across three lines, with
//!      `cargo fmt --check` back at exit 0. The arc record does not move, because
//!      the detector skips macro invocations, so nothing in the repository
//!      notices.
//!
//!      **This is a residual blind spot, not a regression.** The same probe is
//!      green against the guard at `c3f6e2c`, the commit that first asserted
//!      `seen.forbidden` on this file, so no version of this guard ever caught
//!      it. Entry 20's clause removed a regression; it did not claim to close
//!      the class, and the class needs a different rule.
//!
//!      **The closure is cheap and is a further narrowing, not a widening.**
//!      After `normalized`, an item-position `mod` can only be preceded by
//!      nothing, a space, or `}` -- `pub`, `pub(crate)` and `#[cfg(unix)]` all
//!      leave a space, and a preceding item's `}` is closed up by the
//!      normalizer. Requiring one of those three drops every row above while
//!      keeping every real declaration. All nine legitimate forms were measured
//!      green through the current predicate and would stay green through the
//!      narrower one.
//!  25. **`keyword_is_whole` panics on a multi-byte character, and the `'"'` in
//!      its disqualification set is unreachable.** Two defects in the same
//!      three lines; neither is a false green, and both are one-line fixes.
//!
//!      `at.checked_sub("mod ".len())` yields a **byte** index and
//!      `body[..keyword_at]` is sliced at it unconditionally, before the
//!      `ends_with("mod ")` guard is consulted, because the `let` binding is
//!      evaluated eagerly. If a multi-byte character ends within four bytes of
//!      any occurrence of `root_agent`, that slice is not on a character
//!      boundary and the function panics. Measured on the extracted predicate:
//!      `let \u{00F1}abcroot_agent = 1;` panics with "byte index 5 is not a char
//!      boundary; it is inside 'n-tilde'". Rust identifiers may be non-ASCII, so
//!      this is reachable from ordinary code rather than only from a hostile
//!      file. It fails loud, which is the safe direction, but it replaces the
//!      #1273 message with a slicing panic that names nothing useful.
//!      **Fix: make it lazy.** Move the computation behind
//!      `body[..at].ends_with("mod ")` in the `&&` chain -- when that holds,
//!      `at - 4` is guaranteed to be a boundary, because `"mod "` is four ASCII
//!      bytes ending at `at`.
//!
//!      The `character == '"'` arm can never fire here. `names_the_replaced_module`
//!      is only ever called on a body scrubbed with `Literals::Drop`, where every
//!      string literal has already become a single space, so no `"` survives to
//!      be examined. It is inherited from `find_declarations`, which *is* called
//!      on a `with_literals` body and needs it. Harmless -- it can only make the
//!      disqualification stricter, never looser -- but it does not do the job it
//!      appears to do, and the row `probe!("x"mod root_agent;)` above is the
//!      demonstration: the quote is gone before the check sees it, so the prefix
//!      that reaches the predicate is a space.
//!  26. (append here: one entry per spelling a reviewer proves still passes)

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Resolving what the compiler compiles
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// The guards
// ---------------------------------------------------------------------------

mod allowed;
mod knot;
mod registry;
mod resolve;
mod scan;

use allowed::{
    Literals, Reach, ALLOWED_GUARDED_CRATE_REFERENCES, ALLOWED_GUARDED_SELF_REFERENCES,
    ALLOWED_GUARDED_SUPER_REFERENCES, ALLOWED_HOST_CRATE_REFERENCES, ALLOWED_HOST_SELF_REFERENCES,
    ALLOWED_HOST_SUPER_REFERENCES, ALLOWED_REGISTRY_CRATE_REFERENCES,
    ALLOWED_REGISTRY_SELF_REFERENCES, ALLOWED_REGISTRY_SUPER_REFERENCES, CONSTANT_HOME,
    CONSTANT_NAME, CRATE_ANCHOR, FORBIDDEN_NAME, GLOB_CHILD, GUARDED_MODULE, HOST_MODULE,
    REGISTRY_MODULE, SELF_ANCHOR, SUPER_ANCHOR,
};
use resolve::{
    crate_sources, every_file_under, expected, files_of, manifest_dir, observe, relative_of,
    scrubbed,
};
use scan::{
    aliases_a_module_group, children_under, defines_the_constant, names_the_replaced_module,
    normalized, scrub,
};
