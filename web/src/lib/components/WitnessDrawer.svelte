<!-- Witness drawer (spec: Bench → Layout): rises from the bottom to 40% of the height,
     resizable by its handle, closes with Esc. Below 640 px it is a bottom sheet. -->
<script lang="ts">
  import type { Snippet } from "svelte";

  let { open = $bindable(), children }: { open: boolean; children: Snippet } = $props();
  let height = $state(40); // vh
  let panel: HTMLElement | undefined = $state();

  $effect(() => {
    if (open) panel?.focus();
  });

  function drag(e: PointerEvent) {
    const handle = e.currentTarget as HTMLElement;
    handle.setPointerCapture(e.pointerId);
    const move = (m: PointerEvent) => {
      height = Math.min(90, Math.max(20, ((window.innerHeight - m.clientY) / window.innerHeight) * 100));
    };
    const up = () => {
      handle.removeEventListener("pointermove", move);
      handle.removeEventListener("pointerup", up);
    };
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", up);
  }

  function key(e: KeyboardEvent) {
    if (e.key === "ArrowUp") height = Math.min(90, height + 5);
    if (e.key === "ArrowDown") height = Math.max(20, height - 5);
  }
</script>

{#if open}
  <div class="drawer" role="dialog" aria-modal="false" aria-label="Witness" tabindex="-1" bind:this={panel} style={`--h:${height}vh`}>
    <div class="bar">
      <button type="button" class="handle" aria-label="Resize the drawer (arrow keys)" onpointerdown={drag} onkeydown={key}></button>
      <button type="button" class="close" onclick={() => (open = false)}>Close <kbd>Esc</kbd></button>
    </div>
    <div class="content">{@render children()}</div>
  </div>
{/if}

<style>
  .drawer {
    position: fixed;
    left: 0;
    right: 0;
    bottom: 0;
    z-index: 10;
    height: var(--h);
    display: grid;
    grid-template-rows: auto 1fr;
    background: var(--surface);
    border-top: var(--line) solid var(--ink);
    box-shadow: var(--popover-shadow);
    animation: rise var(--t-panel) var(--ease);
  }
  @keyframes rise { from { transform: translateY(24px); } }
  .bar { display: flex; align-items: center; justify-content: space-between; padding: 0 var(--s-4); }
  .handle { width: 64px; height: var(--target); border: 0; background: none; cursor: ns-resize; position: relative; }
  .handle::after { content: ""; position: absolute; left: 16px; right: 16px; top: 20px; height: 4px; border-radius: 2px; background: var(--hairline); }
  .close { min-height: var(--target); padding: 0 var(--s-3); border: var(--line) solid var(--hairline); border-radius: var(--radius); background: var(--surface); cursor: pointer; }
  .close kbd { font-size: 11px; color: var(--ink-muted); }
  .content { overflow: auto; padding: 0 var(--s-6) var(--s-6); }
  @media (max-width: 639px) {
    .drawer { border-radius: 12px 12px 0 0; border: var(--line) solid var(--hairline); }
    .content { padding: 0 var(--s-4) var(--s-4); }
  }
</style>
