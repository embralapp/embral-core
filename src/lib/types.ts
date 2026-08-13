export interface TranscriptionSegment {
  speaker: string | null;
  // Registry link when this segment's speaker is a known person.
  speaker_id?: string | null;
  text: string;
  start: number;
  end: number;
}

/// Live in-flight utterance. `text` is the stable committed portion, while
/// `tentative_text` (when present) is an unstable trailing hypothesis that
/// may change on the next update; render it with reduced emphasis.
///
/// The tail's leading space is meaningful: present means it opens a new
/// word; absent means it continues `text`'s last word (the vendor splits
/// mid-word: "keep tal" + "king"). Concatenate verbatim; never insert a
/// separator.
export interface InterimSegment {
  speaker: string | null;
  text: string;
  start: number;
  end: number;
  tentative_text: string | null;
}

export interface MeetingRecord {
  id: string;
  title: string;
  date: string;
  duration_seconds: number;
  chunks: number;
  // The summary and transcript documents are database columns, not files,
  // so audio is the only path a meeting carries.
  audio_path: string;
}

export interface MeetingDetail {
  record: MeetingRecord;
  summary: string;
  transcript: string;
  audio_path: string | null;
  audio_exists: boolean;
  attendees: string[];
  // Structured transcript; empty for legacy meetings (raw editor fallback).
  segments: TranscriptionSegment[];
  /** Pending "Speaker N looks like X" suggestions from the user's notes. */
  name_suggestions: NameSuggestion[];
  /** User-starred moments. */
  stars: MeetingStar[];
  /** The user's raw live notes, verbatim (the Notes tab). */
  notes: string;
}

/** One starred moment: when, and (when known) which top-level block of the
 * user's notes it sits on. */
export interface MeetingStar {
  seconds: number;
  note_block: number | null;
}

// A pending notes-derived name suggestion awaiting approval.
export interface NameSuggestion {
  label: string;
  name: string;
}

// A registry person (mirrors src-tauri SpeakerProfile).
export interface SpeakerProfile {
  id: string;
  name: string;
  notes: string;
  created_at: string;
  /** The newest meeting they were in; null if never seen in one. The profiles
   * list sorts and groups on this, falling back to created_at. */
  last_seen: string | null;
}

// One meeting a person spoke in: a row of the profile pane's record
// (mirrors src-tauri SpeakerMeeting).
export interface SpeakerMeeting {
  meeting_id: string;
  title: string;
  started_at: string;
  segment_count: number;
}

// One transcript edit operation for edit_segments.
export type SegmentEdit =
  | { kind: 'split'; index: number; char_offset: number }
  | { kind: 'delete'; index: number }
  | { kind: 'reassign'; index: number; speaker: string; speaker_id?: string | null }
  | {
      kind: 'reassign_range';
      from_index: number;
      to_index: number;
      speaker: string;
      speaker_id?: string | null;
    }
  | { kind: 'relabel_all'; from: string; to: string; speaker_id?: string | null }
  | { kind: 'clear_label'; label: string };

// 'cloud' exists only in cloud-edition builds (embral's metered backend).
export type TranscriptionProvider = 'local' | 'cloud';

// The language transcription runs in. It sits above the provider choice
// and both providers read it. 'multilingual' means "detect it as it is
// spoken".
export type TranscriptionLanguage = 'english' | 'multilingual';

// What a cloud recording does when the account's hours (subscription plus
// purchased) run out. 'disabled' keeps recording and note-taking but writes
// no transcript. A dropped connection always falls back to the device.
export type CloudOutOfHours = 'local' | 'disabled';

// Whether the machine's power source overrides the meeting provider:
// 'cloud_on_battery' means cloud while on battery, this device while plugged
// in. Read once per recording, backend-side; the frontend only edits it.
export type PowerPolicy = 'off' | 'cloud_on_battery';

// One local speech model from the engine catalog (mirrors
// embral-engine::ModelStatus).
export interface ModelStatus {
  id: string;
  display_name: string;
  kind: 'streaming_asr' | 'offline_asr' | 'punctuation' | 'speaker_id' | 'llm' | 'embedding';
  note: string;
  // ISO codes, or ['*'] for language-independent models.
  languages: string[];
  present: boolean;
  total_bytes: number;
  dir: string;
  // Vocabulary boost availability (sherpa runtime limitation per model).
  supports_hotwords: boolean;
  // True when the model punctuates/cases natively (no punct model needed).
  native_punctuation: boolean;
}

// Byte-level progress for one model download (model-download-progress event).
export interface ModelProgress {
  model_id: string;
  file_name: string;
  downloaded_bytes: number;
  total_bytes: number;
}

// The palette's hybrid search results (search_library): meetings grouped
// best-passage-per-meeting, dictations alongside. Snippets carry [match]
// markers when the keyword leg produced them.
export interface LibraryMeetingHit {
  id: string;
  title: string;
  started_at: string;
  snippet: string;
  /** Which document matched; it names the tab to open. */
  source: ChunkSource;
  /** Where in the recording the passage runs, for transcript passages. */
  start_secs: number | null;
  end_secs: number | null;
  /** The passage's opening line, for finding it in a document. */
  lead: string;
  /** The image an `image_text` passage was read out of. */
  image: string | null;
}
/** Mirrors embral_search::chunker::Source. `dictation` never reaches a
 * meeting hit; it is here because the column's vocabulary is one list. */
export type ChunkSource = 'transcript' | 'user_notes' | 'summary' | 'dictation' | 'image_text';

/** Where in a meeting a search result was pointing, carried from the
 * palette to the detail pane, which opens the right tab and scrolls to the
 * passage instead of dumping the user at the top of the document. */
export interface PassageLanding {
  source: ChunkSource;
  /** Where the passage runs, for transcript hits. A passage is packed
   * paragraphs, so `start_secs` can be minutes before the line that
   * matched: it is the fallback, and the pair bounds the search for the
   * query so a word that recurs elsewhere cannot win. */
  start_secs: number | null;
  end_secs: number | null;
  /** The passage's opening line. Same caveat: a passage runs to 400 words,
   * so for a short document this is the first heading. Also a fallback. */
  lead: string;
  image: string | null;
  /** What the user actually typed. Tried first, because it is the thing
   * they asked to be taken to; `lead` and `start_secs` only say which
   * passage matched, which for a short summary is the whole document. */
  query: string;
}
export interface LibraryDictationHit {
  id: number;
  snippet: string;
  created_at: string;
}
export interface LibrarySearchResults {
  meetings: LibraryMeetingHit[];
  dictations: LibraryDictationHit[];
}

export type Theme = 'system' | 'light' | 'dark';
export type AutoStartPolicy = 'always' | 'selective' | 'prompt' | 'manual';
export type AutoStopScope = 'never' | 'auto_started' | 'all';
export type SilenceUnanswered = 'stop' | 'keep';
// Mirrors platform::types::PermissionState; 'not_required' on platforms
// that never gate the capability (Windows).
export type PermissionState = 'granted' | 'denied' | 'not_determined' | 'not_required';
export type DiarizationSensitivity = 'low' | 'medium' | 'high';
export type NotesNamingMode = 'off' | 'suggest' | 'automatic';
export type LlmProvider = 'builtin' | 'custom';
// The three documents a meeting carries. A meeting with no summary opens on
// notes whatever this says (its Summary tab doesn't exist).
export type OpenMeetingTab = 'summary' | 'notes' | 'transcript';
// 'cloud' exists only in cloud-edition builds. Cloud degrades to on-device
// while signed out; any failure delivers the raw text.
export type DictationCleanup = 'cloud' | 'on_device' | 'off';

// One synthesis engine (mirrors embral-types::LlmProfile). The list is
// fixed per edition; see utils/llmProfiles.ts availableProfiles().
export interface LlmProfile {
  id: string;
  name: string;
  provider: LlmProvider;
  model: string;
  endpoint: string;
  api_key: string;
}

export const BUILTIN_PROFILE_ID = 'builtin';
/** The cloud summaries engine's id (cloud builds only). */
export const CLOUD_PROFILE_ID = 'cloud';

// One saved dictation (mirrors embral-db::DictationRow).
export interface DictationRow {
  id: number;
  raw_text: string;
  cleaned_text: string | null;
  app: string | null;
  created_at: string;
}
export type ExportMetadataFormat = 'frontmatter' | 'inline';

export type WebhookMethod = 'post' | 'put';

/** One meeting-finished webhook destination (integrations.md §Webhooks). */
export interface WebhookDestination {
  url: string;
  method: WebhookMethod;
  /** The full content is included only when true; metadata-only otherwise. */
  include_content: boolean;
}

// Device names reported by list_audio_devices.
export interface AudioDevices {
  inputs: string[];
  outputs: string[];
}

export interface AppConfig {
  transcription_provider: TranscriptionProvider;
  transcription_language: TranscriptionLanguage;
  // Cloud edition only, absent from the offline build's config. Mirrored
  // (rather than left out) because save_config round-trips the whole object:
  // an unmirrored cfg-gated field would reset to its default on every save.
  transcription_power_policy?: PowerPolicy;
  cloud_out_of_hours?: CloudOutOfHours;
  // Cloud edition only: this device's session token; empty = signed out.
  // Read by the local-LLM usage rule, never written frontend-side.
  cloud_session_token?: string;
  storage_dir: string;
  retain_audio: boolean;
  // Local (on-device) transcription
  local_asr_model: string;
  vocabulary: string[];
  // Post-meeting integrations
  obsidian_export_enabled: boolean;
  obsidian_vault_dir: string;
  webhooks: WebhookDestination[];
  export_filename_template: string;
  export_metadata_format: ExportMetadataFormat;
  export_include_summary: boolean;
  export_include_notes: boolean;
  export_include_transcript: boolean;
  // Appearance & app behavior
  theme: Theme;
  /** Tray recording-disc color as #RRGGBB; empty = Windows accent color. */
  tray_recording_color: string;
  mic_device: string;
  output_device: string;
  notify_summary_ready: boolean;
  notify_recording_started: boolean;
  notify_update_available: boolean;
  audio_retention_days: number;
  meeting_retention_days: number;
  onboarding_completed: boolean;
  // Telemetry (telemetry.md), cloud edition only, absent from the offline
  // build's config: opt-in flag, per-install id (created on enable, cleared
  // on opt-out), and the daily config_snapshot date gate.
  telemetry_enabled?: boolean;
  telemetry_install_id?: string;
  telemetry_last_snapshot?: string;
  // Meeting detection & automation
  auto_start_policy: AutoStartPolicy;
  auto_detect_apps: string[];
  detection_delay_secs: number;
  auto_stop: AutoStopScope;
  silence_stop_minutes: number;
  silence_stop_unanswered: SilenceUnanswered;
  notify_call_detected: boolean;
  record_hotkey: string;
  sidebar_expanded: boolean;
  // Speakers
  diarization_enabled: boolean;
  diarization_sensitivity: DiarizationSensitivity;
  notes_naming_mode: NotesNamingMode;
  // Synthesis
  summaries_enabled: boolean;
  // The engine: "builtin", or "cloud" in cloud builds. Only consulted while
  // summaries_enabled.
  summaries_profile_id: string;
  // Full replacement prompt body; "" = built-in default. The locked output
  // contract is appended backend-side either way.
  summary_prompt: string;
  open_meeting_tab: OpenMeetingTab;
  llm_keep_warm: boolean;
  llm_idle_minutes: number;
  // Dictation: its own transcription tree, independent of meetings.
  dictation_hotkey: string;
  dictation_provider: TranscriptionProvider;
  // Cloud edition only, absent from the offline build's config.
  dictation_out_of_hours?: CloudOutOfHours;
  dictation_language: TranscriptionLanguage;
  dictation_asr_model: string;
  dictation_cleanup: DictationCleanup;
  dictation_copy_clipboard: boolean;
  dictation_auto_paste: boolean;
  dictation_auto_delete: boolean;
  dictation_retention_days: number;
  dictation_retention_count: number;
}
