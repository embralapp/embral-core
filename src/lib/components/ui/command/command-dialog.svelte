<script lang="ts">
	import type { Command as CommandPrimitive, Dialog as DialogPrimitive } from "bits-ui";
	import type { Snippet } from "svelte";
	import Command from "./command.svelte";
	import * as Dialog from "$lib/components/ui/dialog/index.js";
	import { cn, type WithoutChildrenOrChild } from "$lib/utils.js";

	let {
		open = $bindable(false),
		ref = $bindable(null),
		value = $bindable(""),
		title = "Command Palette",
		description = "Search for a command to run...",
		showCloseButton = false,
		portalProps,
		onCloseAutoFocus,
		children,
		class: className,
		...restProps
	}: WithoutChildrenOrChild<DialogPrimitive.RootProps> &
		WithoutChildrenOrChild<CommandPrimitive.RootProps> & {
			portalProps?: DialogPrimitive.PortalProps;
			children: Snippet;
			title?: string;
			description?: string;
			showCloseButton?: boolean;
			/** Closing a dialog normally returns focus to whatever opened it.
			 * A palette that navigates wants to opt out; see SearchPalette. */
			onCloseAutoFocus?: (event: Event) => void;
			class?: string;
		} = $props();
</script>

<!-- `restProps` goes to the Command, not the Dialog: everything a caller passes
     here (`shouldFilter`, `loop`, …) is a Command prop, and spreading it into
     both handed the dialog attributes it has no idea what to do with. -->
<Dialog.Root bind:open>
	<Dialog.Header class="sr-only">
		<Dialog.Title>{title}</Dialog.Title>
		<Dialog.Description>{description}</Dialog.Description>
	</Dialog.Header>
	<Dialog.Content
		class={cn("rounded-xl! top-1/3 translate-y-0 overflow-hidden p-0", className)}
		{showCloseButton}
		{portalProps}
		{onCloseAutoFocus}
	>
		<Command {...restProps} bind:value bind:ref {children} />
	</Dialog.Content>
</Dialog.Root>
