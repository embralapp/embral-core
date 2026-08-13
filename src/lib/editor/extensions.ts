import StarterKit from '@tiptap/starter-kit';
import { Markdown } from 'tiptap-markdown';
import Link from '@tiptap/extension-link';
import ListItem from '@tiptap/extension-list-item';
import Image from '@tiptap/extension-image';
import {
  defaultMarkdownSerializer,
  type MarkdownSerializerState
} from '@tiptap/pm/markdown';
import type { Node as PMNode } from '@tiptap/pm/model';
import Table from '@tiptap/extension-table';
import TableRow from '@tiptap/extension-table-row';
import TableCell from '@tiptap/extension-table-cell';
import TableHeader from '@tiptap/extension-table-header';
import TaskList from '@tiptap/extension-task-list';
import TaskItem from '@tiptap/extension-task-item';
import { toDisplaySrc, toStoredSrc } from './assetSrc';

/**
 * The markdown embral keeps.
 *
 * ProseMirror is schema-driven: markdown-it renders to HTML, ProseMirror's
 * DOMParser walks it, and any element with no matching schema rule is
 * dropped with no error. So "supports markdown" is exactly "has a node or
 * mark for it"; there is no catch-all on the way in. Whatever is missing
 * here is silently deleted the first time a document containing it is
 * serialized back out, which is why this list is one place and every editor
 * mount uses it.
 *
 * `StarterKit` alone left out `Link`, so `[text](url)` lost its URL, and
 * `Image`, `Table` and `TaskList`, which vanished outright.
 * `tiptap-markdown` already ships markdown serialize/parse specs keyed to
 * these node names, so registering the extensions is all that is needed;
 * no custom serializers.
 *
 * The contract is CommonMark + GFM: headings, emphasis, strikethrough,
 * code and code blocks, blockquotes, horizontal rules, hard breaks,
 * bullet/ordered/task lists, links, images, tables. Anything outside it
 * (footnotes, definition lists, raw HTML blocks) is not kept; see the
 * round-trip test, which is what makes that claim checkable.
 */
export function markdownExtensions(storageRoot = '') {
  // The document stores a portable, storage-relative `src`; the DOM needs an
  // absolute one the webview can load. Mapping it on the attribute (rather
  // than rewriting the markdown) keeps `node.attrs.src` in the stored form,
  // which is what the markdown serializer writes out. Both directions are
  // idempotent, so copying an image between documents round-trips.
  const AssetImage = Image.extend({
    addAttributes() {
      return {
        ...this.parent?.(),
        src: {
          default: null,
          parseHTML: (element) => toStoredSrc(element.getAttribute('src') ?? ''),
          renderHTML: (attributes) => ({
            src: toDisplaySrc(storageRoot, (attributes.src as string) ?? '')
          })
        }
      };
    },
    // The stock image spec (prosemirror-markdown's, re-exported by
    // tiptap-markdown) writes the token and never closes its block; a
    // block-level image glued the next paragraph onto the same line.
    // Delegating keeps upstream's escaping; closeBlock supplies the
    // separator, and block images round-trip byte-stable.
    addStorage() {
      return {
        markdown: {
          serialize(
            state: MarkdownSerializerState,
            node: PMNode,
            parent: PMNode,
            index: number
          ) {
            defaultMarkdownSerializer.nodes.image(state, node, parent, index);
            state.closeBlock(node);
          }
        }
      };
    }
  });

  // The stock list item must open with a paragraph. A pasted image can
  // itself be a bullet here (paste onto an empty bullet replaces it;
  // imagePaste.ts),
  // so it admits an image as its opening child; the round-trip test proves
  // `- ![](…)` survives a save. Task items stay paragraph-first: markdown
  // cannot write an image-only task (`- [ ] ` with its image on the next
  // line loses the checkbox on re-parse), and the paste falls back to
  // nesting below instead.
  return [
    StarterKit.configure({ listItem: false }),
    ListItem.extend({ content: '(paragraph | image) block*' }),
    Markdown.configure({ transformPastedText: true }),
    Link.configure({
      // The editor is not a browser: a click should place the caret, not
      // navigate the whole webview away from the app.
      openOnClick: false,
      autolink: true
    }),
    // Block, not inline: a screenshot is block content, so a paste goes
    // where typed text would and the caret moves below it (imagePaste.ts,
    // [shell.md] §Writing surface). The serializer above is what makes a
    // block image safe; see its comment.
    AssetImage.configure({ inline: false, allowBase64: true }),
    Table.configure({ resizable: false }),
    TableRow,
    TableHeader,
    TableCell,
    TaskList,
    TaskItem.configure({ nested: true })
  ];
}
