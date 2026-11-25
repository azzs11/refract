<!-- Input column (spec: Bench → Layout): source flavour, regex editor, operation, flags, test
     strings, engine chips. UI0: the controls hold local state; results are the fixed example. -->
<script lang="ts">
  import { ENGINES, SHIPPED, engineVars, type EngineId } from "$lib/engines";
  import type { Flavour, Operation } from "$lib/types";
  import EngineShape from "./EngineShape.svelte";
  import InvisibleText from "./InvisibleText.svelte";

  let {
    flavour = $bindable(),
    regex = $bindable(),
    operation = $bindable(),
    tests,
    enabled,
    baseline,
    ontoggle,
    regexInput = $bindable(),
  }: {
    flavour: Flavour;
    regex: string;
    operation: Operation;
    tests: string[];
    enabled: EngineId[];
    baseline: EngineId;
    ontoggle: (id: EngineId) => void;
    regexInput?: HTMLTextAreaElement;
  } = $props();

  const FLAVOURS: { id: Flavour; label: string }[] = [
    { id: "js", label: "JavaScript" },
    { id: "python", label: "Python re" },
    { id: "go", label: "Go (RE2)" },
    { id: "java", label: "Java" },
  ];
  const OPS: { id: Operation; label: string }[] = [
    { id: "full", label: "Full match" },
    { id: "search", label: "Search" },
    { id: "start", label: "Match at start" },
  ];
  const rows = $derived(Math.max(3, regex.split("\n").length));
</script>

<section class="inputs" aria-label="Regex and test strings">
  <label class="field">
    <span class="label">Written for</span>
    <select bind:value={flavour}>
      {#each FLAVOURS as f (f.id)}<option value={f.id}>{f.label}</option>{/each}
    </select>
  </label>

  <label class="field">
    <span class="label">Regex <kbd>/</kbd></span>
    <textarea class="code editor" bind:this={regexInput} bind:value={regex} {rows} spellcheck="false" autocomplete="off" autocapitalize="off"></textarea>
  </label>

  <fieldset class="field">
    <legend class="label">Operation</legend>
    <div class="segmented">
      {#each OPS as o (o.id)}
        <label class:on={operation === o.id}><input type="radio" name="operation" value={o.id} bind:group={operation} />{o.label}</label>
      {/each}
    </div>
  </fieldset>

  <details class="field flags">
    <summary><span class="label">Flags</span> <span class="muted">none</span></summary>
    <p class="muted small">Flag toggles (i, m, s, x, u) arrive with the store in UI1.</p>
  </details>

  <div class="field">
    <div class="label-row"><span class="label">Test strings</span>
      <span class="tools"><button type="button" class="mini" aria-disabled="true" title="Arrives in UI1">Add</button><button type="button" class="mini" aria-disabled="true" title="Arrives in UI1">Import</button></span>
    </div>
    <ol class="tests">
      {#each tests as t, i (i)}
        <li><span class="index">{i + 1}</span><span class="test"><InvisibleText text={t} /></span></li>
      {/each}
    </ol>
  </div>

  <fieldset class="field">
    <legend class="label">Engines <span class="muted">(keys 1–4)</span></legend>
    <ul class="engines">
      {#each SHIPPED as e (e.id)}
        <li>
          <button type="button" class="engine" style={engineVars(e)} aria-pressed={enabled.includes(e.id)} onclick={() => ontoggle(e.id)}>
            <EngineShape engine={e} />
            <span>{e.name}</span>
            {#if baseline === e.id}<span aria-label="baseline" title="Baseline">★</span>{/if}
            <kbd>{e.key}</kbd>
          </button>
        </li>
      {/each}
      {#each ENGINES.filter((e) => e.ships !== "v0.5") as e (e.id)}
        <li><span class="engine later" style={engineVars(e)} title={`Joins in ${e.ships}`}><EngineShape engine={e} /><span>{e.name}</span><span class="muted small">{e.ships}</span></span></li>
      {/each}
    </ul>
  </fieldset>
</section>

<style>
  .inputs { display: grid; gap: var(--s-4); align-content: start; padding: var(--s-4); background: var(--surface); border: var(--line) solid var(--hairline); border-radius: var(--radius); }
  .field { display: grid; gap: var(--s-1); margin: 0; padding: 0; border: 0; min-width: 0; }
  .label { font-size: var(--text-12); font-weight: 500; color: var(--ink-muted); }
  .label-row { display: flex; align-items: center; justify-content: space-between; }
  .muted { color: var(--ink-muted); font-weight: 400; }
  .small { font-size: var(--text-12); margin: 0; }
  select, .editor { width: 100%; border: var(--line) solid var(--hairline); border-radius: var(--radius); background: var(--paper); }
  select { height: var(--target); padding: 0 var(--s-2); }
  .editor { padding: var(--s-2) var(--s-3); font-size: var(--text-14); line-height: var(--leading-code); resize: vertical; }
  .segmented { display: flex; border: var(--line) solid var(--hairline); border-radius: var(--radius); overflow: hidden; }
  .segmented label { flex: 1; position: relative; display: flex; align-items: center; justify-content: center; min-height: var(--target); padding: 0 var(--s-2); font-size: var(--text-12); cursor: pointer; border-left: var(--line) solid var(--hairline); text-align: center; }
  .segmented label:first-child { border-left: 0; }
  .segmented label.on { background: var(--ink); color: var(--surface); font-weight: 600; }
  .segmented input { position: absolute; opacity: 0; inset: 0; margin: 0; cursor: pointer; }
  .segmented label:has(input:focus-visible) { outline: 2px solid var(--focus); outline-offset: -2px; }
  .flags summary { cursor: pointer; list-style: none; min-height: 28px; display: flex; gap: var(--s-2); align-items: center; }
  .tests { list-style: none; margin: 0; padding: 0; display: grid; gap: var(--s-1); }
  .tests li { display: flex; align-items: center; gap: var(--s-2); padding: var(--s-1) var(--s-2); border: var(--line) solid var(--hairline); border-radius: var(--radius); background: var(--paper); }
  .index { font-size: var(--text-12); color: var(--ink-muted); width: 1.5em; }
  .tools { display: flex; gap: var(--s-1); }
  .mini { min-height: var(--target); padding: 0 var(--s-3); font-size: var(--text-12); border: var(--line) solid var(--hairline); border-radius: var(--radius); background: var(--surface); color: var(--ink-muted); }
  .engines { list-style: none; margin: 0; padding: 0; display: flex; flex-wrap: wrap; gap: var(--s-2); }
  .engine { display: inline-flex; align-items: center; gap: var(--s-2); min-height: var(--target); padding: 0 var(--s-3); border: var(--line) solid var(--hairline); border-radius: var(--radius-pill); background: var(--surface); color: var(--eng-ink); font-weight: 500; cursor: pointer; }
  .engine[aria-pressed="true"] { border-color: var(--eng-ink); }
  .engine[aria-pressed="false"] { color: var(--ink-muted); text-decoration: line-through; }
  .engine kbd { font-size: 11px; color: var(--ink-muted); border: var(--line) solid var(--hairline); border-radius: 3px; padding: 0 3px; }
  .engine.later { cursor: default; color: var(--ink-muted); border-style: dashed; }
  kbd { font-size: 11px; }
</style>
