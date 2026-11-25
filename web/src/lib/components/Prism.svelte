<!-- The prism zone: the regex beam enters the left face; one ray per selected engine leaves the
     right face, evenly spaced, and lands on its lane's centre. Decorative (aria-hidden). -->
<script lang="ts">
  import type { Engine } from "$lib/engines";

  // `centres`: each lane's vertical centre, measured from the top of the beams zone; `height`:
  // the beams zone's height. Rays land exactly where the lanes are, whatever they measure.
  let { engines, centres, height }: { engines: readonly Engine[]; centres: number[]; height: number } = $props();
  const W = 120;
  const mid = $derived(centres.length ? (centres[0]! + centres[centres.length - 1]!) / 2 : height / 2);
  // Prism: a triangle around the middle; rays leave its right face at evenly spaced points.
  const top = $derived(mid - 34);
  const bottom = $derived(mid + 34);
  const exits = $derived(engines.map((_, i) => mid - 18 + (36 * (i + 0.5)) / Math.max(1, engines.length)));
</script>

<svg class="prism" width={W} {height} viewBox={`0 0 ${W} ${height}`} aria-hidden="true" focusable="false">
  <path class="beam-in" d={`M0 ${mid} H38`} pathLength="1" />
  <path class="glass" d={`M30 ${bottom} L54 ${top} L78 ${bottom} Z`} />
  {#each engines as e, i (e.id)}
    {#if centres[i] !== undefined}<path class="ray" style={`stroke:${e.ink}`} d={`M${66 - Math.abs(exits[i]! - mid) * 0.3} ${exits[i]} C 96 ${exits[i]}, 100 ${centres[i]}, ${W} ${centres[i]}`} pathLength="1" />{/if}
  {/each}
</svg>

<style>
  .prism { display: block; overflow: visible; }
  .glass { fill: var(--surface); stroke: var(--ink); stroke-width: 1; stroke-linejoin: round; }
  .beam-in, .ray {
    fill: none;
    stroke-width: var(--beam);
    stroke-linecap: round;
    stroke-dasharray: 1;
    stroke-dashoffset: 1;
    animation: draw var(--t-beam) var(--ease) forwards;
  }
  .beam-in { stroke: var(--ink); }
  @keyframes draw { to { stroke-dashoffset: 0; } }
</style>
