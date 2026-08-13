//! Children die with this process, however it dies: the reaper pattern
//! ([architecture.md](../../../../docs/architecture.md) §Process/threading).
//!
//! macOS has no job-object equivalent and no `PR_SET_PDEATHSIG`, and the
//! children (`llama-server`) are unmodifiable third-party binaries. So:
//! at startup the app re-execs itself as `embral --child-reaper`, holding
//! the write end of a pipe wired to the reaper's stdin. Managed children
//! spawn into their own process groups (`prepare_spawn`), and their pgids
//! are written down the pipe (`watch_child`). When this process dies
//! (cleanly, by crash, by SIGKILL, by a dev-loop rebuild), the kernel
//! closes the pipe, the reaper's stdin hits EOF, and it `killpg`s every
//! registered group: SIGTERM, a short grace, SIGKILL. Pipe EOF can't
//! suffer pid-reuse races, which is why it beats watching the parent pid.

use std::io::Write;
use std::sync::Mutex;

/// The reaper's stdin (write end). `None` until init, or if the spawn
/// failed; then children are only cleaned up by the clean-quit path.
static REAPER: Mutex<Option<std::process::ChildStdin>> = Mutex::new(None);

/// Grace between SIGTERM and SIGKILL in the reaper.
const GRACE: std::time::Duration = std::time::Duration::from_secs(2);

/// Spawn the reaper subprocess. Failure degrades to clean-quit-only
/// cleanup with a warning, never fatal.
pub fn kill_children_with_us() {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("child reaper: no current_exe ({e}); orphan cleanup disabled");
            return;
        }
    };
    let spawned = std::process::Command::new(exe)
        .arg("--child-reaper")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    match spawned {
        Ok(mut child) => {
            *REAPER.lock().expect("reaper mutex poisoned") = child.stdin.take();
            // The Child handle is dropped without wait(): the reaper outlives
            // us by design, and PID 1 reaps it after it finishes.
        }
        Err(e) => {
            tracing::warn!("child reaper failed to spawn ({e}); orphan cleanup disabled");
        }
    }
}

/// Put a managed child into its own process group so the reaper can
/// `killpg` it (and its own descendants) as a unit.
pub fn prepare_spawn(cmd: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    cmd.process_group(0);
}

/// [`prepare_spawn`] for tokio-spawned children.
pub fn prepare_spawn_tokio(cmd: &mut tokio::process::Command) {
    cmd.process_group(0);
}

/// Register a spawned child's pgid (== its pid, per `prepare_spawn`) with
/// the reaper.
pub fn watch_child(pid: u32) {
    let mut guard = REAPER.lock().expect("reaper mutex poisoned");
    if let Some(stdin) = guard.as_mut() {
        if writeln!(stdin, "{pid}").and_then(|_| stdin.flush()).is_err() {
            tracing::warn!("child reaper pipe is gone; orphan cleanup disabled");
            *guard = None;
        }
    }
}

/// The `--child-reaper` subprocess body: park on stdin until EOF (any
/// parent death closes the pipe), then take every registered group down.
pub fn run_reaper() {
    use std::io::BufRead;

    let stdin = std::io::stdin();
    let mut pgids: Vec<i32> = Vec::new();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if let Ok(pgid) = line.trim().parse::<i32>() {
            pgids.push(pgid);
        }
    }
    // EOF: the parent is gone. ESRCH (already exited) is the common,
    // harmless case after a clean quit.
    for &pgid in &pgids {
        unsafe { libc::killpg(pgid, libc::SIGTERM) };
    }
    std::thread::sleep(GRACE);
    for &pgid in &pgids {
        unsafe { libc::killpg(pgid, libc::SIGKILL) };
    }
}
