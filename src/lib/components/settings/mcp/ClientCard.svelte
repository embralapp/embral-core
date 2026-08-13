<script lang="ts">
    // One MCP client as a card, styled like the model library: a colored left
    // edge for status (green registered, amber installed, red not installed;
    // gray for the informational "other clients" card), the name, a one-word
    // status, the Register/Remove action, and a collapsed manual fallback.
    import type { Snippet } from "svelte";
    import { ChevronDown, ChevronRight } from "lucide-svelte";
    import { Button } from "$lib/components/ui/button";
    import { copy } from "$lib/copy";
    import type { ClientStatus, McpAction } from "./types";

    const t = $derived(copy.settings.mcp.card);

    let {
        title,
        subtitle,
        status = null,
        serverExists = false,
        action = null,
        fallbackLabel = t.manualSetup,
        fallback,
    }: {
        title: string;
        // Shown as the status line on the informational card (no detection).
        subtitle?: string;
        status?: ClientStatus | null;
        serverExists?: boolean;
        action?: McpAction | null;
        fallbackLabel?: string;
        fallback?: Snippet;
    } = $props();

    let busy = $state(false);
    let message = $state<string | null>(null);
    let error = $state<string | null>(null);
    let fallbackOpen = $state(false);

    // A card with no detection and no action is purely informational (Other
    // clients): its manual setup is the whole point, so it stays open.
    let informational = $derived(!status && !action);

    // Lead with the manual path when the client isn't installed or an action
    // failed; the informational card is always open.
    $effect(() => {
        if ((status && !status.installed) || informational) fallbackOpen = true;
    });

    let edge = $derived(
        status?.registered
            ? "border-l-emerald-500"
            : status?.installed
              ? "border-l-amber-500"
              : status
                ? "border-l-destructive"
                : "border-l-muted-foreground/40",
    );

    let statusText = $derived(
        status
            ? status.registered
                ? t.registered
                : status.installed
                  ? t.installed
                  : t.notInstalled
            : informational
              ? (subtitle ?? "")
              : t.checking,
    );

    async function run(kind: "register" | "unregister") {
        if (!action) return;
        busy = true;
        message = null;
        error = null;
        try {
            message = await action(kind);
        } catch (e) {
            error = String(e);
            fallbackOpen = true;
        } finally {
            busy = false;
        }
    }
</script>

<div
    class="flex flex-col rounded-lg border border-border border-l-2 {edge} bg-card p-4"
>
    <div class="flex items-start justify-between gap-3">
        <div class="min-w-0">
            <p class="text-sm font-medium">{title}</p>
            <p
                class="mt-0.5 truncate text-xs text-muted-foreground"
                title={status?.detail}
            >
                {statusText}
            </p>
        </div>
        {#if status?.installed && action}
            {#if status.registered}
                <Button
                    variant="outline"
                    size="sm"
                    disabled={busy}
                    onclick={() => run("unregister")}
                >
                    {busy ? t.working : t.remove}
                </Button>
            {:else}
                <Button
                    size="sm"
                    disabled={busy || !serverExists}
                    onclick={() => run("register")}
                >
                    {busy ? t.working : t.register}
                </Button>
            {/if}
        {/if}
    </div>

    {#if message}
        <p class="mt-2 text-xs text-primary">{message}</p>
    {/if}
    {#if error}
        <p class="mt-2 text-xs text-destructive">{error}</p>
    {/if}

    {#if fallback}
        {#if informational}
            <div class="mt-3 space-y-2">{@render fallback()}</div>
        {:else}
            <button
                type="button"
                class="mt-3 flex items-center gap-1 text-xs text-muted-foreground transition-colors hover:text-foreground"
                onclick={() => (fallbackOpen = !fallbackOpen)}
            >
                {#if fallbackOpen}<ChevronDown size={12} />{:else}<ChevronRight
                        size={12}
                    />{/if}
                {fallbackLabel}
            </button>
            {#if fallbackOpen}
                <div class="mt-2 space-y-2">{@render fallback()}</div>
            {/if}
        {/if}
    {/if}
</div>
