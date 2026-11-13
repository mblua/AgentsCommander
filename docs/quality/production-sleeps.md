# Sleeps in production code

A fixed sleep in production Rust code is allowed only for one of these reasons:

1. **Tick or timer.** The time is the requirement: periodic work, debounce, rate limit,
   batch window, sample gap. Inside a loop that must stop, race it with the shutdown or
   cancel signal in `tokio::select!`.
2. **Backoff.** A retry after a named transient error (for example a Windows sharing
   violation). Bound the attempts or the total time.
3. **Outside state with no signal.** Another process, an OS file lock, a file another
   process writes, OS process or job accounting, a remote status. Bound it with a deadline
   and use `interval.min(remaining)`.
4. **Non-blocking lock spin.** A `try_lock` loop on a path that must not block (exit,
   shutdown). Bound it with a deadline.
5. **Fixed delay for a consumer with no ack** (for example an agent TUI that needs time
   before Enter). Name the consumer and why no signal exists.

If the code waits for state inside this process (a lock, a flag, a count, a task, a
session state), wait on a signal instead: a channel, `tokio::sync::Notify`,
`tokio::sync::watch`, a `JoinHandle`, a condvar, or the child process exit. Add a timeout
only where the old code had one or the caller needs one.

Every allowed sleep states its reason in a short comment or in the name of its constant
(`LOCK_POLL_INTERVAL`, `RETRY_BACKOFFS_MS`). Tests are covered by #4545, not by this rule.
The classification of all sites on e76fd728 is in issue #4568.
