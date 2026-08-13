<script lang="ts">
	import { Command as CommandPrimitive } from "bits-ui";
	import { cn } from "$lib/utils.js";

	let {
		ref = $bindable(null),
		class: className,
		...restProps
	}: CommandPrimitive.ListProps = $props();

	// Wheel scrolling inside the modal dialog gets swallowed upstream (only
	// the native scrollbar worked), so scroll the list directly with a
	// non-passive handler; deterministic regardless of what swallows the
	// default.
	$effect(() => {
		const el = ref as HTMLElement | null;
		if (!el) return;
		const onWheel = (e: WheelEvent) => {
			if (el.scrollHeight <= el.clientHeight) return;
			e.preventDefault();
			el.scrollTop += e.deltaMode === 1 ? e.deltaY * 32 : e.deltaY;
		};
		el.addEventListener("wheel", onWheel, { passive: false });
		return () => el.removeEventListener("wheel", onWheel);
	});
</script>

<CommandPrimitive.List
	bind:ref
	data-slot="command-list"
	class={cn("no-scrollbar max-h-72 scroll-py-1 outline-none overflow-x-hidden overflow-y-auto", className)}
	{...restProps}
/>
