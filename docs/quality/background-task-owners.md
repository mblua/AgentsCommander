# Background task owners

A spawned task or thread that can outlive the call that starts it MUST have an owner.
This includes loops on external input, waits without a bound, and dropped handles.
The owner keeps a `JoinHandle`/`JoinSet`, or passes a
`tokio_util::sync::CancellationToken`, and cancels the task when the owner closes.

For app-lifetime tasks, the owner is `crate::shutdown::ShutdownSignal`.
Pass `shutdown.token().child_token()` and prefer cancellation when both arms are ready:

```rust
tokio::select! {
    biased;
    () = token.cancelled() => return,
    // Work arm goes here.
}
```

See `src-tauri/src/events/pty.rs` for an app-lifetime example.

No owner is needed when the handle is awaited/joined or the body is a bounded one-shot.
An intentionally detached task needs a comment at the spawn explaining why;
`src-tauri/src/config/settings/save/lock.rs` is a process-lifetime example.
Native threads without a runtime can use `ShutdownSignal::is_cancelled()` or channel closure.

Measured on 3d224b05: 189 production spawns; O 42, F 118, N 2, D 1, H 2, W 24.
There are 2 direct no-owner sites plus 1 reached through a wrapper (counted in W 24).
#4579 fixed the error-log ping loop; the Wayland portal hotkey tasks remain (follow-up).
