<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { Editor } from '@tiptap/core';
  import { markdownExtensions } from '$lib/editor/extensions';
  import { imagePaste } from '$lib/editor/imagePaste';
  import { findImagePos, findTextPos } from '$lib/editor/locate';
  import { flashNodeAt, landingFlash } from '$lib/editor/landingFlash';
  import { afterScrollTo } from '$lib/utils/flash';
  import { storageRoot } from '$lib/stores/storageRoot.svelte';
  import { toDisplaySrc } from '$lib/editor/assetSrc';
  import { copy } from '$lib/copy';
  import {
    starAnchors,
    starAtCursor as starAtCursorIn,
    anchorStarAtCursor,
    clearStarAnchor,
    starBlockIndexes,
    anchorStarAtBlock
  } from '$lib/editor/starGutter';

  let {
    value = $bindable(''),
    placeholder = '',
    autofocus = false,
    readonly = false,
    onChange,
    onStarClick,
    pasteMeetingId,
    onPasteError
  }: {
    value?: string;
    placeholder?: string;
    autofocus?: boolean;
    /** A display surface (the saved user-notes view): no editing. */
    readonly?: boolean;
    onChange?: (value: string) => void;
    /** Enables the star gutter: stars anchor to lines and clicking one
     * calls this with the star's id (remove while recording, seek in the
     * saved-notes view). */
    onStarClick?: (id: number) => void;
    /** Enables image paste. Returns the meeting the document belongs to;
     * `undefined` means "the recording happening now", which the backend
     * resolves for itself. Absent prop = pasting an image does nothing. */
    pasteMeetingId?: () => string | undefined;
    /** How this surface reports a failed image paste. */
    onPasteError?: (message: string) => void;
  } = $props();

  let editorEl: HTMLDivElement;
  let editor: Editor | undefined;
  let applyingExternalValue = false;

  const isEmpty = $derived(value.trim().length === 0);

  // The full-size viewer, opened by clicking any image on this surface
  // ([shell.md] §Writing surface). Display src only — the stored form is
  // storage-relative and no webview can load it.
  let lightbox = $state<{ src: string; alt: string } | null>(null);

  function onLightboxKeydown(e: KeyboardEvent) {
    if (!lightbox) return;
    if (e.key === 'Escape') {
      e.preventDefault();
      lightbox = null;
    }
  }

  // One writing surface everywhere (live notes, saved notes, the raw
  // transcript fallback): borderless — the pane *is* the editor — with
  // flat, small margins, and text that fills the pane's width (owner
  // call: no measure cap, a wide window gives wide lines). No centering
  // math, which also guarantees the placeholder can't drift — `ch`-based
  // padding resolved differently on the two elements.
  const surfacePadding = 'padding: 1.25rem 1.5rem 3rem;';

  onMount(() => {
    const extensions = [
      ...markdownExtensions(storageRoot.value),
      landingFlash(),
      ...(onStarClick ? [starAnchors(onStarClick)] : []),
      ...(pasteMeetingId && !readonly
        ? [imagePaste({ meetingId: pasteMeetingId, onError: onPasteError })]
        : [])
    ];
    editor = new Editor({
      element: editorEl,
      extensions,
      content: value,
      editable: !readonly,
      autofocus: autofocus ? 'end' : false,
      editorProps: {
        // The caret keeps breathing room while typing: the view starts
        // following ~2 lines before the caret reaches the edge and lands
        // it with ~2 line boxes of air (24.75px lines), instead of
        // ProseMirror's default flush-to-the-edge scroll.
        scrollThreshold: 40,
        scrollMargin: 56,
        // A click on an image opens the viewer — on every surface, the
        // readonly ones included (mousedown is not an edit handler, so
        // this fires with editable off). Returning false leaves
        // ProseMirror's own node selection in place, which is how an
        // editable surface still deletes an image with Backspace.
        handleClickOn: (_view, _pos, node) => {
          if (node.type.name === 'image' && node.attrs.src) {
            lightbox = {
              src: toDisplaySrc(storageRoot.value, node.attrs.src as string),
              alt: (node.attrs.alt as string) ?? ''
            };
          }
          return false;
        },
        attributes: {
          class: `note-prose h-full overflow-y-auto focus:outline-none${
            readonly ? ' note-prose-readonly' : ''
          }`,
          style: surfacePadding
        }
      },
      onUpdate: ({ editor, transaction }) => {
        // Save only what the user actually did. `onUpdate` fires on *any*
        // transaction, including programmatic ones — star anchoring on
        // mount, most notably — and a save triggered by one of those writes
        // the editor's re-serialized markdown over the stored document
        // without anybody typing. That is how merely opening a tab could
        // destroy something the schema can't model. `docChanged` rules out
        // selection-only transactions; `addToHistory: false` is the flag
        // our own programmatic edits already carry (`starGutter.ts`).
        const programmatic = transaction.getMeta('addToHistory') === false;
        if (applyingExternalValue || programmatic || !transaction.docChanged) return;
        const next = editor.storage.markdown.getMarkdown();
        value = next;
        onChange?.(next);
      }
    });
    reportFidelity(value, editor.storage.markdown.getMarkdown());
  });

  /** Tripwire: what came in should survive a parse and re-serialize. When it
   * doesn't, this editor's schema cannot represent something in the
   * document, and editing here would save the loss (see
   * `editor/extensions.ts`). Once the contract is complete this never
   * fires — it exists to name whatever we failed to anticipate, in the log,
   * with the text that went missing. */
  function reportFidelity(incoming: string, reparsed: string) {
    if (incoming.trim().length === 0) return;
    // Normalization is expected and fine (bullet markers, spacing, escapes),
    // so compare what markdown is *about*: links, images, and table rows.
    const shapes = [/!\[[^\]]*\]\([^)]*\)/g, /(?<!!)\[[^\]]*\]\([^)]*\)/g, /^\|.*\|$/gm];
    for (const shape of shapes) {
      const before = incoming.match(shape)?.length ?? 0;
      const after = reparsed.match(shape)?.length ?? 0;
      if (after < before) {
        console.warn(
          `[embral] the editor cannot represent this document: ${before - after} ` +
            `of ${before} ${shape.source} dropped on load. Editing it would save the loss.`
        );
        return;
      }
    }
  }

  $effect(() => {
    if (!editor) return;
    const current = editor.storage.markdown.getMarkdown();
    if (current === value) return;
    applyingExternalValue = true;
    editor.commands.setContent(value, false);
    applyingExternalValue = false;
  });

  /** The star already on the caret's line, if any (Ctrl+S toggles it). */
  export function starAtCursor(): number | null {
    return editor ? starAtCursorIn(editor) : null;
  }

  /** Anchor a gutter star on the caret's line (the last line when the
   * editor isn't focused — the user may be mid-call elsewhere). */
  export function addStar(id: number) {
    if (editor) anchorStarAtCursor(editor, id);
  }

  /** Drop a gutter star's anchor (store/backend removal happens upstream). */
  export function removeStar(id: number) {
    if (editor) clearStarAnchor(editor, id);
  }

  /** The textblock index each star currently sits on — persisted at stop
   * so the saved notes can re-anchor them. */
  export function getStarBlocks(): Map<number, number> {
    return editor ? starBlockIndexes(editor) : new Map();
  }

  /** Re-anchor a star at a saved textblock index (the saved-notes view). */
  export function addStarAtBlock(id: number, blockIndex: number) {
    if (editor) anchorStarAtBlock(editor, id, blockIndex);
  }

  /** Scroll a star's line into view (player star clicks). */
  export function scrollToStar(id: number) {
    if (!editor) return;
    const el = editor.view.dom.querySelector(
      `[data-star-id="${id}"]`
    ) as HTMLElement | null;
    el?.scrollIntoView({ block: 'center', behavior: 'smooth' });
  }

  /** Bring a document position into view and mark it, so the user can see
   * where they were sent. Returns whether there was anything to go to.
   *
   * The mark is a ProseMirror decoration rather than a class on the element:
   * ProseMirror rebuilds the DOM it owns from document state, so a class set
   * by hand vanishes at the next redraw — which for a document the user is
   * about to type in is right away. */
  function reveal(pos: number | null): boolean {
    if (!editor || pos === null || !editor.state.doc.nodeAt(pos)) return false;
    const view = editor.view;
    const node = view.nodeDOM(pos) ?? view.domAtPos(pos).node;
    const el = (node instanceof HTMLElement ? node : node?.parentElement) ?? null;
    if (!el) {
      flashNodeAt(view, pos);
      return true;
    }
    // Arm the wait before scrolling: it measures where the target is now,
    // and marks it once the scroll has arrived rather than while it is
    // still travelling — over a long document the highlight would be over
    // before the reader could see it.
    afterScrollTo(view.dom, el, () => flashNodeAt(view, pos));
    el.scrollIntoView({ block: 'center', behavior: 'smooth' });
    return true;
  }

  /**
   * Land inside the passage a search result matched: on the query, looked
   * for from where the passage begins.
   *
   * Bounding it matters for a common word — searching "north" would
   * otherwise stop at the first "North Carolina" rather than the
   * "Northstar" the palette showed. When the query is nowhere in the
   * passage (a semantic hit) the passage's own opening line is the answer.
   */
  export function scrollToPassage(query: string, lead: string): boolean {
    if (!editor) return false;
    const { doc } = editor.state;
    const passageStart = findTextPos(doc, lead);
    const target = findTextPos(doc, query, passageStart ?? 0) ?? passageStart;
    return reveal(target);
  }

  /** Scroll to a pasted image by filename — how an `image_text` hit lands,
   * since the text it matched is inside the picture. */
  export function scrollToImage(filename: string): boolean {
    return editor ? reveal(findImagePos(editor.state.doc, filename)) : false;
  }

  /** Whether there is a document to look in yet. The editor is built in
   * `onMount`, so a caller that reacts to this component appearing can
   * arrive a beat too early — and "no editor" has to be told apart from
   * "searched and found nothing", which is a real answer. */
  export function isReady(): boolean {
    return editor !== undefined;
  }

  onDestroy(() => editor?.destroy());
</script>

<div class="relative h-full min-h-0 flex-1 overflow-hidden">
  {#if placeholder && isEmpty}
    <div
      class="pointer-events-none absolute inset-x-0 top-0 select-none"
      style={surfacePadding}
      aria-hidden="true"
    >
      <span class="note-prose-placeholder">{placeholder}</span>
    </div>
  {/if}
  <div bind:this={editorEl} class="h-full"></div>
</div>

<svelte:window onkeydown={onLightboxKeydown} />

{#if lightbox}
  <!-- The ConfirmDialog overlay pattern, not the vendored dialog (its
       scroll lock is unwanted here too). One dismissal everywhere:
       click, Esc. -->
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions
       -- Escape is handled on the window; the whole surface is one
       dismiss target. -->
  <div
    class="fixed inset-0 z-50 flex items-center justify-center bg-black/70 p-6"
    role="dialog"
    aria-modal="true"
    aria-label={copy.common.imageViewer}
    tabindex="-1"
    onclick={() => (lightbox = null)}
  >
    <img
      src={lightbox.src}
      alt={lightbox.alt}
      class="max-h-[92vh] max-w-[92vw] rounded-md object-contain shadow-2xl"
    />
  </div>
{/if}

<style>
  /* The note type system — matches the transcript's reading style (15px/1.65)
     with the display face carrying headings, and everything else kept quiet. */
  :global(.note-prose) {
    font-size: 15px;
    line-height: 1.65;
    color: var(--foreground);
    caret-color: var(--foreground);
    position: relative; /* positions the gutter stars */
  }
  /* Never a focus outline: the pane is the editor, not a field. */
  :global(.note-prose:focus),
  :global(.note-prose:focus-visible) {
    outline: none;
  }

  /* A gutter star: a pseudo-element on the starred textblock (each bullet's
     own paragraph included), absolutely positioned into the surface's left
     padding at its line's top. Pseudo-elements are outside the DOM
     selection, so Ctrl+A copies only the notes text; clicks land via the
     extension's gutter hit-test. */
  :global(.note-prose [data-star-id]::before) {
    content: '★';
    position: absolute;
    left: 0.45rem;
    font-size: 15px;
    line-height: 24.75px; /* the body's 15px × 1.65 line box */
    color: var(--muted-foreground);
    cursor: pointer;
  }
  .note-prose-placeholder {
    font-size: 15px;
    line-height: 1.65;
    color: color-mix(in oklch, var(--muted-foreground) 70%, transparent);
  }

  /* Pasted images: bounded so a screenshot never eats the pane (bytes
     stay full-resolution on disk — display only), with the viewer a
     click away. Radius matches the code blocks'. */
  :global(.note-prose img) {
    display: block;
    max-width: 100%;
    width: auto;
    height: auto;
    max-height: min(420px, 50vh);
    border-radius: 0.375rem;
    cursor: zoom-in;
  }
  :global(.note-prose li > img) {
    margin-top: 0.25em;
  }
  /* A selected block node (click an image, then Backspace deletes it). */
  :global(.note-prose .ProseMirror-selectednode) {
    outline: 2px solid var(--ring);
    outline-offset: 2px;
  }
  /* The gap cursor: the only caret position after a document-ending
     image. StarterKit ships the plugin; this is its standard CSS, which
     nothing else in the app provides. */
  :global(.ProseMirror-gapcursor) {
    display: none;
    pointer-events: none;
    position: absolute;
  }
  :global(.ProseMirror-gapcursor::after) {
    content: '';
    display: block;
    position: absolute;
    top: -2px;
    width: 20px;
    border-top: 1px solid var(--foreground);
    animation: gapcursor-blink 1.1s steps(2, start) infinite;
  }
  @keyframes gapcursor-blink {
    to {
      visibility: hidden;
    }
  }
  :global(.ProseMirror-focused .ProseMirror-gapcursor) {
    display: block;
  }

  /* Stack rhythm: spacing between top-level blocks only, so the first line
     always starts exactly at the surface padding (placeholder alignment
     depends on this). */
  :global(.note-prose > * + *) {
    margin-top: 0.55em;
  }
  :global(.note-prose p) {
    margin: 0;
  }

  :global(.note-prose h1),
  :global(.note-prose h2),
  :global(.note-prose h3) {
    font-family: 'Libre Baskerville', Georgia, 'Times New Roman', serif;
    font-weight: 400;
    line-height: 1.35;
  }
  :global(.note-prose h1) {
    font-size: 1.35rem;
  }
  :global(.note-prose h2) {
    font-size: 1.15rem;
  }
  :global(.note-prose h3) {
    font-size: 1rem;
    font-weight: 700;
  }
  :global(.note-prose > * + h1),
  :global(.note-prose > * + h2),
  :global(.note-prose > * + h3) {
    margin-top: 1.2em;
  }

  :global(.note-prose ul),
  :global(.note-prose ol) {
    padding-left: 1.4rem;
  }
  :global(.note-prose ul) {
    list-style: disc;
  }
  :global(.note-prose ol) {
    list-style: decimal;
  }
  :global(.note-prose li) {
    margin-top: 0.15em;
  }
  :global(.note-prose li p) {
    margin: 0;
  }
  :global(.note-prose li > ul),
  :global(.note-prose li > ol) {
    margin-top: 0.15em;
  }
  :global(.note-prose ::marker) {
    color: var(--muted-foreground);
  }

  :global(.note-prose blockquote) {
    border-left: 2px solid var(--border);
    padding-left: 0.85em;
    color: var(--muted-foreground);
  }
  :global(.note-prose code) {
    font-family:
      ui-monospace, SFMono-Regular, Menlo, Consolas, 'Liberation Mono', monospace;
    font-size: 0.85em;
    background: var(--muted);
    border-radius: 0.25rem;
    padding: 0.1em 0.35em;
  }
  :global(.note-prose pre) {
    background: var(--muted);
    border-radius: 0.375rem;
    padding: 0.6em 0.8em;
    overflow-x: auto;
  }
  :global(.note-prose pre code) {
    background: transparent;
    padding: 0;
    font-size: 0.85em;
  }
  :global(.note-prose hr) {
    border: 0;
    border-top: 1px solid var(--border);
    margin: 1.1em 0;
  }
  :global(.note-prose a) {
    text-decoration: underline;
    text-underline-offset: 2px;
  }
  :global(.note-prose strong) {
    font-weight: 600;
  }
</style>
