<script lang="ts">
  import { convertFileSrc } from '@tauri-apps/api/core';
  import { isLinux } from '$lib/platform';
  import { Pause, Play, Star } from 'lucide-svelte';
  import type { MeetingStar } from '$lib/types';
  import { formatTime } from '$lib/utils/meetingFormat';
  import Tip from './Tip.svelte';
  import { copy } from '$lib/copy';

  const t = $derived(copy.meetings.player);

  let {
    audioPath = null,
    stars = [],
    currentTime = $bindable(0),
    playing = $bindable(false),
    onStarActivate
  }: {
    audioPath?: string | null;
    /** User-starred moments; rendered as timeline ticks + chips. */
    stars?: MeetingStar[];
    currentTime?: number;
    playing?: boolean;
    /** Fired after a star tick/chip seeks, so the active tab can scroll
     * to the star's notes line or transcript position. */
    onStarActivate?: (star: MeetingStar, index: number) => void;
  } = $props();

  let audioEl = $state<HTMLAudioElement | null>(null);
  let trackEl = $state<HTMLDivElement | null>(null);
  let duration = $state(0);
  let loadError = $state<string | null>(null);
  let dragging = $state(false);
  // Cursor position over the track, for the time tooltip.
  let hoverRatio = $state<number | null>(null);

  const assetUrl = $derived(audioPath ? convertFileSrc(audioPath) : null);

  // On Linux the asset URL cannot feed a media element directly. WebKitGTK
  // plays media through GStreamer, which does not go through WebKit's custom
  // URI scheme handlers. Measured in the live webview: the same file errors
  // with MEDIA_ERR_SRC_NOT_SUPPORTED as `asset://localhost/…` and plays as a
  // blob URL, while `fetch()` of the asset URL works either way. Fetching the
  // bytes ourselves and handing over a blob sidesteps the media path entirely.
  // (The blob keeps whatever Content-Type the asset protocol sent; GStreamer
  // typefinds from the bytes, so the type does not matter.)
  //
  // Only Linux pays for it: Windows and macOS keep the streaming asset URL, so
  // nothing there starts buffering whole meetings into memory. The blob is
  // revoked when the path changes or the player goes away.
  let blobUrl = $state<string | null>(null);

  $effect(() => {
    if (!isLinux || !assetUrl) return;
    let revoked = false;
    let created: string | null = null;
    void (async () => {
      try {
        const res = await fetch(assetUrl);
        if (!res.ok) throw new Error(`asset fetch failed: ${res.status}`);
        created = URL.createObjectURL(await res.blob());
        if (revoked) {
          URL.revokeObjectURL(created);
          return;
        }
        blobUrl = created;
      } catch (e) {
        // Leave `blobUrl` null: `src` stays null on Linux, so the player
        // renders its "no audio" state rather than a broken element.
        console.error('audio blob failed', e);
      }
    })();
    return () => {
      revoked = true;
      if (created) URL.revokeObjectURL(created);
      blobUrl = null;
    };
  });

  const src = $derived(isLinux ? blobUrl : assetUrl);
  const progress = $derived(duration > 0 ? (currentTime / duration) * 100 : 0);

  function activateStar(star: MeetingStar, index: number) {
    void seekTo(star.seconds);
    onStarActivate?.(star, index);
  }


  $effect(() => {
    src;
    playing = false;
    currentTime = 0;
    duration = 0;
    loadError = null;
    if (audioEl && src) {
      audioEl.load();
    }
  });

  /** Play/pause: the play button and the Space key. */
  export async function toggle() {
    if (!audioEl) return;
    if (audioEl.paused) {
      try {
        await audioEl.play();
        playing = true;
        loadError = null;
      } catch (e) {
        loadError = e instanceof Error ? e.message : t.errors.couldNotPlay;
        console.error('Failed to play meeting audio:', e, { audioPath, src });
      }
    } else {
      audioEl.pause();
      playing = false;
    }
  }

  /** Jump to an absolute offset and start playing: chapter and transcript
   * clicks. */
  export async function seekTo(seconds: number) {
    if (!audioEl) return;
    audioEl.currentTime = Math.max(0, seconds);
    currentTime = audioEl.currentTime;
    if (audioEl.paused) {
      try {
        await audioEl.play();
        playing = true;
      } catch {
        // Playback refusal (no src yet) is non-fatal.
      }
    }
  }

  /** Relative skip (arrow keys), without changing play state. */
  export function skip(deltaSeconds: number) {
    if (!audioEl || duration <= 0) return;
    audioEl.currentTime = Math.min(duration, Math.max(0, audioEl.currentTime + deltaSeconds));
    currentTime = audioEl.currentTime;
  }

  function onTimeUpdate() {
    currentTime = audioEl?.currentTime ?? 0;
  }

  function onLoadedMetadata() {
    const loaded = audioEl?.duration;
    duration = loaded != null && Number.isFinite(loaded) ? loaded : 0;
    loadError = null;
  }

  function onEnded() {
    playing = false;
    currentTime = 0;
  }

  function ratioAt(e: PointerEvent | MouseEvent): number {
    if (!trackEl) return 0;
    const rect = trackEl.getBoundingClientRect();
    if (rect.width <= 0) return 0;
    return Math.min(1, Math.max(0, (e.clientX - rect.left) / rect.width));
  }

  function scrubTo(e: PointerEvent) {
    if (!audioEl || duration <= 0) return;
    const next = ratioAt(e) * duration;
    audioEl.currentTime = next;
    currentTime = next;
  }

  function onTrackPointerDown(e: PointerEvent) {
    if (duration <= 0) return;
    dragging = true;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    scrubTo(e);
  }

  function onTrackPointerMove(e: PointerEvent) {
    hoverRatio = ratioAt(e);
    if (dragging) scrubTo(e);
  }

  function onTrackPointerUp(e: PointerEvent) {
    dragging = false;
    (e.currentTarget as HTMLElement).releasePointerCapture(e.pointerId);
  }

  function onError() {
    const code = audioEl?.error?.code;
    const reason =
      code === MediaError.MEDIA_ERR_ABORTED
        ? t.errors.aborted
        : code === MediaError.MEDIA_ERR_NETWORK
          ? t.errors.network
          : code === MediaError.MEDIA_ERR_DECODE
            ? t.errors.decode
            : code === MediaError.MEDIA_ERR_SRC_NOT_SUPPORTED
              ? t.errors.unsupported
              : t.errors.network;
    loadError = reason;
    console.error('Meeting audio failed to load:', {
      reason,
      code,
      audioPath,
      src,
      networkState: audioEl?.networkState,
      readyState: audioEl?.readyState
    });
  }
</script>

<div class="relative shrink-0 border-t border-border px-4 py-2.5">
  {#if src}
    <audio
      bind:this={audioEl}
      {src}
      preload="metadata"
      ontimeupdate={onTimeUpdate}
      onloadedmetadata={onLoadedMetadata}
      onerror={onError}
      onended={onEnded}
    ></audio>

    <div class="flex items-center gap-3">
      <!-- The app has one transport vocabulary, set by the live recording
           header: a ghost icon button. Playback is the same kind of action, so
           it is the same control, at full contrast rather than muted, because
           it is this pane's primary action. Ranking by contrast is how this app
           ranks things; a filled circle belongs to a different one. -->
      <Tip text={playing ? t.pause : t.play}>
        {#snippet children({ props })}
          <button
            {...props}
            onclick={toggle}
            class="shrink-0 rounded-md p-2 text-foreground transition-colors hover:bg-accent"
            aria-label={playing ? t.pauseAria : t.playAria}
          >
            {#if playing}
              <Pause size={16} />
            {:else}
              <!-- Optically centred with a transform, not a margin: the button
                   has no fixed width, so a margin would make it a pixel wider
                   than the pause state and the control would twitch on every
                   toggle. -->
              <Play size={16} class="translate-x-px" />
            {/if}
          </button>
        {/snippet}
      </Tip>

      <!-- The timeline: filled progress, a drag handle, and the starred moments. -->
      <!-- `outline-none`: clicking the track focuses it (it is `tabindex="-1"`,
           so it is click-focusable but never keyboard-reachable), and the app's
           global focus ring then drew a hard outline around the whole bar, most
           visibly when Space toggled playback with the track still focused.
           There is no focus state worth showing on something the keyboard cannot
           reach. -->
      <div
        bind:this={trackEl}
        class="group relative min-w-0 flex-1 cursor-pointer py-2 outline-none"
        onpointerdown={onTrackPointerDown}
        onpointermove={onTrackPointerMove}
        onpointerup={onTrackPointerUp}
        onpointerleave={() => (hoverRatio = null)}
        role="slider"
        aria-label={t.position}
        aria-valuemin={0}
        aria-valuemax={Math.floor(duration)}
        aria-valuenow={Math.floor(currentTime)}
        tabindex="-1"
      >
        <div class="relative h-1.5 overflow-visible rounded-full bg-muted">
          <div
            class="absolute inset-y-0 left-0 rounded-full bg-foreground/70"
            style="width: {progress}%"
          ></div>
          <!-- The stars are the timeline's marks, sitting on the line itself.
               A star has to read on both halves of that line (the played part
               is near-`foreground`, the rest is `muted`), so it carries both
               colours and swaps them as the playhead passes: ahead of
               the playhead it is muted-filled with a foreground outline (light
               on the dark track), behind it the two trade places (dark on the
               light fill). Either way one colour is the track's own, so the star
               always has an edge against it, and the pair inverts together
               between themes. The boundary is `progress` itself, so the swap
               happens exactly as the playhead crosses.

               Clicking a star seeks (and scrolls the active tab); its
               pointerdown stops there, or the track beneath would also start a
               scrub and seek to the pointer instead of to the star. -->
          {#each stars as star, i (i)}
            {#if duration > 0}
              {@const passed = star.seconds <= currentTime}
              <!-- No focus ring: a hard outline round a 13px star on a 6px line
                   is all ring and no star. These are keyboard-reachable
                   though (unlike the track), so the focus state borrows the
                   hover scale rather than disappearing entirely. -->
              <Tip text={t.playFrom(formatTime(star.seconds))}>
                {#snippet children({ props })}
                  <button
                    {...props}
                    class="absolute top-1/2 -translate-x-1/2 -translate-y-1/2 outline-none transition-transform hover:scale-115 focus-visible:scale-115"
                    style="left: {(star.seconds / duration) * 100}%"
                    aria-label={t.playFrom(formatTime(star.seconds))}
                    onpointerdown={(e) => {
                      e.stopPropagation();
                      activateStar(star, i);
                    }}
                  >
                    <!-- `strokeWidth` is in the icon's 24-unit viewBox, not
                         pixels: it renders at `size/24 × strokeWidth`. -->
                    <Star
                      size={13}
                      fill="var(--foreground)"
                      color={passed ? 'var(--muted)' : 'var(--foreground)'}
                      strokeWidth={passed ? 3 : 0}
                    />
                  </button>
                {/snippet}
              </Tip>
            {/if}
          {/each}
          <div
            class="absolute top-1/2 h-3 w-3 -translate-x-1/2 -translate-y-1/2 rounded-full bg-foreground shadow-sm transition-opacity {dragging
              ? 'opacity-100'
              : 'opacity-0 group-hover:opacity-100'}"
            style="left: {progress}%"
          ></div>
        </div>

        {#if hoverRatio !== null && duration > 0 && !dragging}
          <div
            class="pointer-events-none absolute -top-5 -translate-x-1/2 rounded border border-border bg-popover px-1.5 py-0.5 font-mono text-[10px] tabular-nums text-popover-foreground shadow-sm"
            style="left: {hoverRatio * 100}%"
          >
            {formatTime(hoverRatio * duration)}
          </div>
        {/if}
      </div>

      <span class="w-20 shrink-0 text-right font-mono text-[11px] tabular-nums text-muted-foreground">
        {formatTime(currentTime)} / {formatTime(duration)}
      </span>
    </div>

    {#if loadError}
      <p class="mt-1.5 text-[11px] text-destructive">{loadError}</p>
    {/if}
  {:else}
    <p class="text-xs text-muted-foreground">{t.noAudio}</p>
  {/if}
</div>
