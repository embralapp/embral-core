//! Process-spawning helpers: console suppression, executable naming, and
//! CLI resolution ([integrations.md](../../../../docs/integrations.md)).

use std::path::{Path, PathBuf};
use std::time::Duration;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Keep a child from flashing a console window.
pub fn hide_console(cmd: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

/// [`hide_console`] for tokio-spawned children.
pub fn hide_console_tokio(cmd: &mut tokio::process::Command) {
    cmd.creation_flags(CREATE_NO_WINDOW);
}

/// The platform spelling of an executable name (`embral-mcp` → `embral-mcp.exe`).
pub fn exe_name(base: &str) -> String {
    format!("{base}.exe")
}

/// Run a probe command windowless, bounded so a hung binary can't wedge a
/// settings page. Shared by [`find_cli`] and the MSIX package probe.
pub(super) async fn bounded_output(
    exe: &Path,
    args: &[&str],
) -> Result<std::process::Output, String> {
    let mut cmd = tokio::process::Command::new(exe);
    cmd.args(args).stdin(std::process::Stdio::null());
    hide_console_tokio(&mut cmd);
    tokio::time::timeout(Duration::from_secs(15), cmd.output())
        .await
        .map_err(|_| format!("{} timed out after 15 s", exe.display()))?
        .map_err(|e| format!("{}: {e}", exe.display()))
}

/// First `where.exe` hit, preferring `.exe` over `.cmd` (npm shims are
/// `.cmd`; `Command::new("claude")` alone would miss them, since
/// CreateProcess only appends `.exe`).
pub async fn find_cli(name: &str) -> Option<PathBuf> {
    let output = bounded_output(Path::new("where.exe"), &[name]).await.ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    lines
        .iter()
        .find(|l| l.to_lowercase().ends_with(".exe"))
        .or_else(|| lines.iter().find(|l| l.to_lowercase().ends_with(".cmd")))
        .or_else(|| lines.first())
        .map(PathBuf::from)
}
