<script lang="ts">
  import { onMount } from "svelte";
  import { pagePermissions as permissions } from "$domain/permissions";
  import PagePermissionPrompt from "./PagePermissionPrompt.svelte";

  let prompt = $derived(permissions.prompt());

  onMount(() => permissions.hostInSidebar());
</script>

{#if prompt?.in_sidebar}
  {#key `${prompt.profile_id}:${prompt.item_id}:${prompt.request_id}`}
    <PagePermissionPrompt {prompt} />
  {/key}
{/if}
