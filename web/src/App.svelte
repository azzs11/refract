<script lang="ts">
  import { ALL, dfa, redos, refract, translate } from "./lib/api";
  import type { DfaView, Flavor, Op, Redos, Refraction, Translation } from "./lib/api";
  import { ENGINE } from "./lib/engines";
  import Bench from "./lib/Bench.svelte";
  import Lens from "./lib/Lens.svelte";
  import Stress from "./lib/Stress.svelte";
  import Translate from "./lib/Translate.svelte";

  const DEMO = "^\\w+@\\w+\\.com$";
  const hash = new URLSearchParams(location.hash.slice(1));
  let pattern = $state(hash.get("p") ?? DEMO);
  let from = $state<Flavor>((ALL as string[]).includes(hash.get("f") ?? "") ? (hash.get("f") as Flavor) : "python");
  let op = $state<Op>(hash.get("op") === "search" ? "search" : "full");
  let tab = $state<"bench" | "lens" | "stress" | "translate">("bench");
  let result = $state<Refraction | null>(null);
  let ms = $state(0);
  let lensView = $state<DfaView | null>(null);
  let lensWitness = $state("");
  let stress = $state<Redos | null>(null);
  let translation = $state<Translation | null>(null);
  let input: HTMLInputElement | undefined = $state();

  let seq = 0;
  $effect(() => {
    const p = pattern, f = from, o = op;
    history.replaceState(null, "", `#${new URLSearchParams({ p, f, op: o })}`);
    const mine = ++seq;
    const t = setTimeout(async () => {
      const r = await refract(f, p, o);
      if (mine !== seq) return;
      result = r.value;
      ms = r.ms;
      stress = null;
      lensView = null;
      translation = null;
      if (tab === "translate") void openTranslate();
      else tab = "bench";
    }, 150);
    return () => clearTimeout(t);
  });

  async function openLens(w: string) {
    lensWitness = w;
    lensView = (await dfa(from, pattern, op, w)).value;
    tab = "lens";
  }

  async function openStress() {
    stress = (await redos(from, pattern, op)).value;
    tab = "stress";
  }

  async function openTranslate() {
    translation = (await translate(from, pattern)).value;
    tab = "translate";
  }

  function onkey(e: KeyboardEvent) {
    const typing = document.activeElement instanceof HTMLInputElement || document.activeElement instanceof HTMLSelectElement;
    if (e.key === "/" && !typing) { e.preventDefault(); input?.focus(); return; }
    if (typing) return;
    if (/^[1-5]$/.test(e.key)) document.getElementById(`row-${e.key}`)?.focus();
    if (e.key === "w") {
      const w = result?.engines?.find((r) => r.witness)?.witness;
      if (w) navigator.clipboard?.writeText(w.text);
    }
  }
  const differs = $derived(result?.engines?.filter((r) => r.verdict === "differs" || r.verdict === "rejects").length ?? 0);
</script>

<svelte:window onkeydown={onkey} />

<header>
  <h1>Refract <span class="muted">optics bench</span></h1>
  <p class="tagline">Same pattern, different meaning: see exactly where.</p>
</header>

<main>
  <form class="prism" onsubmit={(e) => e.preventDefault()}>
    <label class="grow">
      <span>Pattern <kbd>/</kbd></span>
      <input bind:this={input} bind:value={pattern} spellcheck="false" autocomplete="off" aria-describedby="perr" />
    </label>
    <label>
      <span>Written for</span>
      <select bind:value={from}>
        {#each ALL as f (f)}<option value={f}>{ENGINE[f].name}</option>{/each}
      </select>
    </label>
    <fieldset>
      <legend>Operation</legend>
      <label><input type="radio" bind:group={op} value="full" /> full match</label>
      <label><input type="radio" bind:group={op} value="search" /> search</label>
    </fieldset>
  </form>

  {#if result && !result.ok}
    <pre id="perr" class="error" role="alert">{result.error?.rendered ?? result.error?.message}</pre>
  {:else if result?.engines}
    <nav aria-label="Views">
      <button type="button" class:on={tab === "bench"} onclick={() => (tab = "bench")}>Bench</button>
      <button type="button" class:on={tab === "lens"} onclick={() => openLens(result?.engines?.find((r) => r.witness)?.witness?.text ?? "")}>Automaton lens</button>
      <button type="button" class:on={tab === "stress"} onclick={openStress}>Stress lab</button>
      <button type="button" class:on={tab === "translate"} onclick={openTranslate}>Translate</button>
      <span class="muted status" aria-live="polite">
        {differs === 0 ? "no engine bends this beam" : `${differs} of ${result.engines.length - 1} engines bend the beam`} · {ms.toFixed(0)} ms in WebAssembly
      </span>
    </nav>
    {#if tab === "bench"}
      <Bench rows={result.engines} {from} {pattern} {op} onlens={openLens} />
    {:else if tab === "lens" && lensView}
      {#if lensView.ok}<Lens view={lensView} witness={lensWitness} />{:else}<p class="error">{lensView.error?.message}</p>{/if}
    {:else if tab === "stress" && stress}
      {#if stress.ok}<Stress pred={stress} {pattern} {op} {from} />{:else}<p class="error">{stress.error?.message}</p>{/if}
    {:else if tab === "translate" && translation}
      {#if translation.ok}<Translate result={translation} {from} {pattern} />{:else}<p class="error">{translation.error?.message}</p>{/if}
    {/if}
  {:else}
    <p class="muted" aria-live="polite">Loading the analyser…</p>
  {/if}
</main>

<footer class="fineprint">
  Everything runs in your browser; nothing is sent anywhere. Keys: <kbd>/</kbd> pattern ·
  <kbd>1</kbd>–<kbd>5</kbd> engines · <kbd>w</kbd> copy witness. The URL is a permalink.
</footer>
