//! What every child process the app starts gets (password commands, bootstrap scripts, the
//! `ProxyCommand`, the dump / restore tools and their version probes, the MCP client CLIs).

/// Windows' `CREATE_NO_WINDOW` process creation flag.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub trait NoWindow {
    /// Starts a console program without a console window of its own. Release builds on Windows
    /// are GUI programs (`windows_subsystem = "windows"`), so Windows would otherwise open one for
    /// every process; their output is piped or goes to a file anyway. Nothing changes elsewhere.
    fn no_window(&mut self) -> &mut Self;
}

impl NoWindow for tokio::process::Command {
    fn no_window(&mut self) -> &mut Self {
        #[cfg(windows)]
        self.creation_flags(CREATE_NO_WINDOW);
        self
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// Without a console, a program still writes to its pipes.
    #[tokio::test]
    async fn a_windowless_process_still_has_its_output() {
        let out = tokio::process::Command::new("cmd")
            .no_window()
            .args(["/d", "/c", "echo hi"])
            .output()
            .await
            .unwrap();
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "hi");
    }
}
