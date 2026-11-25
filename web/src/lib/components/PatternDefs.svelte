<!-- SVG pattern symbols (spec: Engine identity, Patterns elsewhere). Rendered once in the
     layout; anything can fill with url(#pat-<engine>). Lines are the engine ink at 25%
     opacity, 1 px, on a 6 px tile over the engine fill. -->
<script lang="ts">
  import { ENGINES } from "$lib/engines";
</script>

<svg class="defs" aria-hidden="true" focusable="false" width="0" height="0">
  <defs>
    {#each ENGINES as e (e.id)}
      <pattern id={`pat-${e.id}`} width="6" height="6" patternUnits="userSpaceOnUse">
        <rect width="6" height="6" fill={e.fill} />
        <g stroke={e.ink} stroke-opacity="0.25" stroke-width="1" fill={e.ink} fill-opacity="0.25">
          {#if e.pattern === "diagonal"}
            <path d="M-1 7 L7 -1 M-1 1 L1 -1 M5 7 L7 5" fill="none" />
          {:else if e.pattern === "dots"}
            <circle cx="3" cy="3" r="1" stroke="none" />
          {:else if e.pattern === "horizontal"}
            <path d="M0 3.5 H6" fill="none" />
          {:else if e.pattern === "crosshatch"}
            <path d="M-1 7 L7 -1 M-1 -1 L7 7" fill="none" />
          {:else if e.pattern === "vertical"}
            <path d="M3.5 0 V6" fill="none" />
          {:else if e.pattern === "waves"}
            <path d="M0 3 Q1.5 1 3 3 T6 3" fill="none" />
          {:else if e.pattern === "zigzag"}
            <path d="M0 4 L1.5 2 L3 4 L4.5 2 L6 4" fill="none" />
          {:else}
            <rect width="3" height="3" stroke="none" /><rect x="3" y="3" width="3" height="3" stroke="none" />
          {/if}
        </g>
      </pattern>
    {/each}
    <!-- Pending: diagonal stripes in hairline grey. Not confirmed: grey stripes. Timed out: grey cross-hatch. -->
    <pattern id="pat-pending" width="8" height="8" patternUnits="userSpaceOnUse" patternTransform="rotate(45)">
      <rect width="8" height="8" class="paper" /><rect width="3" height="8" class="hairline" />
    </pattern>
    <pattern id="pat-unconfirmed" width="6" height="6" patternUnits="userSpaceOnUse" patternTransform="rotate(45)">
      <rect width="6" height="6" class="surface" /><rect width="1" height="6" class="muted" />
    </pattern>
    <pattern id="pat-timeout" width="6" height="6" patternUnits="userSpaceOnUse">
      <rect width="6" height="6" class="surface" /><path d="M-1 7 L7 -1 M-1 -1 L7 7" class="muted-line" />
    </pattern>
  </defs>
</svg>

<style>
  .defs { position: absolute; width: 0; height: 0; overflow: hidden; }
  .paper { fill: var(--paper); }
  .surface { fill: var(--surface); }
  .hairline { fill: var(--hairline); }
  .muted { fill: var(--ink-muted); fill-opacity: 0.35; }
  .muted-line { stroke: var(--ink-muted); stroke-opacity: 0.35; stroke-width: 1; fill: none; }
</style>
