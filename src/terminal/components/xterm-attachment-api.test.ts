// @vitest-environment jsdom
import { afterAll, afterEach, beforeAll, describe, expect, it } from "vitest";
import type { Terminal as XtermTerminal } from "@xterm/xterm";

const terminals: XtermTerminal[] = [];
let Terminal: typeof import("@xterm/xterm")["Terminal"];
let previousCanvasContext: typeof HTMLCanvasElement.prototype.getContext;

beforeAll(async () => {
  previousCanvasContext = HTMLCanvasElement.prototype.getContext;
  const getContext = function getContext(this: HTMLCanvasElement) {
    return {
      canvas: this,
      measureText: () => ({ width: 10 }),
    } as unknown as CanvasRenderingContext2D;
  } as unknown as typeof HTMLCanvasElement.prototype.getContext;
  HTMLCanvasElement.prototype.getContext = getContext;
  ({ Terminal } = await import("@xterm/xterm"));
});

afterAll(() => {
  HTMLCanvasElement.prototype.getContext = previousCanvasContext;
});

function createTerminal(rows = 24): XtermTerminal {
  const terminal = new Terminal({ cols: 81, rows, scrollback: 100 });
  terminals.push(terminal);
  return terminal;
}

function writeComplete(terminal: XtermTerminal, data: string | Uint8Array): Promise<void> {
  return new Promise((resolve) => terminal.write(data, resolve));
}

function expectVisibleBufferInvariant(terminal: XtermTerminal): void {
  const active = terminal.buffer.active;
  expect(active.length).toBeGreaterThanOrEqual(active.baseY + terminal.rows);
  for (let row = 0; row < terminal.rows; row += 1) {
    expect(active.getLine(active.baseY + row)).toBeDefined();
  }
}

afterEach(() => {
  for (const terminal of terminals.splice(0)) terminal.dispose();
});

describe("installed xterm 6 attachment APIs", () => {
  it("runs write callbacks asynchronously after parser state is available", async () => {
    const terminal = createTerminal();
    let callbackRan = false;
    const completion = new Promise<void>((resolve) => {
      terminal.write(new Uint8Array([65, 66, 67]), () => {
        callbackRan = true;
        resolve();
      });
    });

    expect(callbackRan).toBe(false);
    await completion;
    expect(callbackRan).toBe(true);
    expect(terminal.buffer.active.getLine(0)?.translateToString(true)).toBe("ABC");
    expectVisibleBufferInvariant(terminal);
  });

  it("observes replay semantics before retained parsing and resolves the retained fence last", async () => {
    const terminal = createTerminal();
    const order: string[] = [];
    await new Promise<void>((resolve) => {
      terminal.write("\x1b[2J\x1b[H", () => {
        order.push(
          `replay-observation:${terminal.buffer.active
            .getLine(0)
            ?.translateToString(true)}`,
        );
        terminal.write("retained");
        terminal.write("", () => {
          order.push(
            `retained-fence:${terminal.buffer.active
              .getLine(0)
              ?.translateToString(true)}`,
          );
          resolve();
        });
      });
    });

    expect(order).toEqual(["replay-observation:", "retained-fence:retained"]);
    expectVisibleBufferInvariant(terminal);
  });

  it("preserves normal history across alternate switching and 81x24 to 81x27 resize reversal", async () => {
    const terminal = createTerminal(24);
    const history = Array.from({ length: 35 }, (_, index) => `line-${index}\r\n`).join("");
    await writeComplete(terminal, history);
    expect(terminal.buffer.active.type).toBe("normal");
    expect(terminal.buffer.active.baseY).toBeGreaterThan(0);
    expectVisibleBufferInvariant(terminal);

    await writeComplete(terminal, "\x1b[?1049hALT");
    expect(terminal.buffer.active.type).toBe("alternate");
    expect(
      Array.from({ length: terminal.rows }, (_, row) =>
        terminal.buffer.active.getLine(row)?.translateToString(true),
      ),
    ).toContain("ALT");
    expectVisibleBufferInvariant(terminal);

    terminal.resize(81, 27);
    expect({ cols: terminal.cols, rows: terminal.rows }).toEqual({ cols: 81, rows: 27 });
    expectVisibleBufferInvariant(terminal);
    terminal.resize(81, 24);
    expect({ cols: terminal.cols, rows: terminal.rows }).toEqual({ cols: 81, rows: 24 });
    expectVisibleBufferInvariant(terminal);

    await writeComplete(terminal, "\x1b[?1049l");
    expect(terminal.buffer.active.type).toBe("normal");
    expect(terminal.buffer.active.baseY).toBeGreaterThan(0);
    expect(
      Array.from({ length: terminal.buffer.active.length }, (_, row) =>
        terminal.buffer.active.getLine(row)?.translateToString(true),
      ),
    ).toContain("line-34");
    expectVisibleBufferInvariant(terminal);
  });
});
