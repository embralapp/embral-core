// Packs the MCP server into an MCP Bundle (dist/embral.mcpb) for Claude
// Desktop's Extensions page. The manifest is generated here so it can't go
// stale against the crate: the version comes from Cargo.toml, and the tools
// list mirrors crates/embral-mcp/src/server.rs — the source of truth.
import { execSync } from "node:child_process";
import {
  copyFileSync,
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";

execSync("cargo build --release -p embral-mcp", { stdio: "inherit" });

const cargoToml = readFileSync("crates/embral-mcp/Cargo.toml", "utf8");
const version = /^version\s*=\s*"([^"]+)"/m.exec(cargoToml)[1];

rmSync("dist/mcpb", { recursive: true, force: true });
mkdirSync("dist/mcpb/server", { recursive: true });
copyFileSync("target/release/embral-mcp.exe", "dist/mcpb/server/embral-mcp.exe");

const manifest = {
  manifest_version: "0.3",
  name: "embral",
  display_name: "Embral",
  version,
  description: "Read-only access to your local Embral meeting notes and transcripts.",
  author: { name: "Embral" },
  server: {
    type: "binary",
    entry_point: "server/embral-mcp.exe",
    mcp_config: {
      command: "${__dirname}/server/embral-mcp.exe",
      args: [],
      env: {
        EMBRAL_STORAGE_DIR: "${user_config.storage_dir}",
      },
    },
  },
  // Display metadata only — the schemas the client actually sees come from
  // the server itself (server.rs).
  tools: [
    {
      name: "get_storage_status",
      description: "Where the library lives and whether it is readable and searchable.",
    },
    {
      name: "search_meetings",
      description: "Hybrid keyword+semantic search over transcripts, notes, and summaries.",
    },
    {
      name: "search_dictations",
      description: "Search the dictation history (personal voice notes).",
    },
    {
      name: "get_passage_context",
      description: "Expand a search hit into its surrounding minutes or neighbors.",
    },
    {
      name: "get_meeting",
      description: "One meeting: metadata, attendees vs speakers, summary, user notes.",
    },
    {
      name: "get_transcript",
      description: "A meeting's transcript, whole or a time window.",
    },
    {
      name: "list_meetings",
      description: "List meetings, newest first, with since/participant filters.",
    },
    {
      name: "get_meeting_image",
      description: "One pasted image from a meeting, downscaled, with its OCR text.",
    },
  ],
  compatibility: {
    claude_desktop: ">=0.10.0",
    platforms: ["win32"],
  },
  user_config: {
    storage_dir: {
      type: "directory",
      title: "Embral storage folder",
      description:
        "Your embral library. Leave empty to use the app's own configuration.",
      default: "${HOME}/embral",
      required: false,
    },
  },
};

writeFileSync("dist/mcpb/manifest.json", JSON.stringify(manifest, null, 2) + "\n");
execSync("npx mcpb pack dist/mcpb dist/embral.mcpb", { stdio: "inherit" });
console.log("packed dist/embral.mcpb");
