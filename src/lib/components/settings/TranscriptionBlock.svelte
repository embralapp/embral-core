<script lang="ts">
    // The transcription decision tree as settings rows: provider,
    // out-of-hours behavior, language, and the on-device accuracy tier.
    // Meetings and Dictation each have their own copy of these fields in
    // config, so the block binds through values + onChange props rather
    // than a draft: one component, two trees.
    import type {
        CloudOutOfHours,
        PowerPolicy,
        TranscriptionLanguage,
        TranscriptionProvider,
    } from "$lib/types";
    import SettingRow from "./SettingRow.svelte";
    import SpeechModelPicker from "./SpeechModelPicker.svelte";
    import * as Select from "$lib/components/ui/select";
    import { Switch } from "$lib/components/ui/switch";
    import { CLOUD_ENABLED } from "$lib/cloud";
    import { cloudAuth } from "$lib/stores/cloudAuth.svelte";
    import { copy } from "$lib/copy";

    const t = $derived(copy.settings.transcription.block);
    const providers = $derived(copy.common.providers);

    let {
        providerLabel,
        cloudNote = "",
        disabledNote = "",
        provider,
        onProviderChange,
        outOfHours,
        onOutOfHoursChange,
        powerPolicy,
        onPowerPolicyChange,
        language,
        onLanguageChange,
        accuracyModel,
        onAccuracyChange,
    }: {
        /** "Dictate with" / "Transcribe meetings with". */
        providerLabel: string;
        /** Shown under the provider row while cloud is selected. */
        cloudNote?: string;
        /** What "Disable transcription" means for this surface. */
        disabledNote?: string;
        provider: TranscriptionProvider;
        onProviderChange: (v: TranscriptionProvider) => void;
        outOfHours: CloudOutOfHours | undefined;
        onOutOfHoursChange: (v: CloudOutOfHours) => void;
        /** Meetings only for now: dictation passes neither, and the row is
         * left out entirely. */
        powerPolicy?: PowerPolicy;
        onPowerPolicyChange?: (v: PowerPolicy) => void;
        language: TranscriptionLanguage;
        onLanguageChange: (v: TranscriptionLanguage) => void;
        /** The effective English model id the accuracy picker shows. */
        accuracyModel: string;
        onAccuracyChange: (id: string) => void;
    } = $props();

    // Whether the cloud is reachable from this configuration at all: the
    // power policy routes to it on battery whatever the provider row says,
    // so the out-of-hours question applies then too.
    let cloudPossible = $derived(
        provider === "cloud" || powerPolicy === "cloud_on_battery",
    );

    // The device transcribes when it is the primary, when an out-of-hours
    // cloud session switches to it, or whenever the power policy is on
    // (plugged in is the device). Only cloud-with-"disable" never needs the
    // accuracy tier.
    let deviceTranscribes = $derived(
        !CLOUD_ENABLED ||
            provider !== "cloud" ||
            powerPolicy === "cloud_on_battery" ||
            outOfHours !== "disabled",
    );
</script>

{#if CLOUD_ENABLED}
    <SettingRow
        title={providerLabel}
        description={provider === "cloud" ? cloudNote : ""}
    >
        <Select.Root
            type="single"
            value={provider}
            onValueChange={(v) => {
                if (v === "local") onProviderChange(v);
                // Switching to cloud needs an account; refuse and prompt when
                // signed out, leaving the provider on its current value.
                else if (v === "cloud" && cloudAuth.requireSignedIn())
                    onProviderChange(v);
            }}
        >
            <Select.Trigger class="w-56"
                >{provider === "cloud"
                    ? providers.cloud
                    : providers.localModel}</Select.Trigger
            >
            <Select.Content>
                <!-- Cloud first: the one order every engine select shares
                     ([shell.md](../../../../docs/shell.md)). -->
                <Select.Item value="cloud" label={providers.cloud} />
                <Select.Item value="local" label={providers.localModel} />
            </Select.Content>
        </Select.Root>
    </SettingRow>

    {#if onPowerPolicyChange}
        <SettingRow
            title={t.powerPolicy.label}
            description={t.powerPolicy.sub}
        >
            <Switch
                checked={powerPolicy === "cloud_on_battery"}
                onCheckedChange={(on) => {
                    // Turning it on can send a meeting to the cloud, so it
                    // needs an account exactly as the provider row does.
                    if (!on) onPowerPolicyChange("off");
                    else if (cloudAuth.requireSignedIn())
                        onPowerPolicyChange("cloud_on_battery");
                }}
            />
        </SettingRow>
    {/if}

    {#if cloudPossible}
        <SettingRow
            title={t.outOfHours.label}
            description={outOfHours === "disabled" ? disabledNote : ""}
        >
            <Select.Root
                type="single"
                value={outOfHours ?? "local"}
                onValueChange={(v) => {
                    if (v === "local" || v === "disabled") onOutOfHoursChange(v);
                }}
            >
                <Select.Trigger class="w-56"
                    >{outOfHours === "disabled"
                        ? t.outOfHours.disable
                        : t.outOfHours.switchToDevice}</Select.Trigger
                >
                <Select.Content>
                    <Select.Item
                        value="local"
                        label={t.outOfHours.switchToDevice}
                    />
                    <Select.Item
                        value="disabled"
                        label={t.outOfHours.disable}
                    />
                </Select.Content>
            </Select.Root>
        </SettingRow>
    {/if}
{/if}

<SettingRow title={t.language.label}>
    <Select.Root
        type="single"
        value={language}
        onValueChange={(v) => {
            if (v === "english" || v === "multilingual") onLanguageChange(v);
        }}
    >
        <Select.Trigger class="w-56"
            >{language === "multilingual"
                ? t.language.all
                : t.language.english}</Select.Trigger
        >
        <Select.Content>
            <Select.Item value="english" label={t.language.english} />
            <Select.Item value="multilingual" label={t.language.all} />
        </Select.Content>
    </Select.Root>
</SettingRow>

<!-- The accuracy tier is an English, on-device concept: multilingual has
     exactly one model (downloaded from the models page), and cloud-with-
     "disable" never touches the device. -->
{#if deviceTranscribes && language === "english"}
    <SpeechModelPicker
        value={accuracyModel}
        onChange={onAccuracyChange}
        {language}
    />
{/if}
