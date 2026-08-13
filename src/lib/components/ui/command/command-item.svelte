<script lang="ts">
	import { Command as CommandPrimitive } from "bits-ui";
	import { cn } from "$lib/utils.js";

	let {
		ref = $bindable(null),
		class: className,
		children,
		...restProps
	}: CommandPrimitive.ItemProps = $props();
</script>

<!--
	The highlight is `bg-accent`, not `bg-muted`. In the dark theme `--muted` is
	`oklch(0.269 0 0)` (byte-identical to `--popover`, the surface this list is
	drawn on), so the selected row painted exactly like an unselected one and the
	arrow keys looked broken. They were never broken; the selection was invisible.
	(`command-link-item` already used `bg-accent`; the two variants disagreed.)

	The `CheckIcon` that used to live here is gone: it keyed off `data-checked`,
	which bits-ui's command item never emits, so it was permanently invisible.
-->
<CommandPrimitive.Item
	bind:ref
	data-slot="command-item"
	class={cn(
		"group/command-item data-selected:bg-accent data-selected:text-accent-foreground data-selected:*:[svg]:text-accent-foreground relative flex cursor-default items-center gap-2 rounded-sm px-2 py-1.5 text-sm outline-hidden select-none in-data-[slot=dialog-content]:rounded-lg! data-[disabled=true]:pointer-events-none data-[disabled=true]:opacity-50 [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4",
		className
	)}
	{...restProps}
>
	{@render children?.()}
</CommandPrimitive.Item>
