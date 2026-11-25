<script lang="ts">
  let { shape, color, size = 14 }: { shape: string; color: string; size?: number } = $props();
  const pts: Record<string, string> = {
    triangle: "7,1 13,13 1,13",
    diamond: "7,1 13,7 7,13 1,7",
    hexagon: "4,1 10,1 13,7 10,13 4,13 1,7",
  };
</script>

<svg width={size} height={size} viewBox="0 0 14 14" aria-hidden="true">
  {#if shape === "circle"}
    <circle cx="7" cy="7" r="6" fill={color} />
  {:else if shape === "square"}
    <rect x="1.5" y="1.5" width="11" height="11" fill={color} />
  {:else}
    <polygon points={pts[shape]} fill={color} />
  {/if}
</svg>
