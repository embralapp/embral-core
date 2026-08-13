// The two shapes every invoke catch relies on when it shows a failure
// (`appState.setError(errorMessage(e))`): a typed backend error renders
// its catalog sentence, and nothing ever renders as "[object Object]".

import { describe, expect, it } from 'vitest';
import { errorMessage } from './errors';
import { copy } from '$lib/copy';

describe('errorMessage', () => {
  it('renders a typed backend error as its catalog sentence', () => {
    expect(errorMessage({ code: 'noActiveRecording' })).toBe(copy.errors.noActiveRecording);
    expect(errorMessage({ code: 'alreadyRecording' })).toBe(copy.errors.alreadyRecording);
  });

  it('interpolates carried detail for the parameterized codes', () => {
    expect(errorMessage({ code: 'internal', detail: 'disk gone' })).toBe(
      copy.errors.internal('disk gone')
    );
    expect(errorMessage({ code: 'fileNotFound', path: '/x.wav' })).toBe(
      copy.errors.fileNotFound('/x.wav')
    );
  });

  it('prefers carried detail for a code the frontend does not know yet', () => {
    expect(errorMessage({ code: 'someFutureCode', detail: 'the reason' })).toBe('the reason');
  });

  it('never renders "[object Object]"', () => {
    for (const shape of [{ unexpected: true }, {}, null]) {
      expect(errorMessage(shape)).not.toContain('[object Object]');
    }
  });
});
