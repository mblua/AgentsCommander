# Plan #2095: Move TauriTransport async init out of the constructor

Status: READY_FOR_IMPLEMENTATION

Issue: [mblua/AgentsCommander#2095](https://github.com/mblua/AgentsCommander/issues/2095). Branch: `fix/2095-sonar-async-constructor`. Lite.

## Cause
SonarCloud key `AaBBZrdibbRnCQnTRHWm`, rule `typescript:S7059`: `src/shared/transport-tauri.ts:42` starts an async operation (`this.ready = this.init();`) inside the constructor.

## Constraints (behavior must not change)
- `init()` must start eagerly, synchronously with obtaining the instance (#1363: a lazy start opens a window where `pty_output` is lost).
- `invoke`/`listen`/`emit` keep awaiting `this.ready`; `noteInvokeStart` stays before `await this.ready` (#1652).
- `init()` body untouched; it still never rejects.

## Decided solution
Static factory; constructor stays synchronous; its body holds only an explanatory comment (avoids `typescript:S1186`).

```ts
private ready!: Promise<void>;

private constructor() {
  // Use TauriTransport.create(): init() must start eagerly, in the same
  // synchronous step that hands out the instance (#1363), and async work
  // does not belong in a constructor (Sonar S7059).
}

static create(): TauriTransport {
  const transport = new TauriTransport();
  transport.ready = transport.init();
  return transport;
}
```

`create()` starts `init()` before returning, so every caller gets an instance whose `ready` is already in flight — identical timing to today. `private constructor` forces all construction through `create()`, so no instance can exist with `ready` unset.

Rejected: field initializer `private ready = this.init()` (same async-in-construction semantics, only hides it from the rule); lazy `ready` getter (breaks #1363 eager start).

## Files / symbols
1. `src/shared/transport-tauri.ts` — `TauriTransport`: add `static create()`, make constructor private with only the explanatory comment block (an empty body could trigger Sonar `typescript:S1186`), `ready` gets definite-assignment `!`.
2. `src/shared/ipc.ts:123` — `createDefaultTransport`: `new TauriTransport()` -> `TauriTransport.create()`.
3. `src/shared/transport-tauri.test.ts:54,67,85` — `new TauriTransport()` -> `TauriTransport.create()`.

No other references (`grep -rn TauriTransport src`).

## Edge cases
- Non-Tauri env: `create()` is only reached when `isTauri`; unchanged.
- `init()` failure path: unchanged, still caught internally.
- Singleton caching in `ipc.ts` (`defaultTransport`) unchanged.

## Tests
- Existing `src/shared/transport-tauri.test.ts` must pass unchanged in assertions.
- `npx tsc --noEmit` clean (private constructor enforces no stray `new`).
- `npx vitest run src/shared` green.

## Acceptance criteria
- No `new TauriTransport()` left in `src/`.
- Constructor contains no async call and is not empty (no S1186); SonarCloud no longer reports S7059 on `transport-tauri.ts` (issue `AaBBZrdibbRnCQnTRHWm` closed on the PR analysis) and no new issue is introduced.
- Typecheck and tests green; no runtime behavior change.
