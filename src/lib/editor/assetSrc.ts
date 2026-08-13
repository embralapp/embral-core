import { convertFileSrc } from '@tauri-apps/api/core';

/** An image's `src` has two forms and the editor sees both.
 *
 * Stored is what lives in the markdown: a path relative to the storage
 * root, `assets/{meeting_id}/img-01.png`. It is portable: it survives a
 * meeting rename, a library moved to another drive, and the trip into an
 * Obsidian vault.
 *
 * Display is what the webview can actually load: an absolute path
 * through Tauri's asset protocol.
 *
 * Both directions are idempotent, which is what makes copying an image
 * from one document and pasting it into another safe: the round trip
 * through the DOM hands back a display src, and converting it again must
 * not double-prefix. Remote and inline images pass through untouched; they
 * are not ours to resolve. */

/** Storage-relative → something the webview can load. */
export function toDisplaySrc(storageRoot: string, src: string): string {
  if (!src || isForeign(src)) return src;
  if (!src.startsWith('assets/')) return src;
  if (!storageRoot) return src;
  const root = storageRoot.replace(/[\\/]+$/, '');
  const separator = root.includes('\\') ? '\\' : '/';
  const native = separator === '\\' ? src.replace(/\//g, '\\') : src;
  return convertFileSrc(`${root}${separator}${native}`);
}

/** Whatever the DOM gives back → the storage-relative form to serialize.
 * A display src carries its storage-relative tail after `assets/`, so the
 * conversion is recoverable without knowing the root. */
export function toStoredSrc(src: string): string {
  if (!src) return src;
  if (src.startsWith('assets/')) return src;
  if (src.startsWith('data:')) return src;

  // An asset-protocol URL (`http://asset.localhost/<encoded absolute path>`
  // on Windows, `asset://localhost/…` elsewhere) or a plain file path.
  let decoded = src;
  const marker = /^(?:https?:\/\/asset\.localhost\/|asset:\/\/localhost\/)/;
  if (marker.test(src)) {
    decoded = decodeURIComponent(src.replace(marker, ''));
  } else if (/^https?:\/\//.test(src)) {
    return src;
  }
  decoded = decoded.replace(/\\/g, '/');
  const at = decoded.lastIndexOf('assets/');
  return at >= 0 ? decoded.slice(at) : src;
}

/** Links we do not own: remote images and inline data URLs. */
function isForeign(src: string): boolean {
  return /^(?:https?:|data:|blob:)/.test(src);
}
