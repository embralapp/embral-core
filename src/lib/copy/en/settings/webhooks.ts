// Settings → Webhooks. A list of destinations called when a meeting
// finishes. The default payload is metadata only; each row gates full
// content behind its own switch, and the sub must say plainly what
// leaves the machine.

export const webhooks = {
  destinations: {
    _group: 'Webhooks',
    intro: {
      label: 'Send a webhook when a meeting finishes',
      sub: 'Calls each URL with meeting metadata so your own tools and automations can react'
    },
    method: {
      post: 'POST',
      put: 'PUT'
    },
    urlPlaceholder: 'https://example.com/hook',
    add: 'Add webhook',
    removeAria: 'Remove webhook',
    content: {
      label: 'Include full content',
      sub: 'Sends the summary, your notes, and the transcript; off sends only the title and timing'
    },
    test: {
      send: 'Send test',
      sending: 'Sending...',
      ok: 'Test delivered'
    }
  },
  payloadNote:
    'The payload is JSON: event, meeting id, title, date, and duration, plus the content fields when enabled'
};
