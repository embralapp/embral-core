// Kill embral-mcp processes running out of this repo's target directory.
//
// MCP clients (Claude Desktop, Claude Code) keep the dev sidecar alive
// between builds, and Windows refuses to overwrite a running exe, so
// tauri-build's sidecar copy dies with "Access is denied" until they're
// gone. Clients respawn their server on the next call, so this is free.
// The app itself is deliberately left alone: it may be mid-recording.
import { spawnSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

const target = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
  "target",
);

if (process.platform === "win32") {
  spawnSync(
    "powershell",
    [
      "-NoProfile",
      "-Command",
      `Get-Process embral-mcp -ErrorAction SilentlyContinue | Where-Object { $_.Path -like '${target}\\*' } | Stop-Process -Force`,
    ],
    { stdio: "ignore" },
  );
} else {
  spawnSync("pkill", ["-f", `${target}/.*embral-mcp`], { stdio: "ignore" });
}
