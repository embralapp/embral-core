//! Where Claude Desktop keeps its MCP config
//! ([integrations.md](../../../../docs/integrations.md)).
//!
//! One location on macOS (`~/Library/Application Support/Claude`), no
//! MSIX-style virtualization to resolve. "Installed but never launched"
//! is covered by the app bundle check, so registration can create the
//! config folder the way the Windows path does.

use std::path::PathBuf;

fn config_dir() -> Option<PathBuf> {
    // dirs::config_dir() = ~/Library/Application Support
    dirs::config_dir().map(|d| d.join("Claude"))
}

/// The best Claude Desktop config directory visible on disk (the single
/// candidate, returned even when absent, so callers can say where to
/// look).
pub fn desktop_config_dir_on_disk() -> Option<PathBuf> {
    config_dir()
}

/// The live Claude Desktop config directory and whether it's installed:
/// the config folder exists (it has run), or the app bundle is present
/// (installed, never launched; registration creates the folder).
pub async fn resolve_desktop_config_dir() -> (Option<PathBuf>, bool) {
    let dir = config_dir();
    let has_config = dir.as_deref().map(|d| d.is_dir()).unwrap_or(false);
    let has_bundle = std::path::Path::new("/Applications/Claude.app").exists()
        || dirs::home_dir()
            .map(|h| h.join("Applications/Claude.app").exists())
            .unwrap_or(false);
    (dir, has_config || has_bundle)
}
