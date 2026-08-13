// Maps what the machine can carry (the `system_specs` command) to the model
// bundle onboarding recommends ([transcription.md](../../../../docs/transcription.md)).
// Pure: the thresholds are the policy, the command only reports hardware.

import type { ModelStatus, TranscriptionLanguage } from "$lib/types";
import {
    ASR_ACCURATE,
    ASR_BALANCED,
    ASR_FAST,
    LLM_RUNTIME,
    LLM_WEIGHTS,
    MULTILINGUAL_ASR_MODEL,
} from "$lib/utils/asrModel";

export interface SystemSpecs {
    total_ram_bytes: number;
    logical_cores: number;
    /** 0 = unknown; the disk check is skipped rather than guessed. */
    free_disk_bytes: number;
}

export interface Recommendation {
    /** The recommended ASR tier's model id. */
    asr: string;
    /** Zipformers need the punctuation pass; parakeet punctuates natively. */
    punct: boolean;
    /** Whether the built-in LLM pair (runtime + weights) is recommended. */
    llm: boolean;
}

const GB = 1024 * 1024 * 1024;

/* The model ids the recommendation composes from live in
 * `$lib/utils/asrModel`, re-exported here so onboarding keeps one import. */
export { ASR_ACCURATE, ASR_BALANCED, ASR_FAST, LLM_RUNTIME, LLM_WEIGHTS };
/** The one model behind the Multilingual language choice. */
export const ASR_MULTILINGUAL = MULTILINGUAL_ASR_MODEL;
export const PUNCTUATION = "punct-en";
export const SPEAKER_ID = "speaker-id";
export const EMBEDDING = "embedding-multilingual";

export function recommend(specs: SystemSpecs): Recommendation {
    // RAM is the only gate. Core counts don't compare across vendors (an
    // 8-core AMD part outruns a 12-thread hybrid Intel one whose count is
    // padded by efficiency cores), while RAM is an honest floor: the
    // accurate model has to fit beside Zoom and a browser, and any machine
    // sold with 16 GB decodes it faster than realtime. The small model is
    // for machines the catalog itself calls "older".
    const asr =
        specs.total_ram_bytes >= 16 * GB
            ? ASR_ACCURATE
            : specs.total_ram_bytes >= 8 * GB
              ? ASR_BALANCED
              : ASR_FAST;
    return {
        asr,
        punct: asr !== ASR_ACCURATE,
        // Qwen3-4B holds a ~3 GB working set; below 16 GB that ends up in the
        // pagefile next to the meeting app it runs beside.
        llm: specs.total_ram_bytes >= 16 * GB,
    };
}

/** English unless the system language (a BCP-47 tag like "de-DE") says
 * otherwise; an empty or malformed tag keeps the shipped default. */
export function recommendedLanguage(tag: string): TranscriptionLanguage {
    const lang = tag.trim().toLowerCase();
    return lang === "" || lang === "en" || lang.startsWith("en-")
        ? "english"
        : "multilingual";
}

/** Every model id in the recommended bundle (speaker-id and semantic search
 * are always in: cheap, and diarization/search default on). */
export function recommendedIds(rec: Recommendation): string[] {
    return [
        rec.asr,
        ...(rec.punct ? [PUNCTUATION] : []),
        ...(rec.llm ? [LLM_RUNTIME, LLM_WEIGHTS] : []),
        SPEAKER_ID,
        EMBEDDING,
    ];
}

/** Summed download size of the not-yet-present models in `ids`. */
export function missingBytes(ids: string[], statuses: ModelStatus[]): number {
    return ids.reduce((sum, id) => {
        const m = statuses.find((s) => s.id === id);
        return m && !m.present ? sum + m.total_bytes : sum;
    }, 0);
}

/** Non-blocking warning: the selected downloads plus 2 GB headroom don't fit.
 * Unknown disk (0) never warns. */
export function diskWarning(specs: SystemSpecs, selectedBytes: number): boolean {
    return (
        specs.free_disk_bytes > 0 &&
        specs.free_disk_bytes < selectedBytes + 2 * GB
    );
}
