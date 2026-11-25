<!-- Witness card (spec: Results → Witness card): the construct, the witness with invisibles
     drawn, one pane per engine with the raw engine output, the evidence, why, and actions. -->
<script lang="ts">
  import { engine } from "$lib/engines";
  import type { Lane, Witness } from "$lib/types";
  import EngineChip from "./EngineChip.svelte";
  import EvidenceChip from "./EvidenceChip.svelte";
  import InvisibleText from "./InvisibleText.svelte";

  let { witness, lanes }: { witness: Witness; lanes: Lane[] } = $props();
  let copied = $state("");

  async function copy(what: string, text: string) {
    try {
      await navigator.clipboard.writeText(text);
      copied = what;
    } catch {
      copied = "";
    }
  }
  const ACTIONS = ["Copy as test case", "Copy as issue text", "Open in Lens", "Copy permalink"];
</script>

<article class="card" aria-labelledby="witness-title">
  <h2 id="witness-title">{witness.title}</h2>
  <div class="witness-row">
    <span class="label">Shortest witness</span>
    <code class="witness"><InvisibleText text={witness.text} /></code>
    <button type="button" class="action" onclick={() => copy("witness", witness.text)}>{copied === "witness" ? "Copied" : "Copy"}</button>
  </div>
  <div class="panes">
    {#each witness.panes as p (p.engine)}
      {@const e = engine(p.engine)}
      {@const lane = lanes.find((l) => l.engine === p.engine)}
      <section class="pane" aria-label={`${e.name} result`}>
        <header><EngineChip engine={e} version /> {#if lane?.evidence}<EvidenceChip evidence={lane.evidence} />{/if}</header>
        <p class="outcome">
          {#if p.matched && p.span}
            Matches <code class="span"><InvisibleText text={[...witness.text].slice(p.span[0], p.span[1]).join("")} /></code> at {p.span[0]}–{p.span[1]}
          {:else}
            No match
          {/if}
        </p>
        <details>
          <summary>Engine output</summary>
          <pre>{p.raw}</pre>
        </details>
      </section>
    {/each}
  </div>
  <p class="why">{witness.why} <a href={witness.specHref} rel="noopener">Spec entry</a></p>
  <div class="actions">
    {#each ACTIONS as a (a)}
      {#if a === "Copy permalink"}
        <button type="button" class="action" onclick={() => copy(a, location.href)}>{copied === a ? "Copied" : a}</button>
      {:else}
        <button type="button" class="action" aria-disabled="true" title="Arrives in a later UI phase">{a}</button>
      {/if}
    {/each}
  </div>
</article>

<style>
  .card { display: grid; gap: var(--s-4); }
  h2 { font-size: var(--text-20); }
  .label { font-size: var(--text-12); color: var(--ink-muted); }
  .witness-row { display: flex; align-items: center; gap: var(--s-3); flex-wrap: wrap; }
  .witness { padding: var(--s-1) var(--s-3); border: var(--line) solid var(--witness); border-radius: var(--radius); color: var(--witness); background: var(--surface); font-size: var(--text-16); }
  .panes { display: grid; grid-template-columns: repeat(auto-fit, minmax(260px, 1fr)); gap: var(--s-3); }
  .pane { display: grid; gap: var(--s-2); padding: var(--s-3); border: var(--line) solid var(--hairline); border-radius: var(--radius); background: var(--surface); }
  .pane header { display: flex; align-items: center; gap: var(--s-2); flex-wrap: wrap; }
  .outcome { margin: 0; }
  .span { outline: 1.5px solid var(--witness); outline-offset: 1px; border-radius: 2px; padding: 0 2px; }
  details summary { cursor: pointer; color: var(--ink-muted); font-size: var(--text-12); min-height: 24px; }
  pre { margin: var(--s-2) 0 0; padding: var(--s-2); background: var(--paper); border-radius: var(--radius); font-size: var(--text-12); white-space: pre-wrap; }
  .why { margin: 0; max-width: var(--text-max); }
  .why a { color: var(--focus); }
  .actions { display: flex; gap: var(--s-2); flex-wrap: wrap; }
  .action { min-height: var(--target); padding: 0 var(--s-3); border: var(--line) solid var(--hairline); border-radius: var(--radius); background: var(--surface); cursor: pointer; }
  .action[aria-disabled="true"] { color: var(--ink-muted); cursor: default; }
</style>
