<script lang="ts">
	import './layout.css';
	import * as Tooltip from '$lib/components/ui/tooltip';
	import HoverTip from '$lib/components/HoverTip.svelte';

	const { children } = $props();

	// The app owns right-click ([shell.md]): the browser's default menu
	// (back, reload, inspect) makes no sense here. It survives only where
	// it is useful: editable text (cut/copy/paste/spellcheck) and a
	// text selection (right-click → Copy). Bubble phase, deliberately: the
	// app's own context menus preventDefault at their trigger before the
	// event reaches the window, which is also this handler's signal to do
	// nothing.
	function onContextMenu(e: MouseEvent) {
		if (e.defaultPrevented) return;
		const el = e.target instanceof Element ? e.target : null;
		if (el?.closest('input, textarea, [contenteditable="true"]')) return;
		// intersectsNode, not Selection.containsNode: the click usually lands
		// on the element holding the selected text, which contains the
		// range rather than being contained by it.
		const sel = window.getSelection();
		if (
			sel &&
			!sel.isCollapsed &&
			el &&
			sel.rangeCount > 0 &&
			sel.getRangeAt(0).intersectsNode(el)
		)
			return;
		e.preventDefault();
	}
</script>

<svelte:window oncontextmenu={onContextMenu} />

<!-- One provider for the whole app. It is not optional: bits-ui's Tooltip.Root
     reads the provider off context and throws without it, which is why the
     vendored primitive sat unused and every hover label in the app was still a
     native Windows tooltip.

     The delay is ours, not the default. The provider defaults to 0 (instant),
     and a rail of icon buttons then fires a tooltip on every mouse traverse. -->
<Tooltip.Provider delayDuration={400}>
	{@render children()}
	<HoverTip />
</Tooltip.Provider>
