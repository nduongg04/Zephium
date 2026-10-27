<script lang="ts">
  import { extensions } from "$domain/extensions";
  import { tabs } from "$domain/tabs";
  import * as m from "$shared/i18n/messages";
  import ExtensionActionIcon from "./ExtensionActionIcon.svelte";

  let profileId = $derived(tabs.profile()?.id ?? null);
  let tabId = $derived(tabs.activeId());
  let actions = $derived(extensions.activeActions(profileId, tabId));
  let shortcut = $derived(extensions.actionShortcut(profileId, tabId));
  let actionRoot = $state<HTMLDivElement>();

  // The menu holding the tiles closes when one is chosen, so a popup hangs
  // from the menu's own button, which stays, rather than from a vanished tile.
  const anchor = (tile: HTMLElement) =>
    (
      tile.closest(".ui-disclosure")?.querySelector<HTMLElement>(":scope > button") ?? tile
    ).getBoundingClientRect();

  function invoke(event: MouseEvent, action: (typeof actions)[number]) {
    const target = event.currentTarget;
    if (profileId === null || tabId === null || !(target instanceof HTMLButtonElement)) return;
    void extensions.invoke(profileId, tabId, action, anchor(target));
  }

  $effect(() => {
    const request = shortcut;
    const root = actionRoot;
    if (request === null || root === undefined || profileId === null || tabId === null) return;
    const action = actions.find(
      (candidate) =>
        candidate.runtime.install_id === request.runtime.install_id &&
        candidate.runtime.generation === request.runtime.generation &&
        candidate.revision === request.action_revision,
    );
    if (action === undefined || !action.enabled) return;
    const button = [
      ...root.querySelectorAll<HTMLButtonElement>("button[data-extension-install]"),
    ].find((candidate) => candidate.dataset.extensionInstall === request.runtime.install_id);
    if (button === undefined || button.disabled) return;
    if (!extensions.consumeActionShortcut(request.projection_revision)) return;
    void extensions.invoke(profileId, tabId, action, anchor(button));
  });
</script>

<!--
  The extensions' own buttons, tiles in the tray's extensions row named by
  their tooltips. Choosing one closes the panel around them. At rail width
  they are rows of the column's native menu instead.
-->
<div
  bind:this={actionRoot}
  class="actions"
  role="group"
  aria-label={m.webext_page_title()}
  data-zephium-extension-actions
>
  {#each actions as action (action.runtime.install_id)}
    <button
      type="button"
      data-dismiss
      aria-label={action.label || m.webext_action()}
      aria-haspopup={action.presents_popup ? "dialog" : undefined}
      title={action.label || m.webext_action()}
      disabled={!action.enabled || extensions.isInvoking(action.runtime.install_id)}
      data-extension-install={action.runtime.install_id}
      class="tile"
      onclick={(event) => invoke(event, action)}
    >
      <span class="glyph">
        <ExtensionActionIcon rgba={action.icon_rgba_base64} />
        {#if action.badge.length > 0}
          <span aria-hidden="true" class="badge" data-unread={action.unread_badge}>
            {action.badge}
          </span>
        {/if}
      </span>
    </button>
  {/each}
</div>

<style>
  /* Right of the row's name, wrapping onto more lines when there are more
     than fit. */
  .actions {
    display: flex;
    flex: 1 1 0;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: 2px;
    min-inline-size: 0;
  }

  .actions:empty {
    display: none;
  }

  .actions .tile {
    display: grid;
    place-items: center;
    width: 28px;
    height: 28px;
    padding: 0;
    border: 0;
    border-radius: var(--radius-inset);
    background: none;
    cursor: default;
    outline: none;
    transition: background-color var(--motion-instant) var(--ease-smooth);
  }

  .actions .tile:hover,
  .actions .tile:focus-visible {
    background: var(--row-hover);
  }

  .actions .tile:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: -2px;
  }

  .actions .tile:active {
    background: var(--row-pressed);
  }

  .actions .tile:disabled {
    opacity: 0.45;
  }

  .glyph {
    position: relative;
    display: grid;
    place-items: center;
    width: 18px;
    height: 18px;
  }

  .glyph :global(canvas) {
    width: 18px;
    height: 18px;
  }

  .badge {
    position: absolute;
    top: -5px;
    right: -8px;
    max-width: 30px;
    overflow: hidden;
    padding-inline: 4px;
    border-radius: var(--radius-capsule);
    background: var(--color-accent);
    box-shadow: 0 0 0 1.5px var(--color-menu);
    font-size: 9px;
    font-weight: 600;
    line-height: 14px;
    color: var(--color-on-accent);
    white-space: nowrap;
    text-overflow: ellipsis;
    pointer-events: none;
  }

  .badge[data-unread="true"] {
    background: var(--color-warning);
  }
</style>
