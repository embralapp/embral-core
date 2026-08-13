<script lang="ts">
  // Test-only mount wrapper: RecordingControls' tooltips need the same
  // Tooltip.Provider the app supplies in +layout.svelte. Not imported by
  // any route — only RecordingControls.test.ts mounts it.
  import * as Tooltip from '$lib/components/ui/tooltip';
  import RecordingControls from './RecordingControls.svelte';
</script>

<Tooltip.Provider>
  <RecordingControls userNotes="" meetingTitle="" />
</Tooltip.Provider>
