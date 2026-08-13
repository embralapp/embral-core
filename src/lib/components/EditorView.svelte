<script lang="ts">
  import { onMount } from 'svelte';
  import type { MeetingStar } from '$lib/types';
  import MarkdownEditor from './MarkdownEditor.svelte';

  /** One meeting document on screen: summary, notes, or the raw transcript
   * fallback. All three are markdown over the same editor; what differs is
   * whether they are editable and whether stars show in the gutter, so both
   * are props rather than three near-identical components.
   *
   * `value` is deliberately not bindable: the parent owns the draft and
   * hears about edits through `onChange`. Binding would let a programmatic
   * transaction (star anchoring on mount, say) push the editor's
   * re-serialized markdown back over the caller's state. */
  let {
    value,
    onChange,
    readonly = false,
    stars = [],
    onStarClick,
    placeholder = '',
    autofocus = false,
    pasteMeetingId,
    onPasteError
  }: {
    value: string;
    /** Absent = a display surface. Present = the parent saves what it gets. */
    onChange?: (value: string) => void;
    readonly?: boolean;
    /** Starred moments to re-anchor in the gutter; empty = no star gutter. */
    stars?: MeetingStar[];
    onStarClick?: (star: MeetingStar) => void;
    placeholder?: string;
    autofocus?: boolean;
    /** Enables image paste; see `MarkdownEditor`. */
    pasteMeetingId?: () => string | undefined;
    onPasteError?: (message: string) => void;
  } = $props();

  let editorRef = $state<MarkdownEditor | null>(null);

  // A star's id here is its index in the meeting's list, which is what
  // `scrollToStar` and the click handler below both use.
  onMount(() => {
    stars.forEach((star, i) => {
      if (star.note_block !== null) {
        editorRef?.addStarAtBlock(i, star.note_block);
      }
    });
  });

  /** Scroll a star's line into view (player star clicks). */
  export function scrollToStar(index: number) {
    editorRef?.scrollToStar(index);
  }

  /** Land inside the passage a search result matched; see `MarkdownEditor`. */
  export function scrollToPassage(query: string, lead: string): boolean {
    return editorRef?.scrollToPassage(query, lead) ?? false;
  }

  /** Land on a pasted image by filename (an `image_text` result). */
  export function scrollToImage(filename: string): boolean {
    return editorRef?.scrollToImage(filename) ?? false;
  }

  /** Whether there is a document to look in yet; see `MarkdownEditor`. */
  export function isReady(): boolean {
    return editorRef?.isReady() ?? false;
  }

  /** Where each star sits in the document now. Stars anchor as node
   * attributes, so ProseMirror carries them through inserts and deletes for
   * free; this reads the resulting ordinals back out so a save can persist
   * them instead of leaving the stored ones to drift. */
  export function currentStars(): MeetingStar[] {
    const blocks = editorRef?.getStarBlocks() ?? new Map<number, number>();
    return stars.map((star, i) => ({
      ...star,
      note_block: blocks.has(i) ? (blocks.get(i) as number) : star.note_block
    }));
  }
</script>

<MarkdownEditor
  bind:this={editorRef}
  {value}
  {readonly}
  {placeholder}
  {autofocus}
  {onChange}
  {pasteMeetingId}
  {onPasteError}
  onStarClick={onStarClick
    ? (id) => {
        const star = stars[id];
        if (star) onStarClick(star);
      }
    : undefined}
/>
