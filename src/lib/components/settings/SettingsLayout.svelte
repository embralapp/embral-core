<script lang="ts">
    import { onMount } from "svelte";
    import {
        SettingsIcon,
        Mic,
        Disc,
        AudioLines,
        Network,
        Info,
        FileText,
        Webhook,
        Speech,
        Brain,
        CircleUser,
    } from "lucide-svelte";
    import { CLOUD_ENABLED, loadAccountSection } from "$lib/cloud";
    import type { AppConfig } from "$lib/types";
    import { appState } from "$lib/stores/app-state.svelte";
    import { settingsForm } from "$lib/stores/settings-form.svelte";
    import { cloudAuth } from "$lib/stores/cloudAuth.svelte";
    import { copy } from "$lib/copy";
    import { cn } from "$lib/utils";
    import GeneralSection from "./GeneralSection.svelte";
    import MeetingsSection from "./MeetingsSection.svelte";
    import DictationSection from "./DictationSection.svelte";
    import AboutSection from "./AboutSection.svelte";
    import MarkdownSection from "./MarkdownSection.svelte";
    import WebhooksSection from "./WebhooksSection.svelte";
    import McpSection from "./McpSection.svelte";
    import TranscriptionSection from "./TranscriptionSection.svelte";
    import SynthesisSection from "./SynthesisSection.svelte";
    import CloudSignInDialog from "./CloudSignInDialog.svelte";

    type SectionId =
        | "account"
        | "general"
        | "meetings"
        | "dictation"
        | "about"
        | "markdown"
        | "webhooks"
        | "mcp"
        | "transcription"
        | "synthesis";

    // The grouping and order are this component's (docs/shell.md); the names
    // come from the catalog, shared with the palette's deep links.
    let nav = $derived(copy.settings.nav);

    type Entry = { id: SectionId; label: string; icon: typeof Mic };
    let groups: { label: string; items: Entry[] }[] = $derived([
        {
            label: nav.groups.application,
            items: [
                { id: "general", label: nav.sections.general, icon: SettingsIcon },
                { id: "meetings", label: nav.sections.meetings, icon: Disc },
                { id: "dictation", label: nav.sections.dictation, icon: Speech },
                // Account exists only in the cloud edition; it sits with About
                // — the two pages about you rather than about the app's work.
                ...(CLOUD_ENABLED
                    ? [
                          {
                              id: "account",
                              label: nav.sections.account,
                              icon: CircleUser,
                          } as Entry,
                      ]
                    : []),
                { id: "about", label: nav.sections.about, icon: Info },
            ],
        },
        {
            label: nav.groups.models,
            items: [
                {
                    id: "transcription",
                    label: nav.sections.transcription,
                    icon: AudioLines,
                },
                { id: "synthesis", label: nav.sections.synthesis, icon: Brain },
            ],
        },
        {
            label: nav.groups.integrations,
            items: [
                { id: "markdown", label: nav.sections.markdown, icon: FileText },
                { id: "webhooks", label: nav.sections.webhooks, icon: Webhook },
                { id: "mcp", label: nav.sections.mcp, icon: Network },
            ],
        },
    ]);
    let sections: Entry[] = $derived(groups.flatMap((g) => g.items));

    let active = $state<SectionId>("general");
    let initialized = false;

    onMount(() => {
        settingsForm.reset();
        // Know the cloud sign-in state before the user reaches a provider
        // selector, and keep it current when they sign in or out on the
        // Account page (which dispatches this event).
        if (!CLOUD_ENABLED) return;
        void cloudAuth.refresh();
        const onCloudChanged = () => {
            void cloudAuth.refresh();
            // Sign-in/out rewrote config backend-side (token, adopted
            // providers); a draft snapshotted before that would save the
            // old values back over it.
            settingsForm.reset();
        };
        window.addEventListener("embral:cloud-changed", onCloudChanged);
        return () =>
            window.removeEventListener("embral:cloud-changed", onCloudChanged);
    });

    // Palette deep links: land on the requested page (also fires when a
    // deep link arrives while settings is already open).
    $effect(() => {
        const target = appState.settingsTarget;
        if (target && sections.some((s) => s.id === target)) {
            active = target as SectionId;
            appState.clearSettingsTarget();
        }
    });

    // Snapshot the whole draft (spread reads every field) so any change in any
    // section schedules the shared debounced save. The first run after reset
    // is the initialization pass, not an edit.
    $effect(() => {
        const draft = settingsForm.draft;
        if (!draft) return;
        const snapshot: AppConfig = { ...draft };
        if (!initialized) {
            initialized = true;
            return;
        }
        settingsForm.scheduleSave(snapshot);
    });

</script>

{#if settingsForm.draft}
    <div class="flex min-h-0 flex-1">
        <nav class="w-52 shrink-0 space-y-4 overflow-y-auto border-r border-border p-3">
            {#each groups as group (group.label)}
                <div>
                    <p
                        class="px-2.5 pb-1 text-[10px] font-semibold tracking-widest text-muted-foreground/70 uppercase"
                    >
                        {group.label}
                    </p>
                    <div class="space-y-0.5">
                        {#each group.items as s (s.id)}
                            <button
                                class={cn(
                                    "flex w-full items-center gap-2.5 rounded-md px-2.5 py-1.5 text-left text-sm text-muted-foreground transition-colors hover:bg-accent hover:text-foreground",
                                    active === s.id &&
                                        "bg-accent font-medium text-foreground",
                                )}
                                onclick={() => (active = s.id)}
                            >
                                <s.icon size={15} />
                                {s.label}
                            </button>
                        {/each}
                    </div>
                </div>
            {/each}
        </nav>

        <div class="min-w-0 flex-1 overflow-y-auto">
            <div class="mx-auto max-w-3xl px-6 py-6">
                <h2 class="mb-5 text-lg font-semibold tracking-tight">
                    {sections.find((s) => s.id === active)?.label}
                </h2>
                {#if active === "account"}
                    {#await loadAccountSection() then Account}
                        {#if Account}<Account />{/if}
                    {/await}
                {:else if active === "general"}
                    <GeneralSection draft={settingsForm.draft} />
                {:else if active === "meetings"}
                    <MeetingsSection draft={settingsForm.draft} />
                {:else if active === "dictation"}
                    <DictationSection draft={settingsForm.draft} />
                {:else if active === "about"}
                    <AboutSection />
                {:else if active === "markdown"}
                    <MarkdownSection draft={settingsForm.draft} />
                {:else if active === "webhooks"}
                    <WebhooksSection draft={settingsForm.draft} />
                {:else if active === "mcp"}
                    <McpSection />
                {:else if active === "transcription"}
                    <TranscriptionSection draft={settingsForm.draft} />
                {:else if active === "synthesis"}
                    <SynthesisSection draft={settingsForm.draft} />
                {/if}
            </div>
        </div>
    </div>

    {#if CLOUD_ENABLED}
        <CloudSignInDialog />
    {/if}
{:else}
    <div class="flex flex-1 items-center justify-center">
        <p class="text-sm text-muted-foreground">{nav.loading}</p>
    </div>
{/if}
