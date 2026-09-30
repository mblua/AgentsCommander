#!/usr/bin/env bash
# Claude Code status line: cwd (branch) [model · effort] ctx% | 5h% | 7d%
node -e '
const { readFileSync } = require("node:fs");
const { execFileSync } = require("node:child_process");
const present = x => x !== undefined && x !== null && x !== false;
const object = (x, name) => {
  if (x === undefined || x === null) return {};
  if (typeof x !== "object" || Array.isArray(x)) throw new Error(name + " must be an object");
  return x;
};
const text = (x, name) => {
  if (x === undefined || x === null) return "";
  if (typeof x !== "string") throw new Error(name + " must be a string");
  return x;
};
const percent = x => {
  if (!present(x)) return "0%";
  if (typeof x !== "number" || !Number.isFinite(x)) throw new Error("percentage must be a finite number");
  return Math.floor(x) + "%";
};
const limit = (x, label) => {
  if (!present(x)) return "";
  if (x !== 0 && x !== "") object(x, label);
  return " | " + label + " " + percent(x.used_percentage);
};
try {
  const input = object(JSON.parse(readFileSync(0, "utf8")), "input");
  const workspace = object(input.workspace, "workspace");
  const dir = text(workspace.current_dir, "workspace.current_dir");
  const cwd = dir.replace(/\\/g, "/").split("/").pop();
  let branch = "";
  if (dir) {
    try {
      branch = execFileSync("git", ["-C", dir, "--no-optional-locks", "branch", "--show-current"],
        { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"], timeout: 2000 }).trimEnd();
    } catch {}
  }
  const model = text(object(input.model, "model").display_name, "model.display_name");
  const effort = object(input.effort, "effort").level;
  if (present(effort) && typeof effort !== "string" && typeof effort !== "number") {
    throw new Error("effort.level must be a string or number");
  }
  const m = model + (present(effort) ? " · " + effort : "");
  const context = object(input.context_window, "context_window");
  const rates = object(input.rate_limits, "rate_limits");
  const row = "\x1b[2;36m" + cwd + (branch ? " (" + branch + ")" : "") +
    "\x1b[0m \x1b[2;33m[" + m + "]\x1b[0m \x1b[2mctx " + percent(context.used_percentage) +
    limit(rates.five_hour, "5h") + limit(rates.seven_day, "7d") + "\x1b[0m";
  process.stdout.write(row.replace(/\r/g, "") + "\n");
} catch (error) {
  process.stderr.write("statusline: " + error.message.replace(/[\r\n]+/g, " ") + "\n");
  process.exitCode = 1;
}
'
