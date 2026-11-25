<!-- Verdict badge (spec: Lane states). Icon + word; colour never alone. -->
<script lang="ts">
  import type { LaneState } from "$lib/types";

  let { state }: { state: LaneState } = $props();
  const LABEL: Record<LaneState, string> = {
    idle: "Waiting",
    pending: "Checking",
    same: "Same",
    differs: "Differs",
    error: "Error",
    slow: "Slow",
    "not-confirmed": "Not confirmed",
  };
</script>

<span class={`badge ${state}`}>
  <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true" focusable="false">
    {#if state === "same"}
      <path d="M2 6.5 L5 9.5 L10 2.5" />
    {:else if state === "differs"}
      <path d="M1.5 6 H5 M5 6 L10.5 2 M5 6 L10.5 10 M8.5 2 H10.5 V4 M8.5 10 H10.5 V8" />
    {:else if state === "error"}
      <path d="M6 1.5 V7.5 M6 9.5 V10.5" />
    {:else if state === "slow"}
      <path d="M2.5 1.5 H9.5 M2.5 10.5 H9.5 M3 1.5 Q3 5 6 6 Q9 7 9 10.5 M9 1.5 Q9 5 6 6 Q3 7 3 10.5" />
    {:else}
      <path d="M2 6 H10" />
    {/if}
  </svg>
  {LABEL[state]}
</span>

<style>
  .badge {
    display: inline-flex;
    align-items: center;
    gap: var(--s-1);
    padding: 2px var(--s-2);
    border: var(--line) solid var(--hairline);
    border-radius: var(--radius-pill);
    background: var(--surface);
    font-size: var(--text-12);
    font-weight: 600;
    white-space: nowrap;
  }
  svg { fill: none; stroke: currentColor; stroke-width: 1.6; stroke-linecap: round; stroke-linejoin: round; }
  .same { color: var(--same); border-color: var(--same); }
  .differs { color: var(--witness); border-color: var(--witness); }
  .error, .slow, .not-confirmed, .idle, .pending { color: var(--ink-muted); }
</style>
