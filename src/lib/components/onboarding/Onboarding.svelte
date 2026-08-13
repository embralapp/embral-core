<script lang="ts">
    // The wizard shell: assembles the step list (cloud builds open on an
    // account step and a plans page; every build ends on the welcome
    // page), renders the dots and nav, and owns the draft. The draft is
    // an overlay of only the fields onboarding touches; finish()
    // re-loads config before saving so anything the cloud-only code changed
    // mid-wizard (provider adoption on sign-in) survives
    // ([shell.md](../../../../docs/shell.md)).
    import { onMount } from "svelte";
    import type { Component } from "svelte";
    import { invoke } from "@tauri-apps/api/core";
    import { configStore } from "$lib/stores/config.svelte";
    import { modelsStore } from "$lib/stores/models.svelte";
    import { cloudAuth } from "$lib/stores/cloudAuth.svelte";
    import {
        CLOUD_ENABLED,
        loadOnboardingAccountStep,
        loadOnboardingPlansStep,
    } from "$lib/cloud";
    import { Button } from "$lib/components/ui/button";
    import EmbralIcon from "$lib/components/EmbralIcon.svelte";
    import { cn } from "$lib/utils";
    import { copy } from "$lib/copy";
    import DownloadFooter from "./DownloadFooter.svelte";
    import WelcomeStep from "./steps/WelcomeStep.svelte";
    import ModelsStep from "./steps/ModelsStep.svelte";
    import MeetingsStep from "./steps/MeetingsStep.svelte";
    import DictationStep from "./steps/DictationStep.svelte";
    import McpStep from "./steps/McpStep.svelte";
    import ExportStep from "./steps/ExportStep.svelte";
    import { draftFrom, type OnboardingDraft } from "./types";

    let step = $state(0);

    // Each step carries its telemetry name ([telemetry.md]
    // onboarding_step_viewed) beside its component. The cloud steps open
    // the flow and their components load from the cloud-only tree, so they
    // start component-less: the list's shape (and the dots) is right from the
    // first frame, and no step ever reindexes under the user. `selfNav`
    // marks the fork pages that carry their own forward buttons.
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    type Step = { name: string; component: Component<any> | null; selfNav?: boolean };
    const baseSteps: Step[] = [
        { name: "models", component: ModelsStep },
        { name: "meetings", component: MeetingsStep },
        { name: "dictation", component: DictationStep },
        { name: "mcp", component: McpStep },
        { name: "export", component: ExportStep },
        { name: "welcome", component: WelcomeStep },
    ];
    let steps = $state<Step[]>(
        CLOUD_ENABLED
            ? [
                  { name: "account", component: null, selfNav: true },
                  { name: "plans", component: null, selfNav: true },
                  ...baseSteps,
              ]
            : baseSteps,
    );

    // Seeded once at mount; the config is loaded before onboarding renders
    // (the +page gate reads it), so this is never null in practice.
    let draft = $state<OnboardingDraft>(
        draftFrom(configStore.config!),
    );

    onMount(() => {
        void modelsStore.refresh();
        // The cloud fork pages open the flow, signed in or not: the
        // account step reads the auth store, so the refresh runs in
        // parallel while its component loads from the cloud-only tree.
        if (CLOUD_ENABLED) {
            void cloudAuth.refresh();
            void (async () => {
                const [AccountStep, PlansStep] = await Promise.all([
                    loadOnboardingAccountStep(),
                    loadOnboardingPlansStep(),
                ]);
                if (AccountStep && PlansStep) {
                    steps = steps.map((s) =>
                        s.name === "account"
                            ? { ...s, component: AccountStep }
                            : s.name === "plans"
                              ? { ...s, component: PlansStep }
                              : s,
                    );
                } else {
                    // If the cloud-only components fail to load, keep the
                    // offline-shaped wizard rather than two dead pages.
                    steps = steps.filter((s) => s.component !== null);
                }
            })();
        }
    });

    // The completion mode ([telemetry.md]): recorded at the cloud forks.
    // Declining an account or never seeing one is offline; the plans page
    // decides cloud_only (skips local models) vs cloud_and_local.
    let mode = "offline";

    function advance(skip?: number) {
        const name = steps[step]?.name;
        if (name === "plans") mode = skip ? "cloud_only" : "cloud_and_local";
        step += 1 + (skip ?? 0);
    }

    // One onboarding_step_viewed per page shown (a population-level
    // funnel). Telemetry is cloud-edition only; the command doesn't
    // exist offline ([telemetry.md]).
    let lastViewed = "";
    $effect(() => {
        const name = steps[step]?.name;
        if (!CLOUD_ENABLED || !name || name === lastViewed) return;
        lastViewed = name;
        void invoke("telemetry_track", {
            name: "onboarding_step_viewed",
            props: { step: name },
        }).catch(() => {});
    });

    async function finish() {
        // Re-load before saving: the base config may have moved under the
        // wizard (cloud sign-in adopts providers backend-side).
        await configStore.load();
        const cfg = configStore.config;
        if (!cfg) return;
        await configStore.save({
            ...cfg,
            ...draft,
            onboarding_completed: true,
        });
        if (CLOUD_ENABLED) {
            void invoke("telemetry_track", {
                name: "onboarding_completed",
                props: { mode },
            }).catch(() => {});
        }
    }

    let Current = $derived(steps[step].component);
    let hideContinue = $derived(steps[step].selfNav === true);
    const t = $derived(copy.onboarding.shell);
</script>

<!-- Pinned header and footer with the step scrolling between them: the
     nav sits in the same place on every page, whatever the content
     height ([shell.md](../../../../docs/shell.md)). -->
<div class="fixed inset-0 z-50 flex flex-col bg-background">
    <div class="mx-auto flex h-full w-full max-w-xl flex-col px-8">
        <div class="flex shrink-0 items-center gap-2 pt-10 pb-8">
            <EmbralIcon size={20} />
            <span class="text-base font-semibold tracking-tight">embral</span>
        </div>

        <div class="min-h-0 flex-1 overflow-y-auto pb-4">
            <!-- Steps may skip ahead: advance(1) hops over the next step
                 (declining an account skips the plans page; "use cloud
                 only" skips the local models page). The cloud steps'
                 components arrive a beat after mount. -->
            {#if Current}
                <Current {draft} {advance} />
            {/if}
        </div>

        <div class="shrink-0 pt-4 pb-8">
            <DownloadFooter />

            <div class="mt-4 flex items-center justify-between">
                <div class="flex gap-1.5">
                    {#each steps as _, i (i)}
                        <span
                            class={cn(
                                "h-1.5 w-1.5 rounded-full",
                                i === step ? "bg-primary" : "bg-muted",
                            )}
                        ></span>
                    {/each}
                </div>
                <div class="flex items-center gap-2">
                    {#if step > 0}
                        <Button variant="ghost" size="sm" onclick={() => step--}
                            >{t.back}</Button
                        >
                    {/if}
                    {#if step < steps.length - 1}
                        <!-- Invisible, not absent, on self-navigating steps:
                             the slot keeps its width so Back sits in the
                             same place on every page. -->
                        <Button
                            size="sm"
                            class={hideContinue ? "invisible" : ""}
                            aria-hidden={hideContinue}
                            tabindex={hideContinue ? -1 : undefined}
                            onclick={() => step++}
                        >
                            {t.continue}
                        </Button>
                    {:else}
                        <Button size="sm" onclick={() => finish()}>{t.finish}</Button>
                    {/if}
                </div>
            </div>
        </div>
    </div>
</div>
