//! Children die with this process, however it dies
//! ([architecture.md](../../../../docs/architecture.md) §Process/threading).

use std::sync::OnceLock;

/// The kill-on-close job object, as a pointer-sized integer (`HANDLE` is a
/// raw pointer and so not `Send`). Deliberately never closed: it closes
/// when this process exits, and that close is what takes the registered
/// children down.
static JOB: OnceLock<isize> = OnceLock::new();

fn job_handle() -> Option<windows::Win32::Foundation::HANDLE> {
    JOB.get()
        .map(|&h| windows::Win32::Foundation::HANDLE(h as *mut core::ffi::c_void))
}

/// Create a job object that kills every registered child when this
/// process dies, however it dies. A clean quit already stops the
/// sidecars (`RunEvent::Exit`), but a dev-loop rebuild, a crash, or a
/// task-manager kill skips that path and used to orphan `llama-server.exe`,
/// which then held its own files open and made every re-download fail with
/// "access denied" (the NTFS delete-pending trap).
///
/// This process itself must never join the job: a job member's children
/// inherit membership, and the updater launches the new installer in the
/// instant this process exits. A whole-process job killed that installer
/// before it ran, which silently broke auto-update on every version that
/// shipped one (v0.4.0–v26.7.0). Membership is opt-in per child via
/// [`watch_child`]; whatever a registered sidecar spawns still inherits
/// from its parent.
pub fn kill_children_with_us() {
    use windows::core::PCWSTR;
    use windows::Win32::System::JobObjects::{
        CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    unsafe {
        let Ok(job) = CreateJobObjectW(None, PCWSTR::null()) else {
            tracing::warn!("failed to create the child-process job object");
            return;
        };
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let set = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const core::ffi::c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        if set.is_err() {
            tracing::warn!("failed to arm the child-process job object");
            return;
        }
        let _ = JOB.set(job.0 as isize);
    }
}

/// The `--child-reaper` subprocess body. Nothing to do on Windows: the job
/// object covers orphan cleanup, so the flag is never passed and this never
/// runs. Present so the caller needs no `cfg` (`platform/mod.rs`).
pub fn run_reaper() {}

/// Job membership is assigned after spawn ([`watch_child`]); nothing
/// to do on the command.
pub fn prepare_spawn(_cmd: &mut std::process::Command) {}

/// [`prepare_spawn`] for tokio-spawned children; nothing per-spawn.
pub fn prepare_spawn_tokio(_cmd: &mut tokio::process::Command) {}

/// Put a spawned child into the job so it dies with us. The instants
/// between spawn and registration are unguarded, the same window macOS
/// accepts between spawn and the reaper pipe write. Failures degrade to
/// clean-quit-only cleanup, never fatal.
pub fn watch_child(pid: u32) {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::JobObjects::AssignProcessToJobObject;
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE};

    let Some(job) = job_handle() else { return };
    unsafe {
        let Ok(child) = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, false, pid) else {
            tracing::warn!(pid, "failed to open the child for job registration");
            return;
        };
        if AssignProcessToJobObject(job, child).is_err() {
            tracing::warn!(pid, "failed to put the child in the job object");
        }
        let _ = CloseHandle(child);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use windows::core::BOOL;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::JobObjects::IsProcessInJob;
    use windows::Win32::System::Threading::GetCurrentProcess;

    fn in_our_job(process: HANDLE) -> bool {
        let job = job_handle().expect("the job object exists");
        let mut result = BOOL::default();
        unsafe { IsProcessInJob(process, Some(job), &mut result) }.expect("IsProcessInJob");
        result.as_bool()
    }

    /// The regression shipped in v0.4.0–v26.7.0: the app itself sat in the
    /// job, so the updater's installer (spawned as the app exits) died
    /// with the job before it could run. The process must stay out; a
    /// watched child must be in.
    #[test]
    fn process_stays_out_of_the_job_and_watched_children_join_it() {
        kill_children_with_us();
        assert!(
            !in_our_job(unsafe { GetCurrentProcess() }),
            "the process itself must never join the job"
        );

        let mut cmd = std::process::Command::new("ping");
        cmd.args(["-n", "30", "127.0.0.1"])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        prepare_spawn(&mut cmd);
        let mut child = cmd.spawn().expect("spawn the dummy child");
        watch_child(child.id());

        let joined = in_our_job(HANDLE(child.as_raw_handle()));
        let _ = child.kill();
        let _ = child.wait();
        assert!(joined, "a watched child joins the job");
    }

    /// Manual probe for the half the in-process test can't reach: the job
    /// handle leaks until process death, and that death, however it comes,
    /// must kill the watched child. Run with
    /// `cargo test -p embral --lib watched_child_outlives -- --ignored --nocapture`,
    /// kill the test process externally (`taskkill /F /PID <printed pid>`),
    /// and confirm the printed ping pid is gone.
    #[test]
    #[ignore = "manual probe; kill this process externally and confirm the child dies"]
    fn watched_child_outlives_nothing() {
        kill_children_with_us();
        let mut cmd = std::process::Command::new("ping");
        cmd.args(["-n", "600", "127.0.0.1"])
            .creation_flags(0x0800_0000)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        prepare_spawn(&mut cmd);
        let child = cmd.spawn().expect("spawn the dummy child");
        watch_child(child.id());
        println!(
            "probe pid {} / child ping pid {} — kill the probe, check the ping",
            std::process::id(),
            child.id()
        );
        std::thread::sleep(std::time::Duration::from_secs(600));
    }
}
