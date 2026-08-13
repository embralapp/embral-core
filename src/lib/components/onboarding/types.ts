// The wizard's draft: only the fields onboarding owns, applied over a
// freshly-loaded config at finish so anything the cloud-only code changed
// mid-wizard (provider adoption on sign-in) survives ([shell.md](../../../../docs/shell.md)).

import type { AppConfig } from "$lib/types";

export type OnboardingDraft = Pick<
    AppConfig,
    | "local_asr_model"
    | "transcription_language"
    | "auto_start_policy"
    | "summaries_enabled"
    | "summaries_profile_id"
    | "record_hotkey"
    | "dictation_hotkey"
    | "dictation_cleanup"
    | "dictation_copy_clipboard"
    | "dictation_auto_paste"
    | "obsidian_export_enabled"
    | "obsidian_vault_dir"
    | "export_include_summary"
    | "export_include_notes"
    | "export_include_transcript"
>;

export function draftFrom(config: AppConfig): OnboardingDraft {
    return {
        local_asr_model: config.local_asr_model,
        transcription_language: config.transcription_language,
        auto_start_policy: config.auto_start_policy,
        summaries_enabled: config.summaries_enabled,
        summaries_profile_id: config.summaries_profile_id,
        record_hotkey: config.record_hotkey,
        dictation_hotkey: config.dictation_hotkey,
        dictation_cleanup: config.dictation_cleanup,
        dictation_copy_clipboard: config.dictation_copy_clipboard,
        dictation_auto_paste: config.dictation_auto_paste,
        obsidian_export_enabled: config.obsidian_export_enabled,
        obsidian_vault_dir: config.obsidian_vault_dir,
        export_include_summary: config.export_include_summary,
        export_include_notes: config.export_include_notes,
        export_include_transcript: config.export_include_transcript,
    };
}
