use std::process::Command;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

/// #992 - Does this process own a console?
///
/// `GetConsoleWindow()` is NOT this test. It returns NULL for a ConPTY-attached
/// process, which does own a console. `GetConsoleCP()` fails, returning 0, only when
/// no console is attached at all. Measured on Windows 11: 437 from a console-owning
/// parent (ConPTY included), 0 from a GUI-subsystem parent launched without one.
#[cfg(target_os = "windows")]
fn process_owns_console() -> bool {
    use windows_sys::Win32::System::Console::GetConsoleCP;
    unsafe { GetConsoleCP() != 0 }
}

pub(crate) fn run_argv(argv: &[String]) -> std::io::Result<std::process::ExitStatus> {
    argv_command(argv).status()
}

pub(crate) fn run_raw_shell(raw: &str) -> std::io::Result<std::process::ExitStatus> {
    platform_shell_command(raw).status()
}

fn argv_command(argv: &[String]) -> Command {
    let mut command = Command::new(&argv[0]);
    crate::config::agent_path::apply_search_path_to_std_command(&mut command);
    command.args(&argv[1..]);
    // #992 - the release binary is GUI-subsystem, and `attach_parent_console`
    // deliberately does NOT attach when the std handles are already valid
    // (inherited pipes, see cli/mod.rs and issue #129). Driven from PowerShell
    // automation, AC therefore owns no console, and a console-subsystem child
    // spawned without the flag makes Windows allocate a NEW console for it: a
    // stray Windows Terminal tab, while the output goes to the inherited pipes
    // where the caller is actually reading it. When we DO own a console the
    // child inherits it and the flag is a no-op, so leave it off and keep the
    // interactive case exactly as it was.
    #[cfg(target_os = "windows")]
    if !process_owns_console() {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// #992 - see `argv_command`. Same reasoning, same condition.
#[cfg(target_os = "windows")]
fn platform_shell_command(raw: &str) -> Command {
    let mut command = Command::new("cmd.exe");
    command.args(["/C", raw]);
    if !process_owns_console() {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

#[cfg(not(target_os = "windows"))]
fn platform_shell_command(raw: &str) -> Command {
    let mut command = Command::new("sh");
    crate::config::agent_path::apply_search_path_to_std_command(&mut command);
    command.args(["-c", raw]);
    command
}

#[cfg(test)]
mod tests;
