//! Where Claude Desktop keeps its MCP config
//! ([integrations.md](../../../../docs/integrations.md)).
//!
//! There is no Claude Desktop build for Linux, so there is no directory to
//! find and no installation to detect. The stub rule (`platform/mod.rs`):
//! return the inert value and let the caller degrade. Registration's UI
//! already handles "not installed" for the Windows/macOS case where the app
//! is absent, and reports the same thing here.
//!
//! What is not in this file: `~/.claude.json` (Claude Code) and the
//! codex config path are both plain `$HOME` paths resolved above the
//! platform layer, so those two clients register on Linux exactly as they
//! do elsewhere.

use std::path::PathBuf;

/// The best Claude Desktop config directory visible on disk: none, on a
/// platform Claude Desktop does not ship for.
pub fn desktop_config_dir_on_disk() -> Option<PathBuf> {
    None
}

/// The live Claude Desktop config directory and whether it's installed:
/// never, here.
pub async fn resolve_desktop_config_dir() -> (Option<PathBuf>, bool) {
    (None, false)
}
