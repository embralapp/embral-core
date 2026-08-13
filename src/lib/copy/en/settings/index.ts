// Composition only — no logic, no strings of its own.
// One entry per settings page, in the order the rail lists them.

import { about } from './about';
import { cloudSignIn } from './cloud-sign-in';
import { dictation } from './dictation';
import { general } from './general';
import { markdown } from './markdown';
import { mcp } from './mcp';
import { webhooks } from './webhooks';
import { meetings } from './meetings';
import { models } from './models';
import { nav } from './nav';
import { synthesis } from './synthesis';
import { transcription } from './transcription';

export const settings = {
  nav,
  about,
  cloudSignIn,
  general,
  meetings,
  dictation,
  transcription,
  synthesis,
  markdown,
  webhooks,
  mcp,
  models
};
