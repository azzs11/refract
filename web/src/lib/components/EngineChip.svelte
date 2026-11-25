<!-- Engine name with its shape and pattern swatch; optionally its pinned version. -->
<script lang="ts">
  import { engineVars, type Engine } from "$lib/engines";
  import EngineShape from "./EngineShape.svelte";

  let { engine, version = false, baseline = false }: { engine: Engine; version?: boolean; baseline?: boolean } = $props();
</script>

<span class="chip" class:stacked={version} style={engineVars(engine)}>
  <EngineShape {engine} />
  <span class="text">
    <span class="name">{engine.name}{#if baseline}<span class="star" title="Baseline: bends are measured against this engine" aria-label="baseline"> ★</span>{/if}</span>
    {#if version}<span class="version">{engine.engine}</span>{/if}
  </span>
</span>

<style>
  .chip { display: inline-flex; align-items: center; gap: var(--s-2); min-width: 0; }
  .name { font-weight: 500; color: var(--eng-ink); white-space: nowrap; }
  .star { color: var(--ink); font-size: var(--text-12); }
  .version { color: var(--ink-muted); font-size: var(--text-12); white-space: nowrap; }
  .text { display: inline-flex; gap: var(--s-2); min-width: 0; }
  .stacked .text { flex-direction: column; gap: 0; line-height: 1.3; }
</style>
