// The Profiles surface: the people list, a profile's editor, and the inline
// speaker-name editor used from transcripts.

import { plural } from '../plural';
import { locale } from './locale';

export const speakers = {
  empty: 'No profiles yet...',
  add: 'Add a profile',

  // The pane shown when several profiles are selected.
  multiSelect: {
    selected: (n: number) => `${n} profiles selected`,
    delete: (n: number) => `Delete ${n}`,
    merge: (n: number) => `Merge ${n}`,
    hint: 'Or press Delete'
  },

  // The right-click menu on a profile row; it acts on the whole selection.
  menu: {
    delete: (n: number) =>
      plural(locale, n, { one: 'Delete', other: `Delete ${n} profiles` }),
    merge: (n: number) => `Merge ${n} profiles...`
  },

  // The merge dialog: the same person under two names becomes one profile.
  merge: {
    title: (n: number) => `Merge ${n} profiles`,
    keep: 'Which name stays?',
    body: 'Speaker labels and notes transfer to the profile you keep',
    confirm: 'Merge'
  },

  // The empty-state pitch when no profile is open.
  intro: {
    title: 'Remember who said what',
    body: 'Name a speaker in a transcript to keep per-speaker notes and history in one place'
  },

  deleteConfirm: {
    title: (n: number) =>
      plural(locale, n, {
        one: 'Delete profile?',
        other: `Delete ${n} profiles?`
      }),
    body: 'Past transcripts will keep names',
    confirm: (n: number) =>
      plural(locale, n, { one: 'Delete', other: `Delete ${n}` })
  },

  // The profile editor.
  profile: {
    newTitle: 'New profile',
    name: 'Name',
    namePlaceholder: 'John Smith',
    notes: 'Notes',
    notesPlaceholder: 'What do you want to remember about this person?',
    save: 'Save changes',
    create: 'Create profile',
    // The record: which meetings they spoke in, and what they said.
    meetings: 'Meetings',
    lines: (n: number) =>
      plural(locale, n, { one: '1 line', other: `${n} lines` }),
    noMeetings: 'Not in any meeting yet',
    reallyDelete: 'Really delete?',
    delete: (name: string) => `Delete ${name}`,
    deleteNote: 'Past transcripts will keep names'
  },

  // The inline name editor (from transcript pills).
  nameInput: {
    aria: 'Speaker name'
  }
};
