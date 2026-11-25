<!-- How sure we are (spec: Evidence levels): exactly one level per verdict. -->
<script lang="ts">
  import type { Evidence } from "$lib/types";

  let { evidence }: { evidence: Evidence } = $props();
  const TIP: Record<Evidence["level"], string> = {
    proved: "Both automaton constructions agree and the property holds for all strings",
    confirmed: "Replayed on the real engine in this browser",
    recorded: "The real engine was run by CI on this pinned version",
    bounded: "Checked for all strings up to this length (not regular, so not proved)",
    tested: "No difference in the strings tried; not a proof",
  };
</script>

<span class={`evidence ${evidence.level}`} title={TIP[evidence.level]}>{evidence.label}</span>

<style>
  .evidence {
    display: inline-block;
    padding: 1px var(--s-2);
    border: var(--line) dashed var(--hairline);
    border-radius: var(--radius-pill);
    background: var(--surface);
    color: var(--ink-muted);
    font-size: var(--text-12);
    white-space: nowrap;
  }
  .proved, .confirmed { border-style: solid; color: var(--ink); }
</style>
