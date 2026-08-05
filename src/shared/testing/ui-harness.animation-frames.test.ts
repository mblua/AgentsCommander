// #1206, plan 5.4: "`restore()` must be safe on a partially initialized
// controller and must not mask an error thrown during install."
//
// The partial-install path is narrow and unreachable by ordinary use, so it is
// reached here by fault injection rather than by arranging a real failure:
// `Object.defineProperty` is replaced for the duration of one call, and each
// injected fault names the exact global and the exact call ordinal it fails on.
// Install is call 1 for a given global and the cleanup attempt is call 2, so a
// fault pair can put an install failure and a cleanup failure on the same run
// and tell them apart by their sentinel.
import { afterEach, describe, expect, it } from "vitest";
import { installDeterministicAnimationFrames } from "./ui-harness";

interface DefinePropertyFault {
  property: string;
  call: number;
  message: string;
}

interface DefinePropertyAttempt {
  property: string;
  call: number;
}

const realDefineProperty = Object.defineProperty;

function forceGlobal(name: string, value: unknown): void {
  realDefineProperty(globalThis, name, { configurable: true, writable: true, value });
}

/** Runs `body` with `Object.defineProperty` failing on the named globals and
 *  call ordinals, and returns every attempt it saw against `globalThis`. The
 *  attempt list is what makes "the cleanup still tried the other global"
 *  observable, rather than inferred from the value left behind. */
function withDefinePropertyFaults(
  faults: readonly DefinePropertyFault[],
  body: () => void
): DefinePropertyAttempt[] {
  const attempts: DefinePropertyAttempt[] = [];
  const calls = new Map<string, number>();

  const stub = ((target: object, property: PropertyKey, descriptor: PropertyDescriptor) => {
    if (target === globalThis && typeof property === "string") {
      const call = (calls.get(property) ?? 0) + 1;
      calls.set(property, call);
      attempts.push({ property, call });

      const fault = faults.find((f) => f.property === property && f.call === call);
      if (fault) {
        throw new Error(fault.message);
      }
    }
    return realDefineProperty(target, property, descriptor);
  }) as typeof Object.defineProperty;

  realDefineProperty(Object, "defineProperty", {
    configurable: true,
    writable: true,
    value: stub,
  });
  try {
    body();
  } finally {
    realDefineProperty(Object, "defineProperty", {
      configurable: true,
      writable: true,
      value: realDefineProperty,
    });
  }

  return attempts;
}

function installCatching(faults: readonly DefinePropertyFault[]): {
  thrown: unknown;
  attempts: DefinePropertyAttempt[];
} {
  let thrown: unknown = null;
  const attempts = withDefinePropertyFaults(faults, () => {
    try {
      installDeterministicAnimationFrames();
    } catch (error) {
      thrown = error;
    }
  });
  return { thrown, attempts };
}

const INSTALL_FAILURE = "install failure sentinel";
const RESTORE_FAILURE = "restore failure sentinel";

describe("installDeterministicAnimationFrames partial install (#1206, plan 5.4)", () => {
  const originalRequestAnimationFrame = globalThis.requestAnimationFrame;
  const originalCancelAnimationFrame = globalThis.cancelAnimationFrame;

  afterEach(() => {
    // Each case deliberately stops the helper from restoring one of the two
    // globals, so both are put back by force rather than by the helper.
    forceGlobal("requestAnimationFrame", originalRequestAnimationFrame);
    forceGlobal("cancelAnimationFrame", originalCancelAnimationFrame);
  });

  it("rethrows the install error when the cleanup's second restoration also fails", () => {
    const { thrown, attempts } = installCatching([
      { property: "cancelAnimationFrame", call: 1, message: INSTALL_FAILURE },
      { property: "cancelAnimationFrame", call: 2, message: RESTORE_FAILURE },
    ]);

    // The cleanup ran and its own failure is real: it is simply not the story.
    expect(attempts).toContainEqual({ property: "requestAnimationFrame", call: 2 });
    expect(attempts).toContainEqual({ property: "cancelAnimationFrame", call: 2 });

    expect(thrown).toBeInstanceOf(Error);
    expect((thrown as Error).message).toBe(INSTALL_FAILURE);
  });

  it("rethrows the install error, and still tries the other global, when the cleanup's first restoration fails", () => {
    const { thrown, attempts } = installCatching([
      { property: "cancelAnimationFrame", call: 1, message: INSTALL_FAILURE },
      { property: "requestAnimationFrame", call: 2, message: RESTORE_FAILURE },
    ]);

    // Best effort per property: one restoration failing must not skip the next.
    expect(attempts).toContainEqual({ property: "cancelAnimationFrame", call: 2 });

    expect(thrown).toBeInstanceOf(Error);
    expect((thrown as Error).message).toBe(INSTALL_FAILURE);
  });

  it("still installs and restores normally when nothing is injected", () => {
    const frames = installDeterministicAnimationFrames();
    expect(globalThis.requestAnimationFrame).not.toBe(originalRequestAnimationFrame);

    frames.restore();
    expect(globalThis.requestAnimationFrame).toBe(originalRequestAnimationFrame);
    expect(globalThis.cancelAnimationFrame).toBe(originalCancelAnimationFrame);
  });
});
