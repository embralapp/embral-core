// The English catalog. Composition only: no logic, no strings of its own.
// One entry per surface; each surface file reads in the order the user meets
// it on screen.

import { common } from './common';
import { dictation } from './dictation';
import { errors } from './errors';
import { meetings } from './meetings';
import { notifications } from './notifications';
import { onboarding } from './onboarding';
import { settings } from './settings';
import { shell } from './shell';
import { speakers } from './speakers';

export const en = {
  common,
  shell,
  meetings,
  speakers,
  dictation,
  onboarding,
  notifications,
  settings,
  errors
};
