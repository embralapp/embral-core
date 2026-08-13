<script lang="ts">
    import { onMount } from "svelte";
    import { invoke } from "@tauri-apps/api/core";
    import ClientCard from "./mcp/ClientCard.svelte";
    import CodeSnippet from "./mcp/CodeSnippet.svelte";
    import CopyParts from "$lib/components/CopyParts.svelte";
    import type {
        McpAction,
        McpClientId,
        McpClientsStatus,
        McpSetupInfo,
    } from "./mcp/types";
    import { appState } from "$lib/stores/app-state.svelte";
    import { modelsStore } from "$lib/stores/models.svelte";
    import { copy } from "$lib/copy";

    const t = $derived(copy.settings.mcp);

    let info = $state<McpSetupInfo | null>(null);
    let status = $state<McpClientsStatus | null>(null);

    // Connected assistants search by meaning once the semantic model is
    // downloaded; until then this is the one nudge toward it.
    let semanticMissing = $derived(
        modelsStore.statuses.some((m) => m.kind === "embedding" && !m.present),
    );

    async function refresh() {
        try {
            status = await invoke<McpClientsStatus>("mcp_clients_status");
        } catch (e) {
            console.error("mcp_clients_status failed:", e);
        }
    }

    onMount(async () => {
        void modelsStore.refresh();
        try {
            [info] = await Promise.all([
                invoke<McpSetupInfo>("mcp_setup_info"),
                refresh(),
            ]);
        } catch (e) {
            console.error("mcp_setup_info failed:", e);
        }
    });

    // Every action ends by re-reading disk/CLI truth; "Registered" is
    // never assumed from a button click.
    function act(client: McpClientId): McpAction {
        return async (kind) => {
            try {
                return await invoke<string>(
                    kind === "register" ? "mcp_register" : "mcp_unregister",
                    { client },
                );
            } finally {
                await refresh();
            }
        };
    }
</script>

<div class="space-y-6">
    <p class="px-1 text-sm text-muted-foreground">
        {t.intro}
    </p>

    {#if info && !info.exists}
        <div
            class="rounded-lg border border-border bg-muted/40 px-4 py-3 text-xs text-muted-foreground"
        >
            {t.missingServer}
        </div>
    {/if}

    {#if semanticMissing}
        <div
            class="rounded-lg border border-border bg-muted/40 px-4 py-3 text-xs text-muted-foreground"
        >
            <CopyParts parts={t.semanticHint}>
                {#snippet part(slot, text)}
                    {#if slot === "link"}<button
                            type="button"
                            class="underline underline-offset-2 transition-colors hover:text-foreground"
                            onclick={() => appState.openSettings("synthesis")}
                            >{text}</button
                        >{/if}
                {/snippet}
            </CopyParts>
        </div>
    {/if}

    <div class="flex flex-col gap-3">
        <ClientCard
            title={t.clients.claudeDesktop.title}
            status={status?.claude_desktop}
            serverExists={status?.server_exists ?? false}
            action={act("claude_desktop")}
        >
            {#snippet fallback()}
                {#if info}
                    <p class="text-xs text-muted-foreground">
                        <CopyParts
                            parts={t.clients.claudeDesktop.restart(
                                info.claude_desktop_config_path,
                            )}
                        >
                            {#snippet part(slot, text)}
                                {#if slot === "code"}<code class="font-mono break-all"
                                        >{text}</code
                                    >{/if}
                            {/snippet}
                        </CopyParts>
                    </p>
                    <CodeSnippet text={info.config_json} block />
                {/if}
            {/snippet}
        </ClientCard>

        <ClientCard
            title={t.clients.claudeCode.title}
            status={status?.claude_code}
            serverExists={status?.server_exists ?? false}
            action={act("claude_code")}
        >
            {#snippet fallback()}
                {#if info}
                    <p class="text-xs text-muted-foreground">
                        {t.clients.claudeCode.instruction}
                    </p>
                    <CodeSnippet text={info.claude_code_command} />
                {/if}
            {/snippet}
        </ClientCard>

        <ClientCard
            title={t.clients.codex.title}
            status={status?.codex}
            serverExists={status?.server_exists ?? false}
            action={act("codex")}
        >
            {#snippet fallback()}
                {#if info}
                    <p class="text-xs text-muted-foreground">
                        {t.clients.codex.instruction}
                    </p>
                    <CodeSnippet text={info.codex_command} />
                    <p class="text-xs text-muted-foreground">
                        {t.clients.codex.orConfig}
                    </p>
                    <CodeSnippet text={info.codex_toml} />
                {/if}
            {/snippet}
        </ClientCard>

        <ClientCard
            title={t.clients.other.title}
            subtitle={t.clients.other.subtitle}
        >
            {#snippet fallback()}
                {#if info}
                    <p class="text-xs text-muted-foreground">
                        {t.clients.other.pointAt}
                    </p>
                    <CodeSnippet text={info.path} />
                    <p class="text-xs text-muted-foreground">
                        {t.clients.other.orConfig}
                    </p>
                    <CodeSnippet text={info.config_json} block />
                {/if}
            {/snippet}
        </ClientCard>
    </div>
</div>
