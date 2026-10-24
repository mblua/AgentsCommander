/// Maximum exact UTF-8 payload accepted by privileged PTY input.
///
/// The payload is handed to a backend in one call. It is never split into
/// chunks, appended with Enter, or interpreted as a command line.
pub const PTY_INPUT_MAX_BYTES: usize = 65_536;
