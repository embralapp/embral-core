// Builds the embral-mcp server and stages it where Tauri's externalBin
// bundling expects sidecars (`bundle.externalBin` in tauri.conf.json).
// Runs automatically as part of beforeBuildCommand, so every `tauri build`
// ships the MCP server next to the app exe. Sidecar names carry the host
// target triple (Tauri resolves `binaries/embral-mcp-<triple>[.exe]` per
// platform).
//
// `--debug` builds the dev profile instead, for the CI jobs that only
// check and test. They stage a sidecar at all because tauri-build
// validates externalBin at compile time (nothing runs it and nothing
// ships it), and a release build there compiles ort, tokenizers and
// bundled SQLite a second time, into a target/ the rest of the job never
// touches. In the dev profile that work is shared with `cargo test`.
import { execSync } from "node:child_process";
import { mkdirSync, copyFileSync } from "node:fs";

const debug = process.argv.includes("--debug");
const profile = debug ? "debug" : "release";
const triple = execSync("rustc --print host-tuple", { encoding: "utf8" }).trim();
const exe = process.platform === "win32" ? ".exe" : "";

execSync(`cargo build ${debug ? "" : "--release "}-p embral-mcp`, {
  stdio: "inherit",
});
mkdirSync("src-tauri/binaries", { recursive: true });
copyFileSync(
  `target/${profile}/embral-mcp${exe}`,
  `src-tauri/binaries/embral-mcp-${triple}${exe}`,
);
console.log(`embral-mcp sidecar staged for ${triple} (${profile})`);
