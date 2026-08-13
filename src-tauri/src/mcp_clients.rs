//! Registering the MCP server with the AI clients on this machine (Claude
//! Desktop through its JSON config, Claude Code and Codex through their
//! CLIs), plus the copy-paste setup info for everything else. Detection
//! reports disk/CLI truth, never UI state: the frontend refetches after every
//! action instead of assuming success ([integrations.md](../../docs/integrations.md)).
//! Where Claude Desktop keeps its config (and how CLIs resolve) is
//! per-OS; `crate::platform::{mcp_paths, find_cli}` own that.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// Where the MCP server binary lives plus ready-made client snippets for the
/// Settings → MCP page. In dev the workspace `target/` build is used; in a
/// bundled install the sidecar sits next to the app executable (bundling is
/// wired up in R6 release engineering).
#[derive(serde::Serialize)]
pub struct McpSetupInfo {
    pub path: String,
    pub exists: bool,
    pub claude_code_command: String,
    pub config_json: String,
    pub codex_command: String,
    pub codex_toml: String,
    pub claude_desktop_config_path: String,
}

#[derive(Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpClient {
    ClaudeDesktop,
    ClaudeCode,
    Codex,
}

#[derive(serde::Serialize)]
pub struct ClientStatus {
    pub installed: bool,
    pub registered: bool,
    /// The resolved CLI/config path, or why detection came up empty: the
    /// UI's status line.
    pub detail: String,
}

#[derive(serde::Serialize)]
pub struct McpClientsStatus {
    pub server_path: String,
    pub server_exists: bool,
    pub claude_desktop: ClientStatus,
    pub claude_code: ClientStatus,
    pub codex: ClientStatus,
}

// --- Paths and resolution ---

/// The server binary: bundled sidecar next to the app exe first, then the
/// workspace's release and debug builds (dev). The final fallback is the
/// sidecar path even when absent, so the UI can say what's missing.
/// Also the app's embedding worker: `search_index` spawns this same
/// binary in its `embed` mode.
pub(crate) fn server_binary() -> Result<(PathBuf, bool), String> {
    let exe = crate::platform::exe_name("embral-mcp");
    let exe_dir_path = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join(&exe)));
    let dev_release_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("target")
        .join("release")
        .join(&exe);
    let dev_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("target")
        .join("debug")
        .join(&exe);

    let path = [exe_dir_path.clone(), Some(dev_release_path), Some(dev_path)]
        .into_iter()
        .flatten()
        .find(|p| p.is_file())
        .or(exe_dir_path)
        .ok_or("could not resolve the embral-mcp path")?;
    let exists = path.is_file();
    Ok((path, exists))
}

/// Where an AppImage's registrations must point instead of the mount.
/// `Some` only when this process runs as an AppImage: the exe-dir path
/// lives under `/tmp/.mount_*`, renamed every launch and gone whenever
/// the app is closed; a client config holding it is broken by design.
/// The stable home is `~/.local/share/embral/bin` (the same per-user
/// root the logs use).
fn appimage_stable_dir() -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        if std::env::var_os("APPIMAGE").is_some_and(|v| !v.is_empty()) {
            return dirs::data_local_dir().map(|d| d.join("embral").join("bin"));
        }
    }
    None
}

/// Copy `src` over `dest` when `dest` is missing or its bytes differ;
/// true when a copy happened. `fs::copy` carries the mode bits, so the
/// copy stays executable on unix. Linux replaces running executables
/// without locks, so a client mid-session on the old copy keeps its
/// image and the next spawn gets the new one.
fn refresh_stable_copy(src: &Path, dest: &Path) -> std::io::Result<bool> {
    let same = match (std::fs::metadata(src), std::fs::metadata(dest)) {
        (Ok(s), Ok(d)) if s.len() == d.len() => std::fs::read(src)? == std::fs::read(dest)?,
        _ => false,
    };
    if same {
        return Ok(false);
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(src, dest)?;
    Ok(true)
}

/// The server path client-facing surfaces hand out: registration
/// writes, the status line, the copy-paste snippets. Normally the
/// sidecar next to the app exe; under an AppImage, a stable per-user
/// copy refreshed from the mount, because the mount path dies with the
/// process ([integrations.md]). A copy failure falls back to the
/// sidecar path: degraded (the old behavior), never newly broken. The
/// app's own embedding worker keeps spawning `server_binary()`; the
/// mount is valid for exactly as long as the app runs.
pub(crate) fn registered_server_binary() -> Result<(PathBuf, bool), String> {
    let (sidecar, exists) = server_binary()?;
    let Some(stable_dir) = appimage_stable_dir() else {
        return Ok((sidecar, exists));
    };
    let stable = stable_dir.join("embral-mcp");
    if exists {
        if let Err(e) = refresh_stable_copy(&sidecar, &stable) {
            tracing::warn!("could not refresh the stable mcp server copy: {e}");
            return Ok((sidecar, exists));
        }
    }
    let stable_exists = stable.is_file();
    Ok((stable, stable_exists))
}

/// The AppImage half of boot-time update hygiene: refresh the stable
/// copy so clients pick up a new build at their next spawn even if
/// Settings → MCP is never opened. A stale copy after an app update is
/// exactly the old-server-on-new-library case the schema guard refuses.
#[cfg(target_os = "linux")]
pub fn refresh_appimage_server_copy() {
    if appimage_stable_dir().is_none() {
        return;
    }
    match registered_server_binary() {
        Ok((path, true)) => tracing::info!("mcp server registration path: {}", path.display()),
        Ok((_, false)) => tracing::warn!("mcp server binary missing beside the app"),
        Err(e) => tracing::warn!("mcp server path did not resolve: {e}"),
    }
}

/// Update leftovers: the Windows installer renames a locked
/// `embral-mcp.exe` aside as `embral-mcp.exe.stale-N` instead of failing
/// the update ([release.md] §Installer hooks). Whatever it left behind is
/// removed here at the next app start; a file still locked (a client
/// that has not restarted, still running the old server) is skipped,
/// and a later boot gets it.
#[cfg(windows)]
pub fn sweep_stale_servers() {
    let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
    else {
        return;
    };
    let (deleted, skipped) = sweep_stale_in(&dir);
    if deleted + skipped > 0 {
        tracing::info!(deleted, skipped, "swept renamed mcp server binaries");
    }
}

#[cfg(windows)]
fn sweep_stale_in(dir: &Path) -> (usize, usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (0, 0);
    };
    let (mut deleted, mut skipped) = (0, 0);
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with("embral-mcp.exe.stale-") {
            continue;
        }
        match std::fs::remove_file(entry.path()) {
            Ok(()) => deleted += 1,
            Err(_) => skipped += 1,
        }
    }
    (deleted, skipped)
}

/// Canonicalize without Windows `\\?\` prefixes (which confuse copied configs).
fn dunce_simplify(path: &Path) -> String {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let s = canonical.to_string_lossy().to_string();
    s.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(s)
}

const DESKTOP_CONFIG_FILE: &str = "claude_desktop_config.json";

use crate::platform::mcp_paths::{desktop_config_dir_on_disk, resolve_desktop_config_dir};

/// Claude Code's user-scope registry (`claude mcp add --scope user` writes it).
fn claude_code_config() -> Option<PathBuf> {
    dirs::home_dir().map(|d| d.join(".claude.json"))
}

fn codex_config() -> Option<PathBuf> {
    dirs::home_dir().map(|d| d.join(".codex").join("config.toml"))
}

// --- Pure config-file logic (tested) ---

/// Set `mcpServers.embral` in a Claude-style JSON config, preserving every
/// other key byte-for-byte semantically. Refuses input it can't parse:
/// never clobber a config we couldn't read.
fn upsert_mcp_server(existing: &str, command: &str) -> Result<String, String> {
    let mut root: serde_json::Value = if existing.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(existing)
            .map_err(|e| format!("the existing config didn't parse as JSON ({e}) — not touching it"))?
    };
    let obj = root
        .as_object_mut()
        .ok_or("the existing config isn't a JSON object — not touching it")?;
    let servers = obj
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}));
    let servers = servers
        .as_object_mut()
        .ok_or("'mcpServers' isn't an object — not touching it")?;
    servers.insert(
        "embral".into(),
        serde_json::json!({ "command": command }),
    );
    serde_json::to_string_pretty(&root).map_err(|e| e.to_string())
}

/// Remove `mcpServers.embral`; `Ok(None)` when it wasn't there.
fn remove_mcp_server(existing: &str) -> Result<Option<String>, String> {
    let mut root: serde_json::Value = serde_json::from_str(existing)
        .map_err(|e| format!("the existing config didn't parse as JSON ({e}) — not touching it"))?;
    let removed = root
        .get_mut("mcpServers")
        .and_then(|s| s.as_object_mut())
        .and_then(|s| s.remove("embral"))
        .is_some();
    if !removed {
        return Ok(None);
    }
    serde_json::to_string_pretty(&root)
        .map(Some)
        .map_err(|e| e.to_string())
}

/// TOML literal strings need no backslash escaping and our paths hold no
/// single quotes.
fn codex_toml_snippet(server_path: &str) -> String {
    format!("[mcp_servers.embral]\ncommand = '{server_path}'\nargs = []")
}

/// Set `mcp_servers.embral` in `~/.codex/config.toml`: the direct-write
/// path for a machine running ChatGPT desktop without the `codex` CLI.
/// `toml_edit` keeps every other table, key, and comment as written; a
/// file that doesn't parse is refused, never clobbered.
fn upsert_codex_server(existing: &str, server_path: &str) -> Result<String, String> {
    let mut doc: toml_edit::DocumentMut = existing
        .parse()
        .map_err(|e| format!("the existing config didn't parse as TOML ({e}) — not touching it"))?;
    let servers = doc["mcp_servers"].or_insert(toml_edit::table());
    if let Some(t) = servers.as_table_mut() {
        // A bare `[mcp_servers]` header for a table that only holds
        // sub-tables is noise; let the entries carry the full names.
        t.set_implicit(true);
    }
    let mut entry = toml_edit::Table::new();
    entry["command"] = toml_edit::value(server_path);
    entry["args"] = toml_edit::value(toml_edit::Array::new());
    servers["embral"] = toml_edit::Item::Table(entry);
    Ok(doc.to_string())
}

/// Remove `mcp_servers.embral`; `Ok(None)` when it wasn't there.
fn remove_codex_server(existing: &str) -> Result<Option<String>, String> {
    let mut doc: toml_edit::DocumentMut = existing
        .parse()
        .map_err(|e| format!("the existing config didn't parse as TOML ({e}) — not touching it"))?;
    let removed = doc
        .get_mut("mcp_servers")
        .and_then(|s| s.as_table_mut())
        .and_then(|s| s.remove("embral"))
        .is_some();
    if !removed {
        return Ok(None);
    }
    Ok(Some(doc.to_string()))
}

/// Whether the shared OpenAI config home exists: what a ChatGPT desktop
/// install creates even when the `codex` CLI is not on PATH. The desktop
/// app, the CLI, and the IDE extension read the same
/// `~/.codex/config.toml` ([integrations.md]).
fn codex_home_exists() -> bool {
    dirs::home_dir()
        .map(|d| d.join(".codex").is_dir())
        .unwrap_or(false)
}

fn json_registered(path: Option<&PathBuf>) -> bool {
    let Some(path) = path else { return false };
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .map(|v| v["mcpServers"]["embral"].is_object())
        .unwrap_or(false)
}

fn codex_registered() -> bool {
    codex_config()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| s.lines().any(|l| l.trim() == "[mcp_servers.embral]"))
        .unwrap_or(false)
}

// --- Running client CLIs ---

use crate::platform::find_cli;

/// Run a resolved CLI without flashing a console window, bounded so a hung
/// client can't hang the settings page. Handing std/tokio the explicit
/// `.cmd` path is safe: the runtime wraps cmd.exe itself with correct
/// quoting, and our args are fixed strings plus a path.
async fn run_cli(exe: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    let mut cmd = tokio::process::Command::new(exe);
    cmd.args(args).stdin(std::process::Stdio::null());
    crate::platform::hide_console_tokio(&mut cmd);
    tokio::time::timeout(Duration::from_secs(15), cmd.output())
        .await
        .map_err(|_| format!("{} timed out after 15 s", exe.display()))?
        .map_err(|e| format!("{}: {e}", exe.display()))
}

/// The last chunk of a failed CLI's output: enough to see why, short
/// enough for an inline error line.
fn output_tail(output: &std::process::Output) -> String {
    let mut text = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if text.is_empty() {
        text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    }
    if text.is_empty() {
        text = format!("exit status {}", output.status);
    }
    match text.char_indices().rev().nth(399) {
        Some((i, _)) => text[i..].to_string(),
        None => text,
    }
}

async fn run_registration(exe: &Path, args: &[&str], success: &str) -> Result<String, String> {
    let output = run_cli(exe, args).await?;
    if output.status.success() {
        Ok(success.to_string())
    } else {
        Err(output_tail(&output))
    }
}

// --- Commands ---

#[tauri::command]
pub async fn mcp_setup_info() -> Result<McpSetupInfo, String> {
    let (path, exists) = registered_server_binary()?;
    let display = dunce_simplify(&path);
    let escaped = display.replace('\\', "\\\\");
    Ok(McpSetupInfo {
        claude_code_command: format!("claude mcp add --scope user embral -- \"{display}\""),
        config_json: format!(
            "{{\n  \"mcpServers\": {{\n    \"embral\": {{\n      \"command\": \"{escaped}\"\n    }}\n  }}\n}}"
        ),
        codex_command: format!("codex mcp add embral -- \"{display}\""),
        codex_toml: codex_toml_snippet(&display),
        claude_desktop_config_path: desktop_config_dir_on_disk()
            .map(|d| d.join(DESKTOP_CONFIG_FILE).to_string_lossy().to_string())
            .unwrap_or_default(),
        path: display,
        exists,
    })
}

#[tauri::command]
pub async fn mcp_clients_status() -> Result<McpClientsStatus, String> {
    let (server_path, server_exists) = registered_server_binary()?;

    let (desktop_dir, desktop_installed) = resolve_desktop_config_dir().await;
    let desktop_config = desktop_dir.map(|d| d.join(DESKTOP_CONFIG_FILE));
    let claude_desktop = ClientStatus {
        installed: desktop_installed,
        registered: json_registered(desktop_config.as_ref()),
        detail: if desktop_installed {
            desktop_config
                .as_ref()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default()
        } else {
            "Claude Desktop was not found on this machine".into()
        },
    };

    let (claude_cli, codex_cli) = tokio::join!(find_cli("claude"), find_cli("codex"));
    let claude_code = ClientStatus {
        installed: claude_cli.is_some(),
        registered: json_registered(claude_code_config().as_ref()),
        detail: claude_cli
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "the claude CLI was not found on PATH".into()),
    };
    // The card covers every surface of the unified OpenAI app (ChatGPT
    // desktop, the Codex CLI, the IDE extension), which share one config
    // file. The CLI is one signal; the config home the desktop app
    // creates is the other.
    let codex = ClientStatus {
        installed: codex_cli.is_some() || codex_home_exists(),
        registered: codex_registered(),
        detail: codex_cli
            .map(|p| p.to_string_lossy().to_string())
            .or_else(|| {
                codex_home_exists()
                    .then(|| codex_config())
                    .flatten()
                    .map(|p| p.to_string_lossy().to_string())
            })
            .unwrap_or_else(|| "neither the codex CLI nor ~/.codex was found".into()),
    };

    Ok(McpClientsStatus {
        server_path: dunce_simplify(&server_path),
        server_exists,
        claude_desktop,
        claude_code,
        codex,
    })
}

/// The client's telemetry name, from the vocabulary's closed set.
fn client_label(client: McpClient) -> &'static str {
    match client {
        McpClient::ClaudeDesktop => "claude_desktop",
        McpClient::ClaudeCode => "claude_code",
        McpClient::Codex => "codex",
    }
}

#[tauri::command]
pub async fn mcp_register(
    state: tauri::State<'_, crate::AppState>,
    client: McpClient,
) -> Result<String, String> {
    let result = mcp_register_inner(client).await;
    match &result {
        Ok(_) => crate::telemetry::track(
            &state,
            "mcp_registered",
            serde_json::json!({ "client": client_label(client) }),
        ),
        Err(_) => crate::telemetry::track(
            &state,
            "error",
            serde_json::json!({ "category": "mcp_register_failed" }),
        ),
    }
    result
}

async fn mcp_register_inner(client: McpClient) -> Result<String, String> {
    let (server_path, exists) = registered_server_binary()?;
    if !exists {
        return Err(format!(
            "A part of embral this feature needs is missing (expected at {}) — reinstalling the app should fix this",
            dunce_simplify(&server_path)
        ));
    }
    let display = dunce_simplify(&server_path);

    match client {
        McpClient::ClaudeDesktop => {
            let (dir, installed) = resolve_desktop_config_dir().await;
            let dir = dir.ok_or("no config directory on this system")?;
            if !installed {
                return Err("Claude Desktop doesn't appear to be installed (no Claude config folder found)".into());
            }
            // create_dir_all covers an installed-but-never-launched MSIX
            // package, where Get-AppxPackage found it before the folder existed.
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let config = dir.join(DESKTOP_CONFIG_FILE);
            let existing = std::fs::read_to_string(&config).unwrap_or_default();
            let updated = upsert_mcp_server(&existing, &display)?;
            std::fs::write(&config, updated).map_err(|e| e.to_string())?;
            Ok("Registered — restart Claude Desktop to pick up embral".into())
        }
        McpClient::ClaudeCode => {
            let cli = find_cli("claude")
                .await
                .ok_or("the claude CLI was not found on PATH")?;
            // remove-then-add: `add` errors on an existing name, and this
            // also refreshes the path after a binary move.
            let _ = run_cli(&cli, &["mcp", "remove", "--scope", "user", "embral"]).await;
            run_registration(
                &cli,
                &["mcp", "add", "--scope", "user", "embral", "--", &display],
                "Registered with Claude Code for every project",
            )
            .await
        }
        McpClient::Codex => {
            // The vendor's own tool when it's here; the shared config file
            // directly when only ChatGPT desktop is installed (the app,
            // the CLI, and the IDE extension all read it).
            match find_cli("codex").await {
                Some(cli) => {
                    let _ = run_cli(&cli, &["mcp", "remove", "embral"]).await;
                    run_registration(
                        &cli,
                        &["mcp", "add", "embral", "--", &display],
                        "Registered with ChatGPT and Codex",
                    )
                    .await
                }
                None => {
                    let config = codex_config().ok_or("no home directory on this system")?;
                    if let Some(dir) = config.parent() {
                        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
                    }
                    let existing = std::fs::read_to_string(&config).unwrap_or_default();
                    let updated = upsert_codex_server(&existing, &display)?;
                    std::fs::write(&config, updated).map_err(|e| e.to_string())?;
                    Ok("Registered — restart ChatGPT to pick up embral".into())
                }
            }
        }
    }
}

#[tauri::command]
pub async fn mcp_unregister(
    state: tauri::State<'_, crate::AppState>,
    client: McpClient,
) -> Result<String, String> {
    let result = mcp_unregister_inner(client).await;
    if result.is_ok() {
        crate::telemetry::track(
            &state,
            "mcp_unregistered",
            serde_json::json!({ "client": client_label(client) }),
        );
    }
    result
}

async fn mcp_unregister_inner(client: McpClient) -> Result<String, String> {
    match client {
        McpClient::ClaudeDesktop => {
            let config = desktop_config_dir_on_disk()
                .map(|d| d.join(DESKTOP_CONFIG_FILE))
                .ok_or("no config directory on this system")?;
            let existing = std::fs::read_to_string(&config)
                .map_err(|_| "no Claude Desktop config file to edit")?;
            match remove_mcp_server(&existing)? {
                Some(updated) => {
                    std::fs::write(&config, updated).map_err(|e| e.to_string())?;
                    Ok("Removed — restart Claude Desktop to apply".into())
                }
                None => Ok("embral wasn't registered with Claude Desktop".into()),
            }
        }
        McpClient::ClaudeCode => {
            let cli = find_cli("claude")
                .await
                .ok_or("the claude CLI was not found on PATH")?;
            run_registration(
                &cli,
                &["mcp", "remove", "--scope", "user", "embral"],
                "Removed from Claude Code",
            )
            .await
        }
        McpClient::Codex => match find_cli("codex").await {
            Some(cli) => {
                run_registration(
                    &cli,
                    &["mcp", "remove", "embral"],
                    "Removed from ChatGPT and Codex",
                )
                .await
            }
            None => {
                let config = codex_config().ok_or("no home directory on this system")?;
                let existing = std::fs::read_to_string(&config)
                    .map_err(|_| "no Codex config file to edit")?;
                match remove_codex_server(&existing)? {
                    Some(updated) => {
                        std::fs::write(&config, updated).map_err(|e| e.to_string())?;
                        Ok("Removed — restart ChatGPT to apply".into())
                    }
                    None => Ok("embral wasn't registered with ChatGPT or Codex".into()),
                }
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_preserves_everything_else() {
        let existing = r#"{
            "theme": "dark",
            "mcpServers": {
                "other": { "command": "other.exe", "args": ["-x"] }
            },
            "unknownFuture": [1, 2]
        }"#;
        let updated = upsert_mcp_server(existing, r"C:\apps\embral-mcp.exe").unwrap();
        let v: serde_json::Value = serde_json::from_str(&updated).unwrap();
        assert_eq!(v["theme"], "dark");
        assert_eq!(v["unknownFuture"][1], 2);
        assert_eq!(v["mcpServers"]["other"]["args"][0], "-x");
        assert_eq!(v["mcpServers"]["embral"]["command"], r"C:\apps\embral-mcp.exe");
    }

    /// Linux port check, run manually because it reads the developer's real
    /// `~/.claude.json`:
    /// `cargo test -p embral --lib linux_registration_probe -- --ignored --nocapture`.
    ///
    /// Registration on Linux has three moving parts the unit tests above
    /// cannot cover, because they all touch the machine: the sidecar has to
    /// resolve, `claude` has to be findable on a PATH a desktop launch
    /// inherits, and the merge has to survive a real config (Claude Code's
    /// own file is tens of kilobytes of nested project state, not the tidy
    /// fixtures above). Nothing is written; the merge runs in memory and every
    /// top-level key is checked to survive it.
    #[test]
    #[ignore = "manual probe; reads the real ~/.claude.json (never writes)"]
    fn linux_registration_probe() {
        let (bin, exists) = server_binary().expect("sidecar path resolves");
        eprintln!("sidecar: {} (present: {exists})", bin.display());
        assert!(exists, "stage the sidecar first: node scripts/prepare-sidecar.mjs --debug");
        assert_eq!(bin.file_name().unwrap(), "embral-mcp", "no .exe suffix on Linux");

        let claude = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(crate::platform::find_cli("claude"));
        eprintln!("claude: {claude:?}");
        assert!(claude.is_some(), "find_cli must resolve a real claude");

        let path = claude_code_config().expect("home dir");
        let before = std::fs::read_to_string(&path).expect("read the real config");
        eprintln!("config: {} ({} bytes)", path.display(), before.len());

        let after = upsert_mcp_server(&before, bin.to_str().unwrap()).expect("merge");
        let a: serde_json::Value = serde_json::from_str(&before).unwrap();
        let b: serde_json::Value = serde_json::from_str(&after).unwrap();

        // Every pre-existing top-level key survives, byte-identical.
        for (k, v) in a.as_object().unwrap() {
            if k == "mcpServers" {
                continue;
            }
            assert_eq!(b.get(k), Some(v), "top-level key {k:?} was altered");
        }
        // Any MCP server already registered is left alone.
        if let Some(existing) = a.get("mcpServers").and_then(|m| m.as_object()) {
            for (k, v) in existing {
                if k != "embral" {
                    assert_eq!(b["mcpServers"].get(k), Some(v), "clobbered server {k:?}");
                }
            }
        }
        assert_eq!(b["mcpServers"]["embral"]["command"], bin.to_str().unwrap());
        eprintln!(
            "merge preserved {} top-level keys and added mcpServers.embral",
            a.as_object().unwrap().len()
        );
    }

    #[test]
    fn upsert_handles_empty_and_updates_in_place() {
        let fresh = upsert_mcp_server("", "a.exe").unwrap();
        let v: serde_json::Value = serde_json::from_str(&fresh).unwrap();
        assert_eq!(v["mcpServers"]["embral"]["command"], "a.exe");

        let moved = upsert_mcp_server(&fresh, "b.exe").unwrap();
        let v: serde_json::Value = serde_json::from_str(&moved).unwrap();
        assert_eq!(v["mcpServers"]["embral"]["command"], "b.exe");
    }

    #[test]
    fn malformed_config_is_refused_untouched() {
        assert!(upsert_mcp_server("{ not json", "a.exe").is_err());
        assert!(upsert_mcp_server("[1,2,3]", "a.exe").is_err());
        assert!(remove_mcp_server("{ not json").is_err());
    }

    #[test]
    fn remove_takes_only_embral() {
        let existing = r#"{"mcpServers": {"embral": {"command": "e.exe"}, "other": {"command": "o.exe"}}}"#;
        let updated = remove_mcp_server(existing).unwrap().unwrap();
        let v: serde_json::Value = serde_json::from_str(&updated).unwrap();
        assert!(v["mcpServers"]["embral"].is_null());
        assert_eq!(v["mcpServers"]["other"]["command"], "o.exe");

        assert!(remove_mcp_server(r#"{"mcpServers": {}}"#).unwrap().is_none());
        assert!(remove_mcp_server("{}").unwrap().is_none());
    }

    #[test]
    fn codex_snippet_keeps_windows_paths_literal() {
        let toml = codex_toml_snippet(r"C:\Program Files\embral\embral-mcp.exe");
        assert!(toml.contains("[mcp_servers.embral]"));
        assert!(toml.contains(r"command = 'C:\Program Files\embral\embral-mcp.exe'"));
    }

    #[test]
    fn codex_upsert_creates_the_table_from_nothing() {
        let out = upsert_codex_server("", r"C:\bin\embral-mcp.exe").unwrap();
        assert!(out.contains("[mcp_servers.embral]"), "{out}");
        assert!(out.contains(r#"command = "C:\\bin\\embral-mcp.exe""#) || out.contains(r"C:\bin\embral-mcp.exe"), "{out}");
        assert!(out.contains("args = []"), "{out}");
        // What the registered-state probe reads is what the write produced.
        assert!(out.lines().any(|l| l.trim() == "[mcp_servers.embral]"));
    }

    #[test]
    fn codex_upsert_preserves_everything_else_verbatim() {
        let existing = "# my settings\nmodel = \"o4\"\n\n[mcp_servers.other]\ncommand = \"x\"\n\n[profiles.work]\nfast = true\n";
        let out = upsert_codex_server(existing, "/usr/lib/embral-mcp").unwrap();
        assert!(out.contains("# my settings"));
        assert!(out.contains("model = \"o4\""));
        assert!(out.contains("[mcp_servers.other]\ncommand = \"x\""));
        assert!(out.contains("[profiles.work]\nfast = true"));
        assert!(out.contains("[mcp_servers.embral]"));

        // Updating in place does not duplicate the table.
        let again = upsert_codex_server(&out, "/new/path").unwrap();
        assert_eq!(again.matches("[mcp_servers.embral]").count(), 1);
        assert!(again.contains("/new/path"));
        assert!(!again.contains("/usr/lib/embral-mcp"));
    }

    #[test]
    fn codex_upsert_refuses_a_file_it_cannot_parse() {
        let err = upsert_codex_server("model = [broken", "/x").unwrap_err();
        assert!(err.contains("not touching it"), "{err}");
    }

    #[test]
    fn codex_remove_takes_only_embral() {
        let existing = "[mcp_servers.embral]\ncommand = \"e\"\n\n[mcp_servers.other]\ncommand = \"o\"\n";
        let out = remove_codex_server(existing).unwrap().unwrap();
        assert!(!out.contains("embral"), "{out}");
        assert!(out.contains("[mcp_servers.other]"));

        assert!(remove_codex_server("model = \"o4\"\n").unwrap().is_none());
        assert!(remove_codex_server("").unwrap().is_none());
        assert!(remove_codex_server("model = [broken").is_err());
    }

    #[test]
    fn the_stable_copy_refreshes_only_when_bytes_differ() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.bin");
        // The dest's parent does not exist yet; the refresh creates it.
        let dest = dir.path().join("bin").join("embral-mcp");
        std::fs::write(&src, b"build one").unwrap();

        assert!(refresh_stable_copy(&src, &dest).unwrap(), "missing dest copies");
        assert_eq!(std::fs::read(&dest).unwrap(), b"build one");
        assert!(!refresh_stable_copy(&src, &dest).unwrap(), "identical bytes skip");

        // Same length, different bytes: the length early-out must not
        // skip the byte compare.
        std::fs::write(&src, b"build two").unwrap();
        assert!(refresh_stable_copy(&src, &dest).unwrap());
        assert_eq!(std::fs::read(&dest).unwrap(), b"build two");

        // Different length.
        std::fs::write(&src, b"a longer third build").unwrap();
        assert!(refresh_stable_copy(&src, &dest).unwrap());
        assert_eq!(std::fs::read(&dest).unwrap(), b"a longer third build");
    }

    #[cfg(windows)]
    #[test]
    fn the_sweep_takes_only_stale_server_binaries() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "embral-mcp.exe.stale-0",
            "embral-mcp.exe.stale-3",
            "embral-mcp.exe",
            "unrelated.txt",
        ] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        assert_eq!(sweep_stale_in(dir.path()), (2, 0));
        assert!(dir.path().join("embral-mcp.exe").is_file());
        assert!(dir.path().join("unrelated.txt").is_file());
        assert!(!dir.path().join("embral-mcp.exe.stale-0").exists());
    }

    #[cfg(windows)]
    #[test]
    fn a_locked_stale_file_is_skipped_not_fatal() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("embral-mcp.exe.stale-0"), b"x").unwrap();
        std::fs::write(dir.path().join("embral-mcp.exe.stale-1"), b"x").unwrap();
        // Hold stale-0 the way a running server holds its image: no
        // sharing at all.
        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(dir.path().join("embral-mcp.exe.stale-0"))
            .unwrap();
        assert_eq!(sweep_stale_in(dir.path()), (1, 1));
        assert!(dir.path().join("embral-mcp.exe.stale-0").is_file());
        drop(held);
        // The next boot's pass gets what the first one could not.
        assert_eq!(sweep_stale_in(dir.path()), (1, 0));
    }
}
