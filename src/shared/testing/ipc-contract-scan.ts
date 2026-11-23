// #4316 - pure scanner for the frontend `invoke` vs Rust `#[tauri::command]` contract.
// Input is path -> text maps; no fs access and no globals. Used only by
// src/shared/ipc-contract.test.ts. Rules R1-R7, F1-F5 and C1-C4 are in the #4316 plan.
import ts from "typescript";

export type Sources = Record<string, string>; // repo-relative "/" path -> file text
export type ContractOptions = {
  wsOnly: readonly string[]; // names served only by the WS dispatcher
  forwarders: Readonly<Record<string, number>>; // file -> allowed non-literal sites
};
export type ContractReport = { failures: string[]; uncalled: string[] }; // both sorted

type RustKey = { key: string; required: boolean };
type RustCommand = { file: string; keys: RustKey[] };
type RustScan = {
  commands: Map<string, RustCommand>;
  registered: Set<string>;
  wsArms: Set<string>;
};
type Call = { cmd: string; loc: string; present: string[]; optional: string[]; ok: boolean };

const EXACT_ATTR = /^[ \t]*#\[tauri::command\][ \t]*\r?$/gm;
const ANY_ATTR = /^[ \t]*#\[\s*tauri::command\b/;
const INJECTED = new RegExp(
  "^(tauri::)?(ipc::|webview::|window::)?" +
    String.raw`(AppHandle|WebviewWindow|Window|Webview|State|Request|Invoke)\b`,
);
const WS_ARM_BEFORE = /"([a-z0-9_]+)"\s*(?==>|\|)/g;
const WS_ARM_AFTER = /\|\s*"([a-z0-9_]+)"/g;

type CommandHead = { start: number; name: string; open: number };

function whitespaceEnd(text: string, index: number): number {
  while (/\s/.test(text.charAt(index))) index++;
  return index;
}

function wordEnd(text: string, index: number): number {
  while (/\w/.test(text.charAt(index))) index++;
  return index;
}

function* commandHeads(text: string): Generator<CommandHead, void> {
  let nextOpen = -1;
  let beforeOpen = -1;
  for (let start = text.indexOf("fn"); start >= 0; start = text.indexOf("fn", start + 2)) {
    if (/\w/.test(text.charAt(start - 1)) || !/\s/.test(text.charAt(start + 2))) continue;
    const nameStart = whitespaceEnd(text, start + 2);
    const nameEnd = wordEnd(text, nameStart);
    if (nameStart === nameEnd) continue;

    const next = whitespaceEnd(text, nameEnd);
    if (text[next] === "(") {
      yield { start, name: text.slice(nameStart, nameEnd), open: next };
    } else if (text[next] === "<") {
      // Candidates in the same span share the first parenthesis and its preceding token.
      if (nextOpen < next) {
        nextOpen = text.indexOf("(", next + 1);
        if (nextOpen < 0) return;
        beforeOpen = nextOpen - 1;
        while (beforeOpen > next && /\s/.test(text.charAt(beforeOpen))) beforeOpen--;
      }
      if (beforeOpen > next && text[beforeOpen] === ">") {
        yield { start, name: text.slice(nameStart, nameEnd), open: nextOpen };
      }
    }
  }
}

function parameterAt(param: string, start: number): { ident: string; type: string } | null {
  if (!/[A-Za-z_]/.test(param.charAt(start))) return null;
  const end = wordEnd(param, start + 1);
  const colon = whitespaceEnd(param, end);
  if (param[colon] !== ":" || colon + 1 === param.length) return null;
  return { ident: param.slice(start, end), type: param.slice(colon + 1) };
}

function splitParameter(param: string): { ident: string; type: string } | null {
  if (param.startsWith("mut") && /\s/.test(param.charAt(3))) {
    const modified = parameterAt(param, whitespaceEnd(param, 3));
    if (modified) return modified;
  }
  return parameterAt(param, 0);
}

function matchClose(text: string, start: number, open: string, close: string): number {
  let depth = 0;
  for (let i = start; i < text.length; i++) {
    if (text[i] === open) depth++;
    else if (text[i] === close && --depth === 0) return i;
  }
  return -1;
}

function splitTop(text: string): string[] {
  const parts: string[] = [];
  let depth = 0;
  let current = "";
  for (const ch of text) {
    if ("<([{".includes(ch)) depth++;
    if (">)]}".includes(ch)) depth--;
    if (ch === "," && depth === 0) {
      parts.push(current);
      current = "";
    } else current += ch;
  }
  parts.push(current);
  return parts.map((p) => p.trim()).filter(Boolean);
}

function isRustTestPath(path: string): boolean {
  const base = path.slice(path.lastIndexOf("/") + 1);
  return path.includes("/tests/") || /^tests\.rs$|_tests?\.rs$/.test(base);
}

function camelKey(ident: string): string {
  return ident.replace(/^_+/, "").replace(/_([A-Za-z0-9])/g, (_, c: string) => c.toUpperCase());
}

function parseParams(name: string, file: string, raw: string, failures: string[]): RustKey[] {
  const keys: RustKey[] = [];
  for (const param of splitTop(raw.replace(/\/\/[^\r\n\u2028\u2029]*/g, ""))) {
    const parsed = splitParameter(param);
    if (!parsed) {
      failures.push(`unparsed parameter "${param}" in ${name} at ${file}`);
      continue;
    }
    const type = parsed.type.trim();
    if (INJECTED.test(type.replace(/^&\s*/, ""))) continue;
    keys.push({ key: camelKey(parsed.ident), required: !/^Option\s*</.test(type) });
  }
  return keys;
}

function scanAttributes(file: string, text: string, failures: string[]): void {
  text.split("\n").forEach((line, i) => {
    if (ANY_ATTR.test(line) && !/^[ \t]*#\[tauri::command\][ \t]*\r?$/.test(line)) {
      failures.push(`unsupported command attribute at ${file}:${i + 1}`);
    }
  });
}

function scanCommands(file: string, text: string, scan: RustScan, failures: string[]): void {
  const heads = commandHeads(text);
  let head: IteratorResult<CommandHead, void> | undefined;
  for (const m of text.matchAll(EXACT_ATTR)) {
    head ??= heads.next();
    while (!head.done && head.value.start < m.index) head = heads.next();
    if (head.done) break;
    const { name, open } = head.value;
    const close = matchClose(text, open, "(", ")");
    const keys = parseParams(name, file, text.slice(open + 1, close), failures);
    const prior = scan.commands.get(name);
    if (prior) failures.push(`duplicate command fn "${name}" in ${prior.file}, ${file}`);
    else scan.commands.set(name, { file, keys });
  }
}

function scanRegistries(text: string, scan: RustScan): void {
  for (const m of text.matchAll(/generate_handler!\s*\[/g)) {
    const open = m.index + m[0].length - 1;
    const body = text.slice(open + 1, matchClose(text, open, "[", "]"));
    for (const entry of splitTop(body)) scan.registered.add(entry.split("::").pop()!.trim());
  }
}

function scanWsArms(text: string, scan: RustScan): void {
  for (const re of [WS_ARM_BEFORE, WS_ARM_AFTER]) {
    // Both WS_ARM regexes have one non-optional group.
    for (const m of text.matchAll(re)) scan.wsArms.add(m[1]!);
  }
}

function scanRust(rust: Sources, failures: string[]): RustScan {
  const scan: RustScan = { commands: new Map(), registered: new Set(), wsArms: new Set() };
  for (const [file, text] of Object.entries(rust)) {
    if (!file.endsWith(".rs") || isRustTestPath(file)) continue;
    scanAttributes(file, text, failures);
    scanCommands(file, text, scan, failures);
    scanRegistries(text, scan);
    if (file.startsWith("src-tauri/src/web/")) scanWsArms(text, scan);
  }
  for (const name of scan.registered) {
    if (!scan.commands.has(name)) {
      failures.push(`registered command "${name}" has no #[tauri::command] fn`);
    }
  }
  return scan;
}

function isFrontendFile(file: string): boolean {
  if (!/\.tsx?$/.test(file) || file.endsWith(".d.ts")) return false;
  return !/\.(test|spec)\./.test(file) && !file.includes("/testing/");
}

function unwrap(node: ts.Expression): ts.Expression {
  return ts.isParenthesizedExpression(node) ? unwrap(node.expression) : node;
}

function propertyName(name: ts.PropertyName): string | null {
  const literal = ts.isIdentifier(name) || ts.isStringLiteral(name) || ts.isNumericLiteral(name);
  return literal ? name.text : null;
}

function byText(a: string, b: string): number {
  return a.localeCompare(b);
}

// Conditional spread: keys of both object-literal branches are optional (F5).
function collectSpread(spread: ts.SpreadAssignment, opt: string[]): boolean {
  const expr = unwrap(spread.expression);
  if (!ts.isConditionalExpression(expr)) return false;
  return [expr.whenTrue, expr.whenFalse].every((branch) => {
    const side = unwrap(branch);
    return ts.isObjectLiteralExpression(side) && collectKeys(side, opt, opt);
  });
}

function keyName(prop: ts.ObjectLiteralElementLike): string | null {
  const named = ts.isPropertyAssignment(prop) || ts.isShorthandPropertyAssignment(prop) ||
    ts.isMethodDeclaration(prop);
  return named ? propertyName(prop.name) : null;
}

// Returns false when the literal cannot be analysed (F5).
function collectKeys(lit: ts.ObjectLiteralExpression, keys: string[], opt: string[]): boolean {
  for (const prop of lit.properties) {
    if (ts.isSpreadAssignment(prop)) {
      if (!collectSpread(prop, opt)) return false;
      continue;
    }
    const name = keyName(prop);
    if (name === null) return false;
    keys.push(name);
  }
  return true;
}

function argsLiteral(arg: ts.Expression): ts.ObjectLiteralExpression | null {
  const node = unwrap(arg);
  if (ts.isObjectLiteralExpression(node)) return node;
  if (ts.isCallExpression(node) && node.arguments[0]) {
    const first = unwrap(node.arguments[0]);
    if (ts.isObjectLiteralExpression(first)) return first;
  }
  return null;
}

function readCall(node: ts.CallExpression, cmd: string, loc: string, failures: string[]): Call {
  const call: Call = { cmd, loc, present: [], optional: [], ok: true };
  const arg = node.arguments[1];
  if (!arg) return call;
  const lit = argsLiteral(arg);
  if (!lit || !collectKeys(lit, call.present, call.optional)) {
    failures.push(`unanalyzable args for "${cmd}" at ${loc}`);
    call.ok = false;
  }
  return call;
}

function isInvokeCallee(callee: ts.Expression): boolean {
  if (ts.isIdentifier(callee)) return callee.text === "invoke";
  return ts.isPropertyAccessExpression(callee) && callee.name.text === "invoke";
}

function scanFrontendFile(file: string, text: string, calls: Call[], failures: string[]): string[] {
  const kind = file.endsWith(".tsx") ? ts.ScriptKind.TSX : ts.ScriptKind.TS;
  const sf = ts.createSourceFile(file, text, ts.ScriptTarget.Latest, true, kind);
  const nonLiteral: string[] = [];
  const visit = (node: ts.Node): void => {
    if (ts.isCallExpression(node) && isInvokeCallee(node.expression)) {
      const loc = `${file}:${sf.getLineAndCharacterOfPosition(node.getStart(sf)).line + 1}`;
      const first = node.arguments[0];
      if (first && ts.isStringLiteralLike(first)) {
        calls.push(readCall(node, first.text, loc, failures));
      } else nonLiteral.push(loc);
    }
    ts.forEachChild(node, visit);
  };
  visit(sf);
  return nonLiteral;
}

function scanFrontend(frontend: Sources, opts: ContractOptions, failures: string[]): Call[] {
  const calls: Call[] = [];
  for (const [file, text] of Object.entries(frontend)) {
    if (!isFrontendFile(file)) continue;
    const sites = scanFrontendFile(file, text, calls, failures);
    const allowed = opts.forwarders[file] ?? 0;
    if (sites.length !== allowed) {
      failures.push(
        `non-literal command sites in ${file}: found ${sites.length}, allowed ${allowed} ` +
          `(${sites.join(", ")})`,
      );
    }
  }
  return calls;
}

function checkWsOnly(scan: RustScan, called: Set<string>, opts: ContractOptions): string[] {
  const failures: string[] = [];
  for (const name of opts.wsOnly) {
    if (!scan.wsArms.has(name)) {
      failures.push(`ws-only command "${name}" has no web dispatcher arm`);
    }
    if (scan.registered.has(name)) {
      failures.push(`ws-only command "${name}" is a registered Tauri command`);
    }
    if (!called.has(name)) failures.push(`ws-only command "${name}" is not called`);
  }
  return failures;
}

function checkKeys(call: Call, command: RustCommand): string[] {
  const failures: string[] = [];
  const rustKeys = new Set(command.keys.map((k) => k.key));
  for (const key of [...call.present, ...call.optional]) {
    if (!rustKeys.has(key)) {
      failures.push(`extra arg key "${key}" for "${call.cmd}" at ${call.loc}`);
    }
  }
  for (const key of command.keys) {
    if (key.required && !call.present.includes(key.key)) {
      failures.push(`missing required arg key "${key.key}" for "${call.cmd}" at ${call.loc}`);
    }
  }
  return failures;
}

function checkCalls(scan: RustScan, calls: Call[], opts: ContractOptions): string[] {
  const failures: string[] = [];
  for (const call of calls) {
    if (opts.wsOnly.includes(call.cmd)) continue;
    if (!scan.registered.has(call.cmd)) {
      failures.push(`unknown command "${call.cmd}" at ${call.loc}`);
      continue;
    }
    const command = scan.commands.get(call.cmd);
    if (command && call.ok) failures.push(...checkKeys(call, command));
  }
  return failures;
}

export function checkContract(
  rust: Sources,
  frontend: Sources,
  opts: ContractOptions,
): ContractReport {
  const failures: string[] = [];
  const scan = scanRust(rust, failures);
  const calls = scanFrontend(frontend, opts, failures);
  const called = new Set(calls.map((c) => c.cmd));
  failures.push(...checkWsOnly(scan, called, opts), ...checkCalls(scan, calls, opts));
  const uncalled = [...scan.registered].filter((name) => !called.has(name));
  failures.sort(byText);
  uncalled.sort(byText);
  return { failures, uncalled };
}
