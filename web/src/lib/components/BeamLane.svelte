<!-- One engine's lane (spec: Bench → Layout, Lane states). The beam's bend is categorical
     (agrees or differs with the baseline), never a measurement. The SVG is hidden from
     assistive tech except the bend node, which is a button. -->
<script lang="ts">
  import { engineVars, type Engine } from "$lib/engines";
  import type { Lane } from "$lib/types";
  import EngineChip from "./EngineChip.svelte";
  import EvidenceChip from "./EvidenceChip.svelte";
  import VerdictBadge from "./VerdictBadge.svelte";

  let { engine, lane, baseline, onwitness }: { engine: Engine; lane: Lane; baseline: boolean; onwitness: () => void } = $props();
  const bent = $derived(lane.state === "differs");
</script>

<li class={`lane ${lane.state}`} style={engineVars(engine)} aria-label={`${engine.name}: ${lane.state === "differs" ? "differs" : lane.state}`}>
  <div class="beam">
    <svg aria-hidden="true" focusable="false">
      {#if lane.state === "error"}
        <line class="ray stub" x1="0" y1="50%" x2="6%" y2="50%" pathLength="1" />
      {:else if bent}
        <line class="ray" x1="0" y1="50%" x2="78%" y2="50%" pathLength="1" />
        <line class="ray bend-arm" x1="78%" y1="50%" x2="96%" y2="20%" pathLength="1" />
      {:else}
        <line class={`ray ${lane.state}`} x1="0" y1="50%" x2="96%" y2="50%" pathLength="1" />
      {/if}
    </svg>
    {#if bent}
      <button class="bend" type="button" onclick={onwitness} aria-label={`Show the witness where ${engine.name} differs`}></button>
    {/if}
  </div>
  <div class="info">
    <div class="row">
      {#if bent}
        <button class="badge-button" type="button" onclick={onwitness} aria-label={`Differs: show the witness for ${engine.name}`}><VerdictBadge state={lane.state} /></button>
      {:else}
        <VerdictBadge state={lane.state} />
      {/if}
      {#if lane.evidence}<EvidenceChip evidence={lane.evidence} />{/if}
    </div>
    <div class="row">
      <span class="summary">{lane.reason ?? lane.summary}</span>
    </div>
  </div>
  <div class="who"><EngineChip {engine} version {baseline} /></div>
</li>

<style>
  .lane {
    display: grid;
    grid-template-columns: minmax(48px, 1fr) minmax(0, max-content) minmax(0, max-content);
    align-items: center;
    gap: var(--s-3);
    min-height: var(--lane-h);
    padding: var(--s-2) var(--s-4) var(--s-2) 0;
    border: var(--line) solid var(--hairline);
    border-left: 0;
    border-radius: 0 var(--radius) var(--radius) 0;
    background: var(--surface);
  }
  .beam { position: relative; align-self: stretch; margin: calc(-1 * var(--s-2)) 0; }
  .beam svg { position: absolute; inset: 0; width: 100%; height: 100%; overflow: visible; }
  .bend-arm { animation-delay: var(--t-beam); stroke: var(--eng-ink); }
  .ray, .stub {
    fill: none;
    stroke: var(--eng-ink);
    stroke-width: var(--beam);
    stroke-linecap: round;
    stroke-dasharray: 1;
    stroke-dashoffset: 1;
    animation: draw var(--t-beam) var(--ease) forwards;
  }
  .ray.pending, .ray.idle { stroke: var(--hairline); }
  .ray.slow { stroke-dasharray: 0.03 0.02; animation: none; }
  .ray.not-confirmed { stroke: var(--ink-muted); }
  @keyframes draw { to { stroke-dashoffset: 0; } }
  .lane.pending, .lane.not-confirmed { background-image: linear-gradient(var(--surface), var(--surface)); }
  .bend {
    position: absolute;
    left: calc(78% - 11px);
    top: calc(50% - 11px);
    width: 22px;
    height: 22px;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: transparent;
    cursor: pointer;
  }
  .bend { top: calc(50% - 11px); }
  .bend::after {
    content: "";
    position: absolute;
    inset: 6px;
    border-radius: 50%;
    background: var(--witness);
    box-shadow: 0 0 0 2px var(--surface);
  }
  /* 44 px touch target around the node. */
  .bend::before { content: ""; position: absolute; inset: -11px; }
  .info { display: grid; gap: var(--s-1); min-width: 0; }
  .row { display: flex; align-items: center; gap: var(--s-2); flex-wrap: wrap; }
  .summary { color: var(--ink-muted); font-size: var(--text-12); }
  .badge-button { padding: 0; border: 0; background: none; cursor: pointer; border-radius: var(--radius-pill); }
  .who { justify-self: end; min-width: 0; }
  .lane.differs { border-color: var(--witness); border-left: 0; }
  @media (max-width: 1023px) {
    .lane { border-left: var(--line) solid var(--hairline); border-radius: var(--radius); padding-left: 0; }
    .lane.differs { border-left: var(--line) solid var(--witness); }
  }
  @media (max-width: 639px) {
    .lane {
      grid-template-columns: 1fr auto;
      grid-template-areas: "who beam" "info info";
      height: auto;
      padding: var(--s-3);
      row-gap: var(--s-2);
    }
    .beam { grid-area: beam; width: 96px; height: 24px; }
    .info { grid-area: info; }
    .who { grid-area: who; justify-self: start; }
  }
</style>
