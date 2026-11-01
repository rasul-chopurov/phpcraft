//! Child processes (git, php, gh, …) for a GUI app.

use std::ffi::OsStr;
use std::process::Command;

/// `Command::new` that on Windows doesn't give the child a console window: without this every
/// `git` or `php` run allocates a console (slow, and it flashes on screen).
pub fn command(program: impl AsRef<OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut command = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}
