/** Stable accent per speaker label so the eye can follow a voice down the
 * page, shared by the live transcript, the pending meeting, and the
 * post-meeting segment editor.
 *
 * Text alone: no pill, no fill. A transcript names a speaker on every
 * turn, and chips at that density read as decoration rather than
 * information; colored text carries the same "follow this voice" cue
 * without boxing every line. Editing one is the same text with a caret in
 * it (`SpeakerNameInput`), not a form field that appears in its place. */
const namePalette = [
  'text-sky-700 dark:text-sky-300',
  'text-emerald-700 dark:text-emerald-300',
  'text-amber-700 dark:text-amber-300',
  'text-violet-700 dark:text-violet-300',
  'text-rose-700 dark:text-rose-300',
  'text-cyan-700 dark:text-cyan-300'
];

export function nameClass(label: string, labels: string[]): string {
  const idx = labels.indexOf(label);
  return namePalette[(idx >= 0 ? idx : 0) % namePalette.length];
}
