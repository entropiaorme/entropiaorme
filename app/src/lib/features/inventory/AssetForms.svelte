<script lang="ts">
	/** The asset form and the purchase form, opened through `forms`. */
	import AssetPurchaseModal from './AssetPurchaseModal.svelte';
	import type { AssetForms } from './assetForms.svelte';
	import InventoryItemFormModal from './InventoryItemFormModal.svelte';

	let { forms }: { forms: AssetForms } = $props();
</script>

<InventoryItemFormModal
	bind:open={() => forms.assetOpen, (open) => (forms.assetOpen = open)}
	item={forms.assetToEdit}
	prefill={forms.assetPrefill}
	around={forms.around}
	onsaved={forms.assetSaved}
/>
<AssetPurchaseModal
	item={forms.purchaseItem}
	purchase={forms.purchase}
	prefill={forms.purchasePrefill}
	onsubmit={forms.savePurchase}
	oncancel={forms.closePurchase}
/>
