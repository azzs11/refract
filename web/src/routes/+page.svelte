<!-- The Bench (spec: Bench screen). UI0: layout and components with the default example's
     results hard-coded; no analysis runs yet (UI2 wires the WASM core). -->
<script lang="ts">
  import { SHIPPED, engine, type EngineId } from "$lib/engines";
  import { DEFAULT_EXAMPLE } from "$lib/mock/default-example";
  import BeamLane from "$lib/components/BeamLane.svelte";
  import InputColumn from "$lib/components/InputColumn.svelte";
  import Prism from "$lib/components/Prism.svelte";
  import WitnessCard from "$lib/components/WitnessCard.svelte";
  import WitnessDrawer from "$lib/components/WitnessDrawer.svelte";
  import type { Flavour, Operation } from "$lib/types";

  const view = DEFAULT_EXAMPLE;
  let flavour = $state<Flavour>(view.flavour);
  let regex = $state(view.regex);
  let operation = $state<Operation>(view.operation);
  let enabled = $state<EngineId[]>([...view.engines]);
  let drawer = $state(false);
  let regexInput: HTMLTextAreaElement | undefined = $state();

  let beamsEl: HTMLElement | undefined = $state();
  let centres = $state<number[]>([]);
  let beamsH = $state(0);

  // Measure where each lane is, so the prism's rays land on them at every width and height.
  $effect(() => {
    const el = beamsEl;
    void lanes.length; // re-measure when lanes are toggled
    if (!el) return;
    const measure = () => {
      const top = el.getBoundingClientRect().top;
      centres = [...el.querySelectorAll("li.lane")].map((li) => {
        const r = li.getBoundingClientRect();
        return r.top - top + r.height / 2;
      });
      beamsH = el.getBoundingClientRect().height;
    };
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    el.querySelectorAll("li.lane").forEach((li) => ro.observe(li));
    measure();
    return () => ro.disconnect();
  });
  const shown = $derived(SHIPPED.filter((e) => enabled.includes(e.id)));
  const lanes = $derived(shown.map((e) => ({ engine: e, lane: view.lanes.find((l) => l.engine === e.id)! })));

  function toggle(id: EngineId) {
    enabled = enabled.includes(id) ? enabled.filter((x) => x !== id) : SHIPPED.map((e) => e.id).filter((x) => x === id || enabled.includes(x));
  }

  function onkey(e: KeyboardEvent) {
    const t = e.target as HTMLElement;
    const typing = t instanceof HTMLInputElement || t instanceof HTMLTextAreaElement || t instanceof HTMLSelectElement;
    if (e.key === "Escape" && drawer) {
      drawer = false;
      return;
    }
    if (typing || e.metaKey || e.ctrlKey || e.altKey) return;
    if (e.key === "/") {
      e.preventDefault();
      regexInput?.focus();
    } else if (/^[1-4]$/.test(e.key)) {
      const id = SHIPPED.find((x) => x.key === Number(e.key))?.id;
      if (id) toggle(id);
    } else if (e.key === "w" && view.witness) {
      drawer = true;
    }
  }
</script>

<svelte:window onkeydown={onkey} />

<main class="bench">
  <h1 class="visually-hidden">Bench</h1>
  <p class="preview" role="note">
    Preview (UI0): the results on this page are a fixed example, checked on the real engines. Live
    analysis arrives in UI2.
  </p>
  <div class="canvas">
    <div class="col-input">
      <InputColumn bind:flavour bind:regex bind:operation bind:regexInput tests={view.tests} {enabled} baseline={view.baseline} ontoggle={toggle} />
    </div>
    <div class="col-prism">
      <Prism engines={shown} {centres} height={beamsH} />
    </div>
    <section class="col-beams" aria-label="Verdict per engine" bind:this={beamsEl}>
      <p class="summary" aria-live="polite">
        <svg class="glyph" width="18" height="18" viewBox="0 0 22 22" aria-hidden="true" focusable="false"><path d="M4 19 L11 4 L18 19 Z" /></svg>
        {shown.length === 0 ? "Select an engine to see its beam." : view.summary}
        <span class="count">{lanes.filter((l) => l.lane.state === "differs").length} of {shown.length} engines differ from the baseline</span>
      </p>
      <ol class="lanes">
        {#each lanes as l (l.engine.id)}
          <BeamLane engine={l.engine} lane={l.lane} baseline={view.baseline === l.engine.id} onwitness={() => (drawer = true)} />
        {/each}
      </ol>
      <p class="hint">Select a bent beam or a <em>Differs</em> badge to see the witness. Keys: <kbd>/</kbd> regex, <kbd>1</kbd>–<kbd>4</kbd> engines, <kbd>w</kbd> witness, <kbd>Esc</kbd> close.</p>
    </section>
  </div>
</main>

{#if view.witness}
  <WitnessDrawer bind:open={drawer}>
    <WitnessCard witness={view.witness} lanes={view.lanes} />
  </WitnessDrawer>
{/if}

<style>
  .bench { min-height: calc(100vh - var(--topbar-h)); }
  .preview { margin: 0; padding: var(--s-2) var(--s-4); font-size: var(--text-12); color: var(--ink-muted); border-bottom: var(--line) solid var(--hairline); background: var(--surface); }
  .canvas {
    display: grid;
    grid-template-columns: var(--input-col) var(--prism-col) minmax(0, 1fr);
    grid-template-rows: auto;
    align-items: start;
    padding: var(--s-6) var(--s-6) var(--s-12);
    min-height: calc(100vh - var(--topbar-h) - 40px);
    background-color: var(--paper);
    background-image: radial-gradient(circle, var(--grid-dot) 1px, transparent 1.5px);
    background-size: var(--grid-step) var(--grid-step);
  }
  .col-input { min-width: 0; }
  .summary { display: flex; align-items: center; gap: var(--s-2); flex-wrap: wrap; height: var(--s-12); margin: 0 0 var(--s-1); font-family: var(--font-serif); font-size: var(--text-20); }
  .summary .count { font-family: var(--font-ui); font-size: var(--text-12); color: var(--ink-muted); }
  .summary .glyph { display: none; fill: var(--surface); stroke: var(--ink); stroke-width: 1.3; }
  .lanes { list-style: none; margin: 0; padding: 0; display: grid; gap: 8px; }
  .hint { color: var(--ink-muted); font-size: var(--text-12); }
  kbd { font-size: 11px; border: var(--line) solid var(--hairline); border-radius: 3px; padding: 0 3px; background: var(--surface); }
  @media (max-width: 1279px) {
    .canvas { padding-left: var(--s-4); padding-right: var(--s-4); }
  }
  @media (max-width: 1023px) {
    .canvas { grid-template-columns: minmax(0, 1fr); padding: var(--s-4); gap: var(--s-6); }
    .col-input { padding-right: 0; }
    .col-prism { display: none; }
    .summary .glyph { display: inline; }
    .summary { height: auto; }
  }
</style>
