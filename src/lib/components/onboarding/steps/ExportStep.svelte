<script lang="ts">
    // Markdown export: the switch and the folder. Template editing stays in Settings
        import { open } from "@tauri-apps/plugin-dialog";
    import { Switch } from "$lib/components/ui/switch";
    import { Button } from "$lib/components/ui/button";
    import SettingRow from "$lib/components/settings/SettingRow.svelte";
    import SettingsGroup from "$lib/components/settings/SettingsGroup.svelte";
    import { copy } from "$lib/copy";
    import type { OnboardingDraft } from "../types";

    let { draft }: { draft: OnboardingDraft } = $props();

    const t = $derived(copy.onboarding.export);

    async function pickFolder() {
        const dir = await open({ directory: true });
        if (typeof dir === "string") {
            draft.obsidian_vault_dir = dir;
            // Picking a folder is the intent; don't make them find the
            // switch too.
            draft.obsidian_export_enabled = true;
        }
    }
</script>

<h1 class="font-display text-2xl tracking-tight">{t.title}</h1>
<p class="mt-3 text-sm text-muted-foreground">
    {t.intro}
</p>

<div class="mt-6 space-y-4">
    <SettingsGroup>
        <SettingRow title={t.exportOnEnd}>
            <Switch bind:checked={draft.obsidian_export_enabled} />
        </SettingRow>

        <SettingRow
            title={t.folder}
            description={draft.obsidian_vault_dir || t.noFolder}
        >
            <Button variant="outline" size="sm" onclick={pickFolder}>
                {t.browse}
            </Button>
        </SettingRow>

        {#if draft.obsidian_export_enabled}
            <SettingRow title={t.includeSummary}>
                <Switch bind:checked={draft.export_include_summary} />
            </SettingRow>
            <SettingRow title={t.includeNotes}>
                <Switch bind:checked={draft.export_include_notes} />
            </SettingRow>
            <SettingRow title={t.includeTranscript}>
                <Switch bind:checked={draft.export_include_transcript} />
            </SettingRow>
        {/if}

        {#if draft.obsidian_export_enabled}
            <div class="px-4 py-3">
                <p class="text-xs text-muted-foreground">
                    {t.filenameNote}
                </p>
            </div>
        {/if}
    </SettingsGroup>
</div>
