<script lang="ts">
    import { onMount } from "svelte";
    import { invoke } from "@tauri-apps/api/core";
    import { Check, PenLine, RotateCcw } from "lucide-svelte";
    import * as Dialog from "$lib/components/ui/dialog";
    import { modelsStore } from "$lib/stores/models.svelte";
    import { isLinux, isMac } from "$lib/platform";
    import { disableApp, enableApp, isAppEnabled } from "$lib/utils/allowlist";
    import TranscriptionBlock from "./TranscriptionBlock.svelte";
    import HotkeyCapture from "./HotkeyCapture.svelte";
    import type {
        AppConfig,
        AutoStartPolicy,
        AutoStopScope,
        SilenceUnanswered,
        DiarizationSensitivity,
        NotesNamingMode,
        OpenMeetingTab,
    } from "$lib/types";
    import { BUILTIN_PROFILE_ID, CLOUD_PROFILE_ID } from "$lib/types";
    import { CLOUD_ENABLED } from "$lib/cloud";
    import { cloudAuth } from "$lib/stores/cloudAuth.svelte";
    import { copy } from "$lib/copy";
    import SettingsGroup from "./SettingsGroup.svelte";
    import SettingRow from "./SettingRow.svelte";
    import * as Select from "$lib/components/ui/select";
    import { Switch } from "$lib/components/ui/switch";
    import { Input } from "$lib/components/ui/input";
    import { Button } from "$lib/components/ui/button";
    import { Textarea } from "$lib/components/ui/textarea";
    import { cn } from "$lib/utils";

    let { draft }: { draft: AppConfig } = $props();

    const t = $derived(copy.settings.meetings);
    const providers = $derived(copy.common.providers);

    // --- Auto-start -----------------------------------------------------

    // The order is fixed here; the names come from the catalog by key.
    const policyOrder: AutoStartPolicy[] = [
        "always",
        "selective",
        "prompt",
        "manual",
    ];

    let policyLabel = $derived(
        t.autoStart.prompt.options[draft.auto_start_policy] ??
            t.autoStart.prompt.options.prompt,
    );

    const autoStopOrder: AutoStopScope[] = ["never", "auto_started", "all"];

    let autoStopLabel = $derived(
        t.autoStart.autoStop.options[draft.auto_stop] ??
            t.autoStart.autoStop.options.auto_started,
    );

    const silenceUnansweredOrder: SilenceUnanswered[] = ["stop", "keep"];

    let silenceUnansweredLabel = $derived(
        t.autoStart.silenceUnanswered.options[draft.silence_stop_unanswered] ??
            t.autoStart.silenceUnanswered.options.stop,
    );

    // The fixed set of meeting apps (4×2 grid). Each checkbox owns one
    // process-match string; the detector matches case-insensitive substrings
    // against every identity the platform reports ("teams" catches
    // ms-teams.exe and com.microsoft.teams2 alike). The names come from the
    // catalog by key; `match` is detector data, platform-keyed where the
    // brand token differs (msedge.exe vs com.microsoft.edgemac).
    const knownApps: {
        key: keyof typeof copy.settings.meetings.autoStart.apps.names;
        match: string;
    }[] = [
        { key: "zoom", match: "zoom" },
        { key: "teams", match: "teams" },
        { key: "chrome", match: "chrome" },
        { key: "edge", match: isMac ? "edge" : "msedge" },
        ...(isMac ? [{ key: "safari", match: "safari" } as const] : []),
        // Linux-only row. Match strings should agree with that platform's
        // `default_auto_detect_apps` (embral-types) so a fresh install shows
        // the boxes it is actually detecting with.
        ...(isLinux ? [{ key: "chromium", match: "chromium" } as const] : []),
        { key: "firefox", match: "firefox" },
        { key: "slack", match: "slack" },
        { key: "discord", match: "discord" },
        { key: "webex", match: "webex" },
    ];

    // Both read the allowlist the way the detector does (bidirectional
    // substring), not by exact equality; otherwise a covering entry the grid
    // has no checkbox for survives an uncheck and keeps the app detected
    // while the box shows off. See utils/allowlist.ts.
    function appChecked(match: string): boolean {
        return isAppEnabled(draft.auto_detect_apps, match);
    }

    function toggleApp(match: string) {
        draft.auto_detect_apps = appChecked(match)
            ? disableApp(draft.auto_detect_apps, match)
            : enableApp(draft.auto_detect_apps, match);
    }


    // --- Summaries prompt ----------------------------------------------------

    let defaultPrompt = $state("");
    let contract = $state("");
    let showContract = $state(false);
    let editorOpen = $state(false);

    onMount(async () => {
        modelsStore.refresh();
        try {
            const parts = await invoke<{ default: string; contract: string }>(
                "get_summary_prompt_parts",
            );
            defaultPrompt = parts.default;
            contract = parts.contract;
        } catch (e) {
            console.error("get_summary_prompt_parts failed:", e);
        }
    });

    // The summary engine (moved here from the Synthesis page: it is the
    // product question "who writes my summaries", not a model-management
    // knob). Engines are fixed per edition; the backend picks actual models.
    let engineValue = $derived(draft.summaries_profile_id || BUILTIN_PROFILE_ID);
    let engineLabel = $derived(
        engineValue === CLOUD_PROFILE_ID
            ? providers.cloud
            : providers.localModel,
    );

    // "" in config means "use the default": the editor always shows the
    // effective text; the first edit materializes it as a custom prompt.
    let customized = $derived(draft.summary_prompt.trim().length > 0);
    let promptText = $derived(customized ? draft.summary_prompt : defaultPrompt);

    function onPromptInput(value: string) {
        // Typing the default back verbatim returns to "not customized".
        draft.summary_prompt = value.trim() === defaultPrompt.trim() ? "" : value;
    }

    // "Open on Summary" is only an option while summaries exist at all; a
    // stored `summary` with the switch off rewrites to notes.
    $effect(() => {
        if (!draft.summaries_enabled && draft.open_meeting_tab === "summary") {
            draft.open_meeting_tab = "notes";
        }
    });

</script>

<div class="space-y-6">
    <SettingsGroup label={t.transcription._group}>
        <TranscriptionBlock
            providerLabel={t.transcription.providerLabel}
            disabledNote={t.transcription.disabledNote}
            provider={draft.transcription_provider}
            onProviderChange={(v) => (draft.transcription_provider = v)}
            outOfHours={draft.cloud_out_of_hours}
            onOutOfHoursChange={(v) => (draft.cloud_out_of_hours = v)}
            powerPolicy={draft.transcription_power_policy}
            onPowerPolicyChange={(v) => (draft.transcription_power_policy = v)}
            language={draft.transcription_language}
            onLanguageChange={(v) => (draft.transcription_language = v)}
            accuracyModel={draft.local_asr_model}
            onAccuracyChange={(id) => (draft.local_asr_model = id)}
        />
    </SettingsGroup>

    <SettingsGroup label={t.autoStart._group}>
        <SettingRow title={t.autoStart.prompt.label}>
            <Select.Root
                type="single"
                value={draft.auto_start_policy}
                onValueChange={(v) =>
                    (draft.auto_start_policy = (v ??
                        "prompt") as AutoStartPolicy)}
            >
                <Select.Trigger class="w-56">{policyLabel}</Select.Trigger>
                <Select.Content>
                    {#each policyOrder as value (value)}
                        <Select.Item
                            {value}
                            label={t.autoStart.prompt.options[value]}
                        />
                    {/each}
                </Select.Content>
            </Select.Root>
        </SettingRow>

        {#if draft.auto_start_policy !== "manual"}
            {#if draft.auto_start_policy !== "always"}
                <SettingRow title={t.autoStart.apps.label} vertical>
                    <div class="grid max-w-xl grid-cols-4 gap-2">
                        {#each knownApps as app (app.match)}
                            {@const checked = appChecked(app.match)}
                            {@const appName = t.autoStart.apps.names[app.key]}
                            <button
                                type="button"
                                class={cn(
                                    "flex items-center gap-2 rounded-md border px-2.5 py-1.5 text-left text-xs transition-colors",
                                    checked
                                        ? "border-primary/50 bg-primary/10"
                                        : "border-border hover:bg-accent/50",
                                )}
                                role="checkbox"
                                aria-checked={checked}
                                onclick={() => toggleApp(app.match)}
                            >
                                <span
                                    class={cn(
                                        "flex size-3.5 shrink-0 items-center justify-center rounded-sm border",
                                        checked
                                            ? "border-primary bg-primary text-primary-foreground"
                                            : "border-input",
                                    )}
                                >
                                    {#if checked}<Check size={10} strokeWidth={3} />{/if}
                                </span>
                                {appName}
                            </button>
                        {/each}
                    </div>
                </SettingRow>
            {/if}

            <SettingRow
                title={t.autoStart.delay.label}
                description={t.autoStart.delay.sub}
            >
                <div class="flex items-center gap-2">
                    <Input
                        type="number"
                        min="1"
                        max="60"
                        value={String(draft.detection_delay_secs)}
                        oninput={(e) =>
                            (draft.detection_delay_secs = Math.max(
                                1,
                                Number(e.currentTarget.value) || 1,
                            ))}
                        class="w-16 text-right"
                    />
                    <span class="text-xs text-muted-foreground"
                        >{t.autoStart.delay.unit}</span
                    >
                </div>
            </SettingRow>

            <SettingRow title={t.autoStart.autoStop.label}>
                <Select.Root
                    type="single"
                    value={draft.auto_stop}
                    onValueChange={(v) =>
                        (draft.auto_stop = (v ?? "auto_started") as AutoStopScope)}
                >
                    <Select.Trigger class="w-56">{autoStopLabel}</Select.Trigger>
                    <Select.Content>
                        {#each autoStopOrder as value (value)}
                            <Select.Item
                                {value}
                                label={t.autoStart.autoStop.options[value]}
                            />
                        {/each}
                    </Select.Content>
                </Select.Root>
            </SettingRow>
        {/if}

        <!-- The silence check-in guards every recording, whatever the
             detection policy; it sits outside the policy gate. -->
        <SettingRow
            title={t.autoStart.silence.label}
            description={t.autoStart.silence.sub}
        >
            <div class="flex items-center gap-2">
                <Input
                    type="number"
                    min="0"
                    max="120"
                    value={String(draft.silence_stop_minutes)}
                    oninput={(e) =>
                        (draft.silence_stop_minutes = Math.max(
                            0,
                            Number(e.currentTarget.value) || 0,
                        ))}
                    class="w-16 text-right"
                />
                <span class="text-xs text-muted-foreground"
                    >{t.autoStart.silence.unit}</span
                >
            </div>
        </SettingRow>

        {#if draft.silence_stop_minutes > 0}
            <SettingRow title={t.autoStart.silenceUnanswered.label}>
                <Select.Root
                    type="single"
                    value={draft.silence_stop_unanswered}
                    onValueChange={(v) =>
                        (draft.silence_stop_unanswered = (v ??
                            "stop") as SilenceUnanswered)}
                >
                    <Select.Trigger class="w-56"
                        >{silenceUnansweredLabel}</Select.Trigger
                    >
                    <Select.Content>
                        {#each silenceUnansweredOrder as value (value)}
                            <Select.Item
                                {value}
                                label={t.autoStart.silenceUnanswered.options[value]}
                            />
                        {/each}
                    </Select.Content>
                </Select.Root>
            </SettingRow>
        {/if}
    </SettingsGroup>

    <SettingsGroup label={t.toggle._group}>
        <SettingRow title={t.toggle.hotkey.label}>
            <HotkeyCapture
                value={draft.record_hotkey}
                ariaLabel={t.toggle.hotkey.aria}
                onChange={(combo) => (draft.record_hotkey = combo)}
            />
        </SettingRow>
    </SettingsGroup>

    <SettingsGroup label={t.speakers._group}>
        <SettingRow title={t.speakers.detect.label}>
            <Switch bind:checked={draft.diarization_enabled} />
        </SettingRow>

        {#if draft.diarization_enabled}
            <SettingRow title={t.speakers.separation.label}>
                <Select.Root
                    type="single"
                    value={draft.diarization_sensitivity}
                    onValueChange={(v) =>
                        v &&
                        (draft.diarization_sensitivity =
                            v as DiarizationSensitivity)}
                >
                    <Select.Trigger class="w-56"
                        >{t.speakers.separation.options[
                            draft.diarization_sensitivity
                        ]}</Select.Trigger
                    >
                    <Select.Content>
                        <Select.Item
                            value="low"
                            label={t.speakers.separation.options.low}
                        />
                        <Select.Item
                            value="medium"
                            label={t.speakers.separation.options.medium}
                        />
                        <Select.Item
                            value="high"
                            label={t.speakers.separation.options.high}
                        />
                    </Select.Content>
                </Select.Root>
            </SettingRow>

            <SettingRow title={t.speakers.naming.label}>
                <Select.Root
                    type="single"
                    value={draft.notes_naming_mode}
                    onValueChange={(v) =>
                        v && (draft.notes_naming_mode = v as NotesNamingMode)}
                >
                    <Select.Trigger class="w-56"
                        >{t.speakers.naming.options[
                            draft.notes_naming_mode
                        ]}</Select.Trigger
                    >
                    <Select.Content>
                        <Select.Item
                            value="off"
                            label={t.speakers.naming.options.off}
                        />
                        <Select.Item
                            value="suggest"
                            label={t.speakers.naming.options.suggest}
                        />
                        <Select.Item
                            value="automatic"
                            label={t.speakers.naming.options.automatic}
                        />
                    </Select.Content>
                </Select.Root>
            </SettingRow>
        {/if}
    </SettingsGroup>

    <SettingsGroup label={t.summaries._group}>
        <SettingRow title={t.summaries.enabled.label}>
            <Switch bind:checked={draft.summaries_enabled} />
        </SettingRow>

        {#if draft.summaries_enabled}
            <SettingRow title={t.summaries.engine.label}>
                <Select.Root
                    type="single"
                    value={engineValue}
                    onValueChange={(v) => {
                        if (!v) return;
                        // embral cloud needs an account; refuse and prompt when
                        // signed out, leaving the engine on its current value.
                        if (v === CLOUD_PROFILE_ID && !cloudAuth.requireSignedIn())
                            return;
                        draft.summaries_profile_id = v;
                    }}
                >
                    <Select.Trigger class="w-56">{engineLabel}</Select.Trigger>
                    <Select.Content>
                        {#if CLOUD_ENABLED}
                            <Select.Item
                                value={CLOUD_PROFILE_ID}
                                label={providers.cloud}
                            />
                        {/if}
                        <Select.Item
                            value={BUILTIN_PROFILE_ID}
                            label={providers.localModel}
                        />
                    </Select.Content>
                </Select.Root>
            </SettingRow>

            <SettingRow
                title={t.summaries.prompt.label}
                description={customized ? t.summaries.prompt.customized : ""}
            >
                <Button variant="outline" size="sm" onclick={() => (editorOpen = true)}>
                    <PenLine size={13} class="mr-1" /> {t.summaries.prompt.edit}
                </Button>
            </SettingRow>
        {/if}

        <SettingRow title={t.summaries.openOn.label}>
            <Select.Root
                type="single"
                value={draft.open_meeting_tab}
                onValueChange={(v) =>
                    v && (draft.open_meeting_tab = v as OpenMeetingTab)}
            >
                <Select.Trigger class="w-56"
                    >{t.summaries.openOn.options[
                        draft.open_meeting_tab
                    ]}</Select.Trigger
                >
                <Select.Content>
                    {#if draft.summaries_enabled}
                        <Select.Item
                            value="summary"
                            label={t.summaries.openOn.options.summary}
                        />
                    {/if}
                    <Select.Item
                        value="notes"
                        label={t.summaries.openOn.options.notes}
                    />
                    <Select.Item
                        value="transcript"
                        label={t.summaries.openOn.options.transcript}
                    />
                </Select.Content>
            </Select.Root>
        </SettingRow>
    </SettingsGroup>

    <SettingsGroup label={t.audio._group}>
        <SettingRow title={t.audio.keep.label}>
            <Switch bind:checked={draft.retain_audio} />
        </SettingRow>

        <SettingRow
            title={t.audio.deleteAudio.label}
            description={t.audio.deleteAudio.sub}
        >
            <div class="flex items-center gap-2">
                <Input
                    type="number"
                    min="0"
                    value={String(draft.audio_retention_days)}
                    oninput={(e) =>
                        (draft.audio_retention_days = Math.max(
                            0,
                            Math.floor(Number(e.currentTarget.value) || 0),
                        ))}
                    class="w-16 text-right"
                />
                <span class="text-xs text-muted-foreground"
                    >{t.audio.deleteAudio.unit}</span
                >
            </div>
        </SettingRow>

        <SettingRow
            title={t.audio.deleteMeetings.label}
            description={t.audio.deleteMeetings.sub}
        >
            <div class="flex items-center gap-2">
                <Input
                    type="number"
                    min="0"
                    value={String(draft.meeting_retention_days)}
                    oninput={(e) =>
                        (draft.meeting_retention_days = Math.max(
                            0,
                            Math.floor(Number(e.currentTarget.value) || 0),
                        ))}
                    class="w-16 text-right"
                />
                <span class="text-xs text-muted-foreground"
                    >{t.audio.deleteMeetings.unit}</span
                >
            </div>
        </SettingRow>
    </SettingsGroup>
</div>

<Dialog.Root bind:open={editorOpen}>
    <Dialog.Content class="sm:max-w-3xl">
        <Dialog.Header>
            <Dialog.Title>{t.promptDialog.title}</Dialog.Title>
            <Dialog.Description>
                {t.promptDialog.description}
            </Dialog.Description>
        </Dialog.Header>
        <div class="space-y-2">
            <Textarea
                value={promptText}
                rows={16}
                class="max-h-[55vh] font-mono text-xs leading-relaxed"
                oninput={(e) => onPromptInput(e.currentTarget.value)}
            />
            <button
                class="text-xs text-muted-foreground underline-offset-2 hover:text-foreground hover:underline"
                onclick={() => (showContract = !showContract)}
            >
                {showContract
                    ? t.promptDialog.hideFormat
                    : t.promptDialog.showFormat}
            </button>
            {#if showContract}
                <pre
                    class="max-h-48 overflow-y-auto rounded-md border border-border bg-muted/40 p-3 font-mono text-[11px] leading-relaxed whitespace-pre-wrap text-muted-foreground">{contract}</pre>
            {/if}
        </div>
        <Dialog.Footer class="items-center gap-2 sm:justify-between">
            <div>
                {#if customized}
                    <Button
                        variant="outline"
                        size="sm"
                        onclick={() => (draft.summary_prompt = "")}
                    >
                        <RotateCcw size={13} class="mr-1" /> {t.promptDialog.reset}
                    </Button>
                {/if}
            </div>
            <Button size="sm" onclick={() => (editorOpen = false)}
                >{t.promptDialog.done}</Button
            >
        </Dialog.Footer>
    </Dialog.Content>
</Dialog.Root>
