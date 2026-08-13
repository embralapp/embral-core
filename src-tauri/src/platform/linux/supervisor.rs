//! Children die with this process, however it dies: the reaper pattern
//! ([architecture.md](../../../../docs/architecture.md) §Process/threading).
//!
//! Linux has the kernel feature the other two platforms have to emulate:
//! `PR_SET_PDEATHSIG` asks the kernel to signal this process when its
//! parent goes away. It is set in the child, after `fork` and before `exec`
//! (a `pre_exec` hook), and it survives the exec, so an unmodifiable
//! third-party binary like `llama-server` needs no cooperation. No reaper
//! subprocess (macOS), no job object (Windows), and no pid-reuse race.
//!
//! **`run_reaper` is therefore a no-op and `--child-reaper` is never
//! passed**, which also sidesteps a trap the macOS design would hit inside
//! an AppImage: re-execing `current_exe()` from a squashfs mount that
//! unmounts when the main process dies.
//!
//! Two properties of `PR_SET_PDEATHSIG` shape the code:
//!
//! 1. It tracks the parent thread, not the parent process. If the
//!    thread that spawned the child exits, the child is signalled even
//!    though the app is alive and well. Both managed children are spawned
//!    from `async fn`s (`llm.rs`'s `ensure_running` and
//!    `search_index.rs`'s `EmbedPipe::spawn`), that is, from tokio worker
//!    threads, which live as long as the runtime. That is what makes this
//!    correct today. Moving either spawn onto `tokio::task::spawn_blocking`
//!    would break it: blocking-pool threads retire after ~10s idle, and
//!    the child would take a SIGTERM mid-request. If that move ever happens,
//!    this module must move to the macOS pipe-reaper instead.
//! 2. **It signals only the direct child**, not a whole tree. Both of ours
//!    are single-process, so there is no tree to miss; a future child that
//!    forks would need its own process group and a `killpg`.
//!
//! There is also a narrow race: if the parent dies between `fork` and the
//! `prctl` call, the signal is never armed and the child would be orphaned.
//! The hook closes it by re-reading `getppid()` after arming and exiting
//! immediately if the parent is already gone.

/// Nothing to arm up-front: the guarantee is established per spawn, in
/// [`prepare_spawn`]. Present so `lib.rs` calls it unconditionally like the
/// other platforms.
pub fn kill_children_with_us() {
    tracing::debug!("child cleanup on Linux is per-spawn PR_SET_PDEATHSIG");
}

/// Ask the kernel to SIGTERM this child when we die. See the module doc for
/// the thread-affinity requirement this carries.
pub fn prepare_spawn(cmd: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    // SAFETY: `pre_exec` runs in the forked child between fork and exec,
    // where only async-signal-safe work is allowed. `prctl` and `getppid`
    // are both plain syscalls, and `_exit` avoids running atexit handlers
    // in a child that must not do so.
    unsafe {
        cmd.pre_exec(|| {
            arm_pdeathsig();
            Ok(())
        });
    }
}

/// [`prepare_spawn`] for tokio-spawned children.
pub fn prepare_spawn_tokio(cmd: &mut tokio::process::Command) {
    // SAFETY: as `prepare_spawn`; tokio forwards this to the same
    // `pre_exec` slot on the underlying std Command.
    unsafe {
        cmd.pre_exec(|| {
            arm_pdeathsig();
            Ok(())
        });
    }
}

/// The child-side hook: arm the death signal, then confirm we still have the
/// parent we armed against.
fn arm_pdeathsig() {
    // The parent we intend to die with, read before arming.
    let parent = unsafe { libc::getppid() };
    unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) };
    // If the parent died in the window between fork and prctl, the signal
    // will never come; getppid() has already been reparented (to init, or
    // to a subreaper). Leave now rather than linger as an orphan.
    if unsafe { libc::getppid() } != parent {
        unsafe { libc::_exit(0) };
    }
}

/// Nothing to register: [`prepare_spawn`] already armed the child before it
/// exec'd, so there is no post-spawn window to close (unlike the job-object
/// and reaper-pipe designs, which both accept one).
pub fn watch_child(_pid: u32) {}

/// The `--child-reaper` subprocess body. Never runs on Linux: the kernel
/// does this job, so the flag is never passed. Present so the caller needs
/// no `cfg` (`platform/mod.rs`).
pub fn run_reaper() {}

#[cfg(test)]
mod tests {
    /// The guarantee itself needs an out-of-process kill to observe, so it
    /// is a manual probe like its Windows twin. Run with
    /// `cargo test -p embral --lib pdeathsig -- --ignored --nocapture`,
    /// `kill -9` the printed probe pid, and confirm the sleep pid is gone.
    #[test]
    #[ignore = "manual probe; kill this process externally and confirm the child dies"]
    fn pdeathsig_takes_the_child_with_us() {
        let mut cmd = std::process::Command::new("sleep");
        cmd.arg("600")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        super::prepare_spawn(&mut cmd);
        let child = cmd.spawn().expect("spawn the dummy child");
        super::watch_child(child.id());
        println!(
            "probe pid {} / child sleep pid {} — kill -9 the probe, check the sleep",
            std::process::id(),
            child.id()
        );
        std::thread::sleep(std::time::Duration::from_secs(600));
    }
}
