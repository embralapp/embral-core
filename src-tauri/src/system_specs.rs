//! What this machine can handle (RAM, cores, and free disk), so onboarding
//! can recommend a model bundle instead of asking the user to know their
//! hardware ([shell.md](../../docs/shell.md),
//! [transcription.md](../../docs/transcription.md)). The mapping from specs
//! to models lives in the frontend (`onboarding/recommend.ts`); this command
//! only reports what is true.

use std::path::Path;

#[derive(Debug, Clone, serde::Serialize)]
pub struct SystemSpecs {
    pub total_ram_bytes: u64,
    pub logical_cores: u32,
    /// Free space on the drive holding the models dir; 0 = unknown (the
    /// frontend skips the disk check rather than warning on a guess).
    pub free_disk_bytes: u64,
}

/// Free bytes on the disk whose mount point is the longest prefix of `path`.
/// Pure over the (mount, free) list so the matching is unit-testable.
fn free_disk_for(path: &Path, disks: &[(std::path::PathBuf, u64)]) -> u64 {
    disks
        .iter()
        .filter(|(mount, _)| path.starts_with(mount))
        .max_by_key(|(mount, _)| mount.as_os_str().len())
        .map(|(_, free)| *free)
        .unwrap_or(0)
}

#[tauri::command]
pub fn system_specs() -> SystemSpecs {
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();

    let disks = sysinfo::Disks::new_with_refreshed_list();
    let disk_list: Vec<(std::path::PathBuf, u64)> = disks
        .iter()
        .map(|d| (d.mount_point().to_path_buf(), d.available_space()))
        .collect();
    let models_dir = embral_engine::catalog::models_root();

    SystemSpecs {
        total_ram_bytes: sys.total_memory(),
        logical_cores: std::thread::available_parallelism()
            .map(|n| n.get() as u32)
            .unwrap_or(1),
        free_disk_bytes: free_disk_for(&models_dir, &disk_list),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn specs_report_real_hardware() {
        let specs = system_specs();
        assert!(specs.total_ram_bytes > 0);
        assert!(specs.logical_cores > 0);
        // free_disk_bytes may legitimately be 0 (unknown) on exotic mounts,
        // so it carries no assertion.
    }

    #[test]
    fn disk_matching_takes_the_longest_mount_prefix() {
        // Forward-slash paths parse component-wise on every OS; the
        // Windows-literal `C:\...` form only splits on Windows and made
        // this test fail on macOS.
        let disks = vec![
            (PathBuf::from("/"), 100),
            (PathBuf::from("/data"), 50),
            (PathBuf::from("/other"), 999),
        ];
        let models = PathBuf::from("/data/someone/embral/models");
        assert_eq!(free_disk_for(&models, &disks), 50);
        assert_eq!(free_disk_for(Path::new("relative/elsewhere"), &disks), 0);
    }
}
