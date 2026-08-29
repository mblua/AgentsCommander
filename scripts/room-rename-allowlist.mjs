#!/usr/bin/env node
// #1614 plan section 9.4 AC1: the visible-text gate, and the script that
// derives and classifies the allowlist beside it.
//
//   node scripts/room-rename-allowlist.mjs sweep [--rev <rev>]
//   node scripts/room-rename-allowlist.mjs moved --rev d7008b34
//   node scripts/room-rename-allowlist.mjs derive --rev d7008b34 --write
//   node scripts/room-rename-allowlist.mjs gate
//
// Part A of scripts/room-rename-allowlist.tsv is derived at the frozen base
// d7008b34 by taking the three binding sweeps and SUBTRACTING the lines this
// plan moves. It is committed before the first visible-text edit, so an
// unrenamed Rule R line is not in it and comes back from the gate unlisted.
// Part B is appended at step 10b for lines the change itself introduces.
//
// The allowlist key is (path, trimmed line content). Content, not line number,
// so the file survives line drift. Trimming also absorbs the line-ending split:
// .gitattributes pins *.rs to LF while *.ts, *.tsx and *.md are CRLF in the
// working tree, so a rev sweep and a working-tree sweep agree after trimming.

import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO = join(HERE, "..");
const TSV = join(HERE, "room-rename-allowlist.tsv");

// The binding alternation, identical in all three sweeps (AC1 point 2).
const RE =
  "[Ww]orkgroup|WORKGROUP|(^|[^A-Za-z0-9_])([Ww][Gg])([^A-Za-z0-9_]|$)|(^|[^A-Za-z0-9_])wg-";

const SURFACES = {
  frontend: {
    paths: ["src"],
    keep: (p) => /^src\/[^:]*\.tsx?$/.test(p) && !/^src\/[^:]*\.test\.tsx?$/.test(p),
  },
  rust: { paths: ["src-tauri/src"], keep: () => true },
  docs: {
    paths: [
      "docs",
      "README.md",
      "ROADMAP.md",
      "PRIVACY.md",
      "src-tauri/src/api/README.md",
      ":!docs/assets",
    ],
    keep: () => true,
  },
};

function git(args) {
  try {
    return execFileSync("git", ["-C", REPO, ...args], { encoding: "utf8", maxBuffer: 1 << 29 });
  } catch (e) {
    if (e.status === 1 && typeof e.stdout === "string") return e.stdout; // no matches
    throw e;
  }
}

export function sweep(surface, rev) {
  const s = SURFACES[surface];
  const args = ["grep", "-nE", RE];
  if (rev) args.push(rev);
  args.push("--", ...s.paths);
  const rows = [];
  for (const line of git(args).split("\n")) {
    if (!line) continue;
    const bin = line.match(/^Binary file (?:[0-9a-f]{7,40}:)?(.*) matches$/);
    if (bin) {
      if (s.keep(bin[1])) rows.push({ surface, path: bin[1], lineno: 0, content: "<binary file>" });
      continue;
    }
    const body = rev && line.startsWith(`${rev}:`) ? line.slice(rev.length + 1) : line;
    const i1 = body.indexOf(":");
    const i2 = body.indexOf(":", i1 + 1);
    if (i1 < 0 || i2 < 0) continue;
    const path = body.slice(0, i1);
    if (!s.keep(path)) continue;
    rows.push({ surface, path, lineno: Number(body.slice(i1 + 1, i2)), content: body.slice(i2 + 1).trim() });
  }
  return rows;
}

export const key = (r) => `${r.path}\t${r.content}`;

// ===========================================================================
// The MOVE specification. Every entry cites the plan section that requires it.
// ===========================================================================

// Plan 3.2: the 40 dual-prefix gate lines, each replaced by a call to
// crate::config::entity_prefix::*, so the "wg-" literal leaves the line.
const GATES = {
  "src-tauri/src/cli/list_peers.rs": [707, 907, 912, 1009],
  "src-tauri/src/cli/role_experiment.rs": [2298, 2328, 3185, 3204, 3224],
  "src-tauri/src/commands/ac_discovery.rs": [1182, 1957],
  "src-tauri/src/commands/config.rs": [1527, 1561],
  "src-tauri/src/commands/entity_creation.rs": [1085, 2964, 3488, 4301],
  "src-tauri/src/commands/task.rs": [104],
  "src-tauri/src/config/ac_root.rs": [145],
  "src-tauri/src/config/coding_agent_profiles.rs": [235],
  "src-tauri/src/config/loops.rs": [444],
  "src-tauri/src/config/placeholders.rs": [273],
  "src-tauri/src/config/replica_identity.rs": [238],
  "src-tauri/src/config/teams.rs": [89, 121, 212, 449, 649, 1137, 1465, 1469, 1661],
  "src-tauri/src/phone/mailbox.rs": [2157, 11206],
  "src-tauri/src/phone/messaging.rs": [373, 386],
  "src-tauri/src/pty/container_paths.rs": [289],
  "src-tauri/src/pty/container_repos.rs": [136],
  "src-tauri/src/screenshot/windows.rs": [1405],
  "src-tauri/src/session/session.rs": [249],
};

// Plan 5.2 P3 / 3.7: byte ranges that must not move. A line here is never a
// MOVE, whatever else it looks like.
const FROZEN = {
  "src-tauri/src/config/session_context.rs": [
    [3735, 3736], // LEGACY_GIT_SCOPE_*_BEFORE_1072, frozen by #1072
    [3769, 4025], // legacy_rendered_default_context_for_generation, whole body
  ],
  "src-tauri/src/config/root_agent.rs": [
    [290, 290], // OLD_DEFERRED_MESSAGING_PARAGRAPH (D8d), frozen in place
    [308, 338], // OLD_ROOT_ROLE_MD
    [340, 374], // OLD_ROOT_CONTEXT_WITH_COORDINATION_MD
    [376, 674], // the remaining frozen *_BEFORE_* root generations
  ],
  // every *_BEFORE_* seeded-template constant
  "src-tauri/src/config/seeded_context_templates.rs": [[80, 331]],
  "src-tauri/src/config/injected_messages.rs": [
    [46, 46], // TOKEN_WORKGROUP, Rule P0
    [50, 52], // DEFAULT_CONTEXT_ALERT_TEMPLATE and its byte-count comment
    [85, 85], // known_default_sha256
  ],
};

// The one line inside a frozen range the plan authorizes: D8b's identifier
// switch at :3860. Its content changes, so it is a MOVE.
const FROZEN_EXCEPTIONS = { "src-tauri/src/config/session_context.rs": [3860] };

// Plan 5.8: the doc comments clap compiles into printed help, plus the field
// docs 5.8 names. Rule P1's carve-out (a).
const CLAP_DOCS = {
  "src-tauri/src/cli/mod.rs": [160, 163, 165, 173, 175],
  "src-tauri/src/cli/workgroup.rs": [26, 28, 30],
  "src-tauri/src/cli/team.rs": [29, 31],
  "src-tauri/src/cli/purge_wg.rs": [94],
  "src-tauri/src/cli/close_session.rs": [135, 136, 137],
  "src-tauri/src/cli/send.rs": [53],
};

// Rule P1 clause (b), the closed set of plan 3.14: a doc comment that quotes
// `starts_with("wg-")` or the `[3..]` slice as prose, plus the
// determine_next_wg_number block plan 5.5 rewrites (:4265-4290).
const CONTRADICTING_DOCS = {
  "src-tauri/src/commands/entity_creation.rs": [3832, 4265, 4269, 4274, 4275, 4279, 4288],
};

// Plan 9.3: existing assertions authorized to move. Clause 1 pins a renamed
// string, clause 2 pins a bumped version or a resized constant, clause 3 an
// assertion on a widened predicate. These live inside #[cfg(test)] items, so
// the production narrowing never sees them, but the raw sweep does.
const TEST_EDITS = {
  "src-tauri/src/config/session_context.rs": [4793, 5282, 5318, 5490, 5956, 5992, 6104, 6105, 8434],
  "src-tauri/src/commands/session.rs": [5392],
  "src-tauri/src/cli/workgroup.rs": [545],
  "src-tauri/src/cli/list_peers.rs": [2252],
  "src-tauri/src/commands/entity_creation.rs": [5125, 6929, 6944, 6945, 7203, 7240, 7257],
  "src-tauri/src/config/root_agent.rs": [2113, 2402],
  "src-tauri/src/config/injected_messages.rs": [1331, 1671],
  "src-tauri/src/config/seeded_context_templates.rs": [2055, 2058, 2068, 3344, 3345],
};

// Lines the STR/template heuristic reaches but that Rule P keeps. Each names
// the clause. These are the hand-read classes of AC1 point 7.
const STR_EXCEPTIONS = {
  // Plan 3.10: the outbox/JSON keys and the internal identity key format.
  "src-tauri/src/phone/mailbox.rs": {
    // Plan 3.10 / D13: the outbox `action` wire value. An in-flight message
    // written by an older CLI must still be handled, so it stays "purge-wg"
    // even though every piece of PROSE naming the command becomes purge-room.
    1174: "P0-wire",
    9537: "P0-key", 9571: "P0-key", 9667: "P0-key", 9693: "P0-key", 9793: "P0-key",
  },
  "src-tauri/src/config/teams.rs": { 995: "P0-key" },
  "src-tauri/src/cli/role_experiment.rs": { 1479: "P0-key" },
  // The `wg-*/` ignore pattern itself stays; only its comment moves (5.6).
  "src-tauri/src/commands/ac_discovery.rs": { 1508: "P0-identifier" },
  // NOT this product's concept: "wg" here is the WireGuard network-interface
  // name prefix, in a list beside "veth", "virbr", "tun" and "tap".
  "src-tauri/src/commands/config.rs": { 2206: "P0-identifier" },
  // `.unwrap_or("workgroup")` fallbacks stand in for a DIRECTORY NAME, not for
  // prose: each is the `file_name()` of an entity directory.
  "src-tauri/src/cli/workgroup.rs": { 239: "P0-identifier" },
  "src-tauri/src/commands/entity_creation.rs": { 2132: "P0-identifier" },
  "src-tauri/src/loops/delivery.rs": { 362: "P0-identifier" },
};

// Plan 3.14 / 6.1: both terminal-snapshot test files are Rule P2 in whole.
// Their purpose is to represent a legacy Workgroup, so no production line moves
// and section 9.1 adds room- twins beside the existing literals.
const P2_FILES = [
  "src-tauri/src/pty/terminal_snapshot/acceptance_tests.rs",
  "src-tauri/src/pty/terminal_snapshot/resource_tests.rs",
];

// Plan 3.8 / 9.4 AC1 point 8: the 63 frontend lines this plan moves.
// 55 Rule R (3.8 classes a-d) + 3 R1 resolvers + 5 section 5.4 predicates.
const FRONTEND_MOVES = {
  "src/resource-monitor/App.tsx": [673, 677],
  "src/watchers/App.tsx": [851],
  "src/guide/components/HintsTab.tsx": [70],
  "src/shared/path-extractors.ts": [25],
  "src/shared/profile-utils.ts": [124, 472],
  "src/sidebar/components/AcDiscoveryPanel.tsx": [263],
  "src/sidebar/components/ActionBar.tsx": [15],
  "src/sidebar/components/AgentPickerModal.tsx": [436, 944, 949, 972],
  "src/sidebar/components/EditLoopModal.tsx": [228, 242],
  "src/sidebar/components/NewLoopModal.tsx": [171, 185],
  "src/sidebar/components/NewWorkgroupModal.tsx": [36, 59, 95],
  "src/sidebar/components/ProjectPanel.tsx": [
    814, 827, 993, 995, 1057, 1058, 1071, 1434, 1451, 1455, 2542, 2616, 2703, 2708, 2760, 2767,
    2795, 3117, 3361, 3773, 3777, 3835, 3899, 3949, 3961, 3998,
  ],
  "src/sidebar/components/SettingsModal.tsx": [889, 2089, 2263],
  "src/sidebar/components/TeamContextAlertsEditor.tsx": [78, 79],
  "src/sidebar/components/WorkgroupGroupRail.tsx": [67, 72, 73],
  "src/sidebar/stores/workgroup-groups.ts": [587, 597, 612, 637, 659, 675, 685],
  "src/terminal/components/TaskCleanConfirmModal.tsx": [72],
  "src/terminal/components/WorkgroupTask.tsx": [74],
};

// Plan 5.13: every docs line takes Rule R except these, whose every occurrence
// is a Rule P carrier. Hand-read, one row each (AC1 point 7).
const DOCS_KEEP = {
  "docs/features/context-tracking.md": { 75: "P0-token" },
  "docs/integrations/rtk_claude/hooks/ac_rtk_shared.js": { 50: "P1-comment" },
  "docs/integrations/rtk_pi/extensions/tool-hook.ts": { 55: "P1-comment" },
  "docs/reference/architecture.md": {
    216: "P0-identifier", 251: "P0-identifier", 346: "P0-event",
    843: "P0-identifier", 860: "P0-identifier", 870: "P0-identifier",
  },
  "docs/reference/directory-layout.md": { 51: "P0-identifier" },
  "docs/screenshots/hero.png": { 0: "P2-fixture" },
  "docs/testing/README.md": { 182: "P0-identifier" },
  "docs/testing/destructive-filesystem-regression.md": {
    148: "P0-identifier", 179: "P2-fixture", 182: "P2-fixture", 198: "P2-fixture",
    200: "P2-fixture", 207: "P0-identifier", 228: "P2-fixture", 241: "P2-fixture",
    242: "P0-identifier", 243: "P0-identifier", 246: "P0-identifier", 259: "P0-identifier",
    262: "P0-event", 281: "P2-fixture",
  },
  "docs/testing/semantic-ui-automation-affordance-matrix.md": {
    65: "P0-testid", 71: "P0-testid", 72: "P0-testid", 91: "P0-testid",
  },
};

// ===========================================================================
// Rust source-region analysis (plan 3.14 step 1, and Rule P4's log:: spans).
// ===========================================================================

const blobCache = new Map();
function blob(rev, path) {
  // With no rev, read the WORKING TREE, so `sweep` and `moved` are runnable
  // against the current checkout and not only against a committed revision.
  const k = `${rev ?? "<worktree>"}:${path}`;
  if (!blobCache.has(k)) {
    blobCache.set(k, rev ? git(["cat-file", "blob", `${rev}:${path}`]) : readFileSync(join(REPO, path), "utf8"));
  }
  return blobCache.get(k);
}

function stripLiterals(line, st) {
  let out = "";
  let i = 0;
  while (i < line.length) {
    if (st.raw !== null) {
      const close = '"' + "#".repeat(st.raw);
      const at = line.indexOf(close, i);
      if (at === -1) return out;
      i = at + close.length;
      st.raw = null;
      continue;
    }
    if (line[i] === "/" && line[i + 1] === "/") return out;
    const rm = /^r(#*)"/.exec(line.slice(i));
    if (rm) {
      st.raw = rm[1].length;
      i += rm[0].length;
      continue;
    }
    if (line[i] === '"') {
      let k = i + 1;
      while (k < line.length) {
        if (line[k] === "\\") { k += 2; continue; }
        if (line[k] === '"') break;
        k++;
      }
      i = k + 1;
      continue;
    }
    out += line[i++];
  }
  return out;
}

// Lines inside a #[cfg(test)] item; lines interior to a multi-line literal;
// lines inside a log:: invocation.
function analyze(text) {
  const lines = text.split("\n");
  const st = { raw: null };
  const code = lines.map((l) => stripLiterals(l, st));

  const inTest = new Set();
  for (let i = 0; i < lines.length; i++) {
    if (!/^#\[cfg\(test\)\]/.test(lines[i])) continue;
    let j = i + 1;
    while (j < lines.length && /^\s*(#\[|\/\/)/.test(lines[j])) j++;
    let depth = 0, started = false, k = j;
    for (; k < lines.length; k++) {
      for (const ch of code[k]) {
        if (ch === "{") { depth++; started = true; }
        else if (ch === "}") depth--;
      }
      if (started && depth <= 0) break;
      if (!started && /;\s*$/.test(code[k].trimEnd())) break;
    }
    for (let m = i; m <= Math.min(k, lines.length - 1); m++) inTest.add(m + 1);
  }

  const inString = new Set();
  let raw = null, cont = false;
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i], n = i + 1;
    if (raw !== null) {
      inString.add(n);
      if (line.includes('"' + "#".repeat(raw))) raw = null;
      continue;
    }
    if (cont) {
      inString.add(n);
      let j = 0, closed = false;
      while (j < line.length) {
        if (line[j] === "\\") { j += 2; continue; }
        if (line[j] === '"') { closed = true; break; }
        j++;
      }
      if (closed) cont = false;
      continue;
    }
    let j = 0;
    while (j < line.length) {
      if (line[j] === "/" && line[j + 1] === "/") break;
      const rm = /^r(#*)"/.exec(line.slice(j));
      if (rm) {
        const close = '"' + "#".repeat(rm[1].length);
        const at = line.indexOf(close, j + rm[0].length);
        if (at === -1) { raw = rm[1].length; break; }
        j = at + close.length;
        continue;
      }
      if (line[j] === '"') {
        let k = j + 1, closed = false;
        while (k < line.length) {
          if (line[k] === "\\") { k += 2; continue; }
          if (line[k] === '"') { closed = true; break; }
          k++;
        }
        if (!closed) { if (/\\\s*$/.test(line)) cont = true; break; }
        j = k + 1;
        continue;
      }
      j++;
    }
  }

  const inLog = new Set();
  for (let i = 0; i < lines.length; i++) {
    if (!/\blog::(trace|debug|info|warn|error)!\s*\(/.test(lines[i])) continue;
    let depth = 0, started = false;
    for (let k = i; k < lines.length; k++) {
      for (const ch of lines[k]) {
        if (ch === "(") { depth++; started = true; }
        else if (ch === ")") depth--;
      }
      inLog.add(k + 1);
      if (started && depth <= 0) break;
    }
  }
  return { inTest, inString, inLog };
}

const analysisCache = new Map();
function analysisOf(rev, path) {
  const k = `${rev}:${path}`;
  if (!analysisCache.has(k)) analysisCache.set(k, analyze(blob(rev, path)));
  return analysisCache.get(k);
}

// ===========================================================================
// Rule R detection
// ===========================================================================

// Lower-case bare `wg` is in the alternation because the sweep's `[Ww][Gg]`
// covers it and because `purge-wg` in prose is a Rule R carrier (plan 5.8).
const WORD = "(?:[Ww]orkgroups?|WORKGROUPS?|WG|Wg|wg)";
// The concept word as a free-standing word: not glued to identifier
// characters, not a field access, not a path or module segment.
const PROSE = new RegExp(`(?:^|[^A-Za-z0-9_%$./])${WORD}(?![A-Za-z0-9_%$])(?!\\s*::)(?!\\.(rs|ts|tsx|md|js))`);
// A wg- directory shape written into text (plan 5.1's last substitution row).
const PROSE_WG_DIR = /(?:^|[^A-Za-z0-9_])wg-|<wgN>|\bwg\d/;

const isDoc = (c) => /^\s*(\/\/\/|\/\/!)/.test(c);
const isNonDocComment = (c) => /^\s*\/\//.test(c) && !isDoc(c);
const inRanges = (rs, n) => (rs ?? []).some(([a, b]) => n >= a && n <= b);
const has = (tbl, p, n) => (tbl[p] ?? []).includes(n);

function quotedSpans(line) {
  const out = [];
  let i = 0;
  while (i < line.length) {
    const rm = /^r(#*)"/.exec(line.slice(i));
    if (rm) {
      const close = '"' + "#".repeat(rm[1].length);
      const at = line.indexOf(close, i + rm[0].length);
      if (at === -1) { out.push(line.slice(i + rm[0].length)); break; }
      out.push(line.slice(i + rm[0].length, at));
      i = at + close.length;
      continue;
    }
    if (line[i] === '"') {
      let k = i + 1;
      while (k < line.length) {
        if (line[k] === "\\") { k += 2; continue; }
        if (line[k] === '"') break;
        k++;
      }
      out.push(line.slice(i + 1, Math.min(k, line.length)));
      i = k + 1;
      continue;
    }
    i++;
  }
  return out;
}

// Returns "MOVE" or a Rule P class name.
function classifyRust(r, rev) {
  // src-tauri/src/api/README.md is inside the Rust sweep's path set because the
  // sweep is path-shaped, but it is documentation and section 6.5 owns it. Both
  // sweeps must classify it identically or one surface would list a row the
  // other subtracts.
  if (r.path.endsWith(".md")) return classifyDocs(r);
  const { inTest, inString, inLog } = analysisOf(rev, r.path);
  if (has(GATES, r.path, r.lineno)) return "MOVE";
  if (has(FROZEN_EXCEPTIONS, r.path, r.lineno)) return "MOVE";
  if (has(CLAP_DOCS, r.path, r.lineno)) return "MOVE";
  if (has(CONTRADICTING_DOCS, r.path, r.lineno)) return "MOVE";
  if (has(TEST_EDITS, r.path, r.lineno)) return "MOVE";
  const ex = STR_EXCEPTIONS[r.path]?.[r.lineno];
  if (ex) return ex;
  if (P2_FILES.includes(r.path)) return "P2-fixture";
  if (inRanges(FROZEN[r.path], r.lineno)) return "P3-frozen";
  if (inTest.has(r.lineno)) return "P2-fixture";
  if (inLog.has(r.lineno)) return "P4-log";
  if (isNonDocComment(r.content)) return "P1-comment";
  if (isDoc(r.content)) return "P1-comment";
  if (inString.has(r.lineno))
    return PROSE.test(r.content) || PROSE_WG_DIR.test(r.content) ? "MOVE" : machineClass(r.content);
  const spans = quotedSpans(r.content);
  if (!spans.some((s) => PROSE.test(s) || PROSE_WG_DIR.test(s))) return machineClass(r.content);
  return "MOVE";
}

// Plan 3.10: name the compatibility-critical classes explicitly, because AC1
// point 7 requires P0-wire, P0-event, P0-token and P0-key to be read by hand
// and a reviewer needs to find them by their class.
function machineClass(c) {
  if (/PURGE_WG_ACTION|"purge-wg"/.test(c)) return "P0-wire";
  if (/"workgroupCreated"|"workgroupRemoved"|"workgroup_task_updated"/.test(c)) return "P0-event";
  if (/%WORKGROUP%|TOKEN_WORKGROUP/.test(c)) return "P0-token";
  if (/"workgroup"\s*:|rename\s*=\s*"workgroup|"workgroupCoordinator"|physical-wg-replica/.test(c))
    return "P0-key";
  return "P0-identifier";
}

function classifyFrontend(r) {
  if (has(FRONTEND_MOVES, r.path, r.lineno)) return "MOVE";
  const c = r.content;
  if (/^\s*(\/\/|\/\*|\*)/.test(c)) return "P1-comment";
  if (r.path === "src/shared/testing/ui-harness.tsx") return "P2-fixture";
  if (/data-ac-testid/.test(c)) return "P0-testid";
  if (/class(Name)?\s*=|ac-wg-|workgroup-group|workgroup-groups|\.wg-|css`/.test(c)) return "P0-css";
  if (/"workgroup_task_updated"|"workgroupCreated"|"workgroupRemoved"/.test(c)) return "P0-event";
  if (/%WORKGROUP%/.test(c)) return "P0-token";
  if (/"workgroupCoordinator"|"selected-workgroup"|"workgroups"|"workgroup"/.test(c)) return "P0-key";
  return "P0-identifier";
}

function classifyDocs(r) {
  const k = DOCS_KEEP[r.path]?.[r.lineno];
  return k ?? "MOVE";
}

export function classify(r, rev) {
  if (r.surface === "rust") return classifyRust(r, rev);
  if (r.surface === "frontend") return classifyFrontend(r);
  return classifyDocs(r);
}

// ===========================================================================

function loadAllowlist() {
  const rows = new Map();
  for (const raw of readFileSync(TSV, "utf8").split("\n")) {
    // .gitattributes pins no eol rule for *.tsv and core.autocrlf is true, so
    // this file is CRLF in a fresh working tree. Strip the CR or every key
    // would carry a trailing \r and match nothing.
    const line = raw.replace(/\r$/, "");
    if (!line || line.startsWith("#")) continue;
    // Part A rows are <class>\t<path>\t<content>. Part B rows carry a fourth
    // column, the one-line justification AC1 point 4 requires, which is NOT
    // part of the key.
    const f = line.split("\t");
    if (f.length < 3) continue;
    rows.set(`${f[1]}\t${f[2]}`, f[0]);
  }
  return rows;
}

function main() {
  const mode = process.argv[2];
  const ri = process.argv.indexOf("--rev");
  const rev = ri > 0 ? process.argv[ri + 1] : undefined;

  if (mode === "sweep" || mode === "moved" || mode === "derive") {
    const partA = [];
    const moved = [];
    const subtotals = {};
    for (const s of Object.keys(SURFACES)) {
      const rows = sweep(s, rev);
      let mv = 0;
      for (const r of rows) {
        const cls = classify(r, rev);
        if (cls === "MOVE") { moved.push(r); mv++; }
        else partA.push({ ...r, cls });
      }
      subtotals[s] = { swept: rows.length, moved: mv, partA: rows.length - mv };
    }
    if (mode === "moved") {
      for (const r of moved) console.log(`${r.path}:${r.lineno}: ${r.content.slice(0, 150)}`);
    }
    if (mode === "sweep") {
      for (const r of partA) console.log(`${r.cls}\t${r.path}\t${r.content}`);
    }
    let sw = 0, mv = 0, pa = 0;
    for (const [s, v] of Object.entries(subtotals)) {
      console.error(`${s.padEnd(9)} swept ${String(v.swept).padStart(5)}  moved ${String(v.moved).padStart(4)}  Part A ${String(v.partA).padStart(5)}`);
      sw += v.swept; mv += v.moved; pa += v.partA;
    }
    console.error(`${"TOTAL".padEnd(9)} swept ${String(sw).padStart(5)}  moved ${String(mv).padStart(4)}  Part A ${String(pa).padStart(5)}`);
    console.error(`closing check: rows(Part A) ${pa} + lines subtracted ${mv} = ${pa + mv}`);

    if (mode === "derive" && process.argv.includes("--write")) {
      const seen = new Set();
      const out = [
        "# scripts/room-rename-allowlist.tsv -- #1614 plan section 9.4 AC1.",
        `# Part A: derived at frozen base ${rev} from the three binding sweeps,`,
        "# minus the lines this plan moves. Committed before the first",
        "# visible-text edit, so an unrenamed Rule R line comes back unlisted.",
        "# Columns: <Rule P class>\\t<path>\\t<trimmed line content>.",
        "#",
        "# Per-surface subtotals at the base (AC1 point 8):",
        ...Object.entries(subtotals).map(
          ([s, v]) => `#   ${s.padEnd(9)} swept ${v.swept}, moved ${v.moved}, Part A ${v.partA}`,
        ),
        `#   closing check: Part A ${pa} + subtracted ${mv} = ${pa + mv}`,
        "#",
        "# Rows are keyed on (path, trimmed content), so the identical trimmed",
        "# content on several lines of one file collapses to ONE row. The",
        "# closing arithmetic above counts LINES; the file below holds rows.",
        "# Regenerate: node scripts/room-rename-allowlist.mjs derive --rev " + rev + " --write",
        "# Check:      node scripts/room-rename-allowlist.mjs gate",
      ];
      for (const r of partA) {
        const line = `${r.cls}\t${r.path}\t${r.content}`;
        if (seen.has(line)) continue; // identical content on two lines of one file
        seen.add(line);
        out.push(line);
      }
      writeFileSync(TSV, out.join("\n") + "\n");
      console.error(`wrote ${TSV}: ${seen.size} unique rows from ${partA.length} Part A lines`);
    }
    return;
  }

  // Emit every unlisted line with FULL content, for building Part B. The gate's
  // own output truncates for readability and must never be parsed for this.
  if (mode === "unlisted") {
    const allow = loadAllowlist();
    for (const s of Object.keys(SURFACES)) {
      for (const r of sweep(s, rev)) {
        if (!allow.has(key(r))) console.log(`${r.path}\t${r.lineno}\t${r.content}`);
      }
    }
    return;
  }

  if (mode === "gate") {
    const allow = loadAllowlist();
    let unlisted = 0;
    for (const s of Object.keys(SURFACES)) {
      const rows = sweep(s, rev);
      let miss = 0;
      for (const r of rows) {
        if (allow.has(key(r))) continue;
        miss++; unlisted++;
        console.log(`UNLISTED ${r.path}:${r.lineno}: ${r.content.slice(0, 160)}`);
      }
      console.error(`${s.padEnd(9)} ${String(rows.length).padStart(5)} lines, ${miss} unlisted`);
    }
    console.error(`unlisted total: ${unlisted}`);
    process.exitCode = unlisted === 0 ? 0 : 1;
    return;
  }

  console.error("usage: room-rename-allowlist.mjs sweep|moved|derive|gate [--rev <rev>] [--write]");
  process.exitCode = 2;
}

main();
