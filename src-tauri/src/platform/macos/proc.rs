//! Process-spawning helpers. No console windows exist to suppress on
//! macOS, and executables carry no suffix; CLI resolution is a $PATH walk
//! padded with the homes CLIs actually live in: a bundled app launched
//! from Finder inherits a minimal PATH that misses Homebrew and
//! version-manager shims.

use std::path::{Path, PathBuf};

/// Keep a child from flashing a console window; nothing to do here.
pub fn hide_console(_cmd: &mut std::process::Command) {}

/// [`hide_console`] for tokio-spawned children; nothing to do here.
pub fn hide_console_tokio(_cmd: &mut tokio::process::Command) {}

/// The platform spelling of an executable name (no suffix).
pub fn exe_name(base: &str) -> String {
    base.to_string()
}

/// Resolve a CLI: every $PATH entry, then the usual install homes Finder
/// launches don't see. First executable hit wins.
pub async fn find_cli(name: &str) -> Option<PathBuf> {
    let path_dirs = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .unwrap_or_default();
    let extras = ["/opt/homebrew/bin", "/usr/local/bin"]
        .into_iter()
        .map(PathBuf::from)
        .chain(dirs::home_dir().into_iter().flat_map(|h| {
            [".local/bin", ".bun/bin", ".volta/bin", ".npm-global/bin"]
                .into_iter()
                .map(move |d| h.join(d))
        }));
    path_dirs
        .into_iter()
        .chain(extras)
        .map(|dir| dir.join(name))
        .find(|p| is_executable(p))
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn finds_a_universal_binary_on_path() {
        // `ls` lives in /bin on every macOS and /bin is always on PATH.
        let found = super::find_cli("ls").await.expect("ls on PATH");
        assert!(found.ends_with("ls"));
        assert!(super::find_cli("no-such-cli-xyzzy").await.is_none());
    }
}
