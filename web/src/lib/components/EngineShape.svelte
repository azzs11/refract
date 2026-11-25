<!-- An engine's shape, filled with its pattern and outlined in its ink. Decorative: the
     engine name always sits next to it. -->
<script lang="ts">
  import type { Engine } from "$lib/engines";

  let { engine, size = 14 }: { engine: Engine; size?: number } = $props();

  const PATHS: Record<string, string> = {
    circle: "M8 1.5 A6.5 6.5 0 1 1 7.99 1.5 Z",
    square: "M2 2 H14 V14 H2 Z",
    triangle: "M8 1.5 L14.5 14 H1.5 Z",
    diamond: "M8 1 L15 8 L8 15 L1 8 Z",
    pentagon: "M8 1.2 L14.6 6 L12.1 14 H3.9 L1.4 6 Z",
    hexagon: "M4.5 2 H11.5 L15 8 L11.5 14 H4.5 L1 8 Z",
    cross: "M6 1.5 H10 V6 H14.5 V10 H10 V14.5 H6 V10 H1.5 V6 H6 Z",
    star: "M8 1 L9.9 5.9 L15 6.2 L11 9.5 L12.3 14.6 L8 11.8 L3.7 14.6 L5 9.5 L1 6.2 L6.1 5.9 Z",
  };
</script>

<svg class="shape" width={size} height={size} viewBox="0 0 16 16" aria-hidden="true" focusable="false">
  <path d={PATHS[engine.shape]} fill={`url(#pat-${engine.id})`} stroke={engine.ink} stroke-width="1.4" stroke-linejoin="round" />
</svg>

<style>
  .shape { flex: none; display: block; }
</style>
