<!--
	The mark on something read off the game that nothing on screen fully
	confirms: a misread could hide in it, so the player compares it with the
	sell window before accepting. A word, not a colour alone; `why` says what
	is unconfirmed about it.
-->
<script lang="ts">
	let { why }: { why: string } = $props();
</script>

<span class="text-[10px] font-medium uppercase tracking-wide text-warning" title={why}>
	check
</span>
