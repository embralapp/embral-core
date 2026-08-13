<script lang="ts">
    import { invoke } from "@tauri-apps/api/core";
    import { Plus, Trash2 } from "lucide-svelte";
    import type {
        AppConfig,
        WebhookDestination,
        WebhookMethod,
    } from "$lib/types";
    import { errorMessage } from "$lib/copy/errors";
    import { copy } from "$lib/copy";
    import SettingsGroup from "./SettingsGroup.svelte";
    import SettingRow from "./SettingRow.svelte";
    import * as Select from "$lib/components/ui/select";
    import { Input } from "$lib/components/ui/input";
    import { Switch } from "$lib/components/ui/switch";
    import { Button } from "$lib/components/ui/button";

    let { draft }: { draft: AppConfig } = $props();

    const t = $derived(copy.settings.webhooks);

    // Rows edit by wholesale reassignment: the settings autosave snapshots
    // top-level fields, so a deep mutation inside the array would never
    // schedule a save (the auto_detect_apps pattern).
    function update(index: number, patch: Partial<WebhookDestination>) {
        draft.webhooks = draft.webhooks.map((d, i) =>
            i === index ? { ...d, ...patch } : d,
        );
    }

    function add() {
        draft.webhooks = [
            ...draft.webhooks,
            { url: "", method: "post", include_content: false },
        ];
    }

    function remove(index: number) {
        draft.webhooks = draft.webhooks.filter((_, i) => i !== index);
        // Statuses are keyed by row index, which just shifted.
        tests = {};
    }

    // One test status per row. The command takes the row's draft values
    // directly, so an unsaved row is testable.
    type TestState = { state: "sending" | "ok" | "error"; error: string };
    let tests = $state<Record<number, TestState>>({});

    async function sendTest(index: number) {
        const d = draft.webhooks[index];
        tests[index] = { state: "sending", error: "" };
        try {
            await invoke("test_webhook", {
                url: d.url,
                method: d.method,
                includeContent: d.include_content,
            });
            tests[index] = { state: "ok", error: "" };
        } catch (e) {
            tests[index] = { state: "error", error: errorMessage(e) };
        }
    }
</script>

<div class="space-y-6">
    <SettingsGroup label={t.destinations._group}>
        <SettingRow
            title={t.destinations.intro.label}
            description={t.destinations.intro.sub}
            vertical
        >
            <div class="w-full space-y-3">
                {#each draft.webhooks as d, i (i)}
                    <div class="space-y-2.5 rounded-md border border-border p-3">
                        <div class="flex w-full items-center gap-2">
                            <Select.Root
                                type="single"
                                value={d.method}
                                onValueChange={(v) =>
                                    update(i, {
                                        method: (v ?? "post") as WebhookMethod,
                                    })}
                            >
                                <Select.Trigger class="w-24"
                                    >{t.destinations.method[
                                        d.method
                                    ]}</Select.Trigger
                                >
                                <Select.Content>
                                    <Select.Item
                                        value="post"
                                        label={t.destinations.method.post}
                                    />
                                    <Select.Item
                                        value="put"
                                        label={t.destinations.method.put}
                                    />
                                </Select.Content>
                            </Select.Root>
                            <Input
                                value={d.url}
                                placeholder={t.destinations.urlPlaceholder}
                                class="flex-1"
                                oninput={(e) =>
                                    update(i, { url: e.currentTarget.value })}
                            />
                            <Button
                                variant="outline"
                                size="sm"
                                disabled={!d.url.trim() ||
                                    tests[i]?.state === "sending"}
                                onclick={() => sendTest(i)}
                            >
                                {tests[i]?.state === "sending"
                                    ? t.destinations.test.sending
                                    : t.destinations.test.send}
                            </Button>
                            <Button
                                variant="ghost"
                                size="sm"
                                aria-label={t.destinations.removeAria}
                                onclick={() => remove(i)}
                            >
                                <Trash2 size={14} />
                            </Button>
                        </div>
                        <div class="flex items-center justify-between gap-6">
                            <div class="min-w-0">
                                <p class="text-sm font-medium">
                                    {t.destinations.content.label}
                                </p>
                                <p class="mt-0.5 text-xs text-muted-foreground">
                                    {t.destinations.content.sub}
                                </p>
                            </div>
                            <Switch
                                checked={d.include_content}
                                onCheckedChange={(v) =>
                                    update(i, { include_content: v })}
                            />
                        </div>
                        {#if tests[i]?.state === "ok"}
                            <p class="text-xs text-primary">
                                {t.destinations.test.ok}
                            </p>
                        {:else if tests[i]?.state === "error"}
                            <p class="text-xs text-destructive">
                                {tests[i].error}
                            </p>
                        {/if}
                    </div>
                {/each}
                <Button variant="outline" size="sm" onclick={add}>
                    <Plus size={13} class="mr-1" />
                    {t.destinations.add}
                </Button>
            </div>
        </SettingRow>
    </SettingsGroup>

    <p class="px-1 text-xs text-muted-foreground">{t.payloadNote}</p>
</div>
