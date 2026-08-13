//! Where Claude Desktop keeps its MCP config on this machine
//! ([integrations.md](../../../../docs/integrations.md)).
//!
//! The standalone (Squirrel) installer keeps it at `%APPDATA%\Claude`, but
//! the MSIX build (Microsoft Store / WinGet / the current official Windows
//! installer) runs in a virtualized filesystem and redirects it under the
//! package folder in `%LOCALAPPDATA%\Packages`, so a bare `%APPDATA%`
//! check misses every Store/WinGet install.

use std::path::{Path, PathBuf};

/// The MSIX build of Claude Desktop runs in a virtualized filesystem: its
/// `%APPDATA%\Claude` is redirected under this package folder in
/// `%LOCALAPPDATA%\Packages`. `pzs8sxrjxfjjc` is Anthropic's stable publisher
/// hash; the `Claude_*` glob in [`desktop_config_candidates`] covers a change.
const CLAUDE_MSIX_PACKAGE: &str = "Claude_pzs8sxrjxfjjc";

/// Every directory Claude Desktop might keep `claude_desktop_config.json` in,
/// most specific first: the standalone (Squirrel) install's `%APPDATA%\Claude`,
/// then the MSIX virtualized path (the known package, then any other `Claude_*`
/// package discovered under `%LOCALAPPDATA%\Packages`). Pure so the ordering is
/// unit-tested without touching disk; `package_dirs` is the folder listing.
fn desktop_config_candidates(
    roaming: Option<&Path>,
    local: Option<&Path>,
    package_dirs: &[String],
) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(roaming) = roaming {
        dirs.push(roaming.join("Claude"));
    }
    if let Some(local) = local {
        let virtualized = |pkg: &str| {
            local
                .join("Packages")
                .join(pkg)
                .join("LocalCache")
                .join("Roaming")
                .join("Claude")
        };
        dirs.push(virtualized(CLAUDE_MSIX_PACKAGE));
        for pkg in package_dirs {
            if pkg.starts_with("Claude_") && pkg != CLAUDE_MSIX_PACKAGE {
                dirs.push(virtualized(pkg));
            }
        }
    }
    dirs
}

/// Subdirectory names under `%LOCALAPPDATA%\Packages` (empty when it can't be
/// read), feeding the `Claude_*` glob without the pure candidate builder
/// touching disk.
fn local_package_dirs(local: Option<&Path>) -> Vec<String> {
    local
        .map(|l| l.join("Packages"))
        .and_then(|p| std::fs::read_dir(p).ok())
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// The best Claude Desktop config directory visible on disk: the first
/// candidate that exists, else the first candidate as a sensible default for
/// display. No `Get-AppxPackage` call, so it's cheap enough for the setup-info
/// path shown in the copy-paste fallback.
pub fn desktop_config_dir_on_disk() -> Option<PathBuf> {
    let (roaming, local) = (dirs::config_dir(), dirs::data_local_dir());
    let packages = local_package_dirs(local.as_deref());
    let candidates = desktop_config_candidates(roaming.as_deref(), local.as_deref(), &packages);
    candidates
        .iter()
        .find(|p| p.is_dir())
        .cloned()
        .or_else(|| candidates.into_iter().next())
}

/// The live Claude Desktop config directory and whether it's installed.
/// Prefers a config dir that exists (standalone or MSIX); only when the fast
/// disk checks come up empty does it ask `Get-AppxPackage` (slow, hence last)
/// and derive the dir from the package family, so a freshly-installed but
/// never-launched Desktop still registers. The path comes back even when
/// nothing is installed, so callers can still say where to look.
pub async fn resolve_desktop_config_dir() -> (Option<PathBuf>, bool) {
    let (roaming, local) = (dirs::config_dir(), dirs::data_local_dir());
    let packages = local_package_dirs(local.as_deref());
    let candidates = desktop_config_candidates(roaming.as_deref(), local.as_deref(), &packages);

    if let Some(existing) = candidates.iter().find(|p| p.is_dir()) {
        return (Some(existing.clone()), true);
    }
    if let (Some(family), Some(local)) = (appx_package_family().await, local.as_deref()) {
        let dir = local
            .join("Packages")
            .join(family)
            .join("LocalCache")
            .join("Roaming")
            .join("Claude");
        return (Some(dir), true);
    }
    (candidates.into_iter().next(), false)
}

/// `PackageFamilyName` of an installed Claude Desktop MSIX package, or `None`.
/// Windowless and time-bounded like the client CLIs.
async fn appx_package_family() -> Option<String> {
    let output = super::proc::bounded_output(
        Path::new("powershell.exe"),
        &[
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "(Get-AppxPackage -Name Claude | Select-Object -First 1).PackageFamilyName",
        ],
    )
    .await
    .ok()?;
    if !output.status.success() {
        return None;
    }
    let family = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!family.is_empty()).then_some(family)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_candidates_prefer_roaming_then_msix() {
        let roaming = PathBuf::from(r"C:\Roaming");
        let local = PathBuf::from(r"C:\Local");
        let packages = vec![
            "SomethingElse_abc".to_string(),
            "Claude_pzs8sxrjxfjjc".to_string(),
            "Claude_newerhash".to_string(),
        ];
        let got = desktop_config_candidates(Some(&roaming), Some(&local), &packages);

        // Standalone Roaming path is tried first.
        assert_eq!(got[0], roaming.join("Claude"));
        // Then the known MSIX package's virtualized config dir.
        assert_eq!(
            got[1],
            local
                .join("Packages")
                .join("Claude_pzs8sxrjxfjjc")
                .join("LocalCache")
                .join("Roaming")
                .join("Claude"),
        );
        // The glob picks up an unknown Claude_* package but never a non-Claude one.
        assert!(got
            .iter()
            .any(|p| p.to_string_lossy().contains("Claude_newerhash")));
        assert!(!got
            .iter()
            .any(|p| p.to_string_lossy().contains("SomethingElse")));
        // The known package is listed once, even though it's also in the listing.
        let known = got
            .iter()
            .filter(|p| p.to_string_lossy().contains("Claude_pzs8sxrjxfjjc"))
            .count();
        assert_eq!(known, 1);
    }

    #[test]
    fn desktop_candidates_offer_msix_without_roaming() {
        assert!(desktop_config_candidates(None, None, &[]).is_empty());

        let local = PathBuf::from(r"C:\Local");
        let got = desktop_config_candidates(None, Some(&local), &[]);
        // No Roaming base, but the known MSIX path is always offered.
        assert_eq!(got.len(), 1);
        assert!(got[0].to_string_lossy().contains("Claude_pzs8sxrjxfjjc"));
    }
}
