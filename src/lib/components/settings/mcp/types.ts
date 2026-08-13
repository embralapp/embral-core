// Mirrors of src-tauri/src/mcp_clients.rs: the setup snippets and the
// per-client detection results.

export interface McpSetupInfo {
    path: string;
    exists: boolean;
    claude_code_command: string;
    config_json: string;
    codex_command: string;
    codex_toml: string;
    claude_desktop_config_path: string;
}

export interface ClientStatus {
    installed: boolean;
    registered: boolean;
    detail: string;
}

export interface McpClientsStatus {
    server_path: string;
    server_exists: boolean;
    claude_desktop: ClientStatus;
    claude_code: ClientStatus;
    codex: ClientStatus;
}

export type McpClientId = "claude_desktop" | "claude_code" | "codex";

export type McpAction = (kind: "register" | "unregister") => Promise<string>;
