<script lang="ts">
    import { Trash2, ChevronRight } from "lucide-svelte";
    import { invoke } from "@tauri-apps/api/core";
    import type {
        SpeakerProfile,
        SpeakerMeeting,
        TranscriptionSegment,
    } from "$lib/types";
    import { speakersStore } from "$lib/stores/speakers.svelte";
    import { meetingsStore } from "$lib/stores/meetings.svelte";
    import { appState } from "$lib/stores/app-state.svelte";
    import { formatMeetingDate, formatDuration } from "$lib/utils/meetingFormat";
    import { Button } from "$lib/components/ui/button";
    import { Input } from "$lib/components/ui/input";
    import { Textarea } from "$lib/components/ui/textarea";
    import { Label } from "$lib/components/ui/label";
    import { copy } from "$lib/copy";

    const t = $derived(copy.speakers.profile);

    let {
        speaker,
        onSaved,
        onDeleted,
    }: {
        speaker: SpeakerProfile | null;
        onSaved: (id: string) => void;
        onDeleted?: () => void;
    } = $props();

    let name = $state("");
    let notes = $state("");
    let loadedId = $state<string | null>(null);
    let confirmDelete = $state(false);

    // The record: the meetings this person spoke in, and (fetched when a
    // meeting is expanded) what they said in it.
    let record = $state<SpeakerMeeting[]>([]);
    let expanded = $state<string[]>([]);
    let lines = $state<Record<string, TranscriptionSegment[]>>({});

    // Load drafts whenever a different person is shown (null = new person).
    $effect(() => {
        const key = speaker?.id ?? "__new__";
        if (loadedId !== key) {
            loadedId = key;
            name = speaker?.name ?? "";
            notes = speaker?.notes ?? "";
            confirmDelete = false;
            record = [];
            expanded = [];
            lines = {};
            if (speaker) void loadRecord(speaker.id);
        }
    });

    async function loadRecord(id: string) {
        try {
            record = await invoke<SpeakerMeeting[]>("speaker_meetings", { id });
        } catch {
            record = [];
        }
    }

    async function toggleMeeting(meetingId: string) {
        if (expanded.includes(meetingId)) {
            expanded = expanded.filter((m) => m !== meetingId);
            return;
        }
        expanded = [...expanded, meetingId];
        if (!lines[meetingId] && speaker) {
            try {
                const segs = await invoke<TranscriptionSegment[]>(
                    "speaker_segments",
                    { id: speaker.id, meetingId },
                );
                lines = { ...lines, [meetingId]: segs };
            } catch {
                lines = { ...lines, [meetingId]: [] };
            }
        }
    }

    /** A line takes the reader to itself in the transcript: the same
     * landing a palette search result takes. */
    function jumpTo(meetingId: string, seg: TranscriptionSegment) {
        appState.setView("idle");
        void meetingsStore.select(meetingId, {
            source: "transcript",
            start_secs: seg.start,
            end_secs: seg.end,
            lead: seg.text,
            image: null,
            query: seg.text,
        });
    }

    let dirty = $derived(
        speaker === null
            ? name.trim().length > 0
            : name !== speaker.name || notes !== speaker.notes,
    );

    async function save() {
        const saved = await speakersStore.save({
            id: speaker?.id,
            name: name.trim(),
            notes,
        });
        if (saved) onSaved(saved.id);
    }

    async function remove() {
        if (!speaker) return;
        if (!confirmDelete) {
            confirmDelete = true;
            return;
        }
        await speakersStore.remove(speaker.id);
        onDeleted?.();
    }
</script>

<div class="mx-auto max-w-xl p-6">
    <h2 class="font-display text-lg tracking-tight">
        {speaker ? speaker.name : t.newTitle}
    </h2>

    <div class="mt-5 space-y-4">
        <div class="space-y-1.5">
            <Label for="sp-name">{t.name}</Label>
            <Input id="sp-name" bind:value={name} placeholder={t.namePlaceholder} />
        </div>
        <div class="space-y-1.5">
            <Label for="sp-notes">{t.notes}</Label>
            <Textarea
                id="sp-notes"
                bind:value={notes}
                rows={3}
                placeholder={t.notesPlaceholder}
            />
        </div>

        <div class="flex items-center gap-2">
            <Button size="sm" onclick={save} disabled={!dirty || !name.trim()}>
                {speaker ? t.save : t.create}
            </Button>
            {#if speakersStore.error}
                <p class="text-xs text-destructive">{speakersStore.error}</p>
            {/if}
        </div>
    </div>

    {#if speaker}
        <div class="mt-8">
            <h3
                class="text-[11px] font-medium tracking-wide text-muted-foreground/80 uppercase"
            >
                {t.meetings}
            </h3>
            {#if record.length === 0}
                <p class="mt-2 text-sm text-muted-foreground">{t.noMeetings}</p>
            {:else}
                <div class="mt-1.5">
                    {#each record as m (m.meeting_id)}
                        <div>
                            <button
                                onclick={() => toggleMeeting(m.meeting_id)}
                                class="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left transition-colors hover:bg-accent/40"
                            >
                                <ChevronRight
                                    size={13}
                                    class={"shrink-0 text-muted-foreground transition-transform " +
                                        (expanded.includes(m.meeting_id) ? "rotate-90" : "")}
                                />
                                <span class="font-display min-w-0 flex-1 truncate text-sm"
                                    >{m.title}</span
                                >
                                <span class="shrink-0 text-[11px] text-muted-foreground"
                                    >{t.lines(m.segment_count)}</span
                                >
                                <span class="shrink-0 text-[11px] text-muted-foreground"
                                    >{formatMeetingDate(m.started_at)}</span
                                >
                            </button>
                            {#if expanded.includes(m.meeting_id)}
                                <div class="mb-1 ml-[18px] space-y-0.5 border-l border-border pl-3">
                                    {#each lines[m.meeting_id] ?? [] as seg, i (i)}
                                        <button
                                            onclick={() => jumpTo(m.meeting_id, seg)}
                                            class="block w-full rounded px-1.5 py-1 text-left transition-colors hover:bg-accent/40"
                                        >
                                            <p class="line-clamp-2 text-sm leading-snug">
                                                <span
                                                    class="mr-1 text-[11px] tabular-nums text-muted-foreground"
                                                    >{formatDuration(seg.start)}</span
                                                >{seg.text}
                                            </p>
                                        </button>
                                    {/each}
                                </div>
                            {/if}
                        </div>
                    {/each}
                </div>
            {/if}
        </div>

        <div class="mt-8 border-t border-border pt-4">
            <Button size="sm" variant="ghost" class="text-destructive" onclick={remove}>
                <Trash2 size={14} class="mr-1" />
                {confirmDelete ? t.reallyDelete : t.delete(speaker.name)}
            </Button>
            <p class="mt-1 text-xs text-muted-foreground">
                {t.deleteNote}
            </p>
        </div>
    {/if}
</div>
