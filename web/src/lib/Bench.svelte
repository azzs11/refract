<script lang="ts">
  import type { EngineRow, Flavor, Op } from "./api";
  import { ENGINE, visible } from "./engines";
  import { browserEngine, jsMatches } from "./live";
  import Shape from "./Shape.svelte";

  let { rows, from, pattern, op, onlens }: {
    rows: EngineRow[]; from: Flavor; pattern: string; op: Op; onlens: (w: string) => void;
  } = $props();

  const word = (m: boolean) => (m ? "matches" : "no match");

  /** Live evidence for rows involving JavaScript: replay the witness on this browser. */
  function live(r: EngineRow): string | null {
    if (r.verdict !== "differs" || !r.witness || (from !== "js" && r.flavor !== "js")) return null;
    const expected = from === "js" ? r.source_matches : !r.source_matches;
    const got = jsMatches(pattern, op, r.witness.text);
    if (typeof got === "string") return `live check failed: ${got}`;
    return got === expected
      ? `confirmed live on ${browserEngine()}`
      : `this browser disagrees with the V8 model (${word(got)}): engines differ`;
  }
</script>

<ol class="beams" aria-label="Verdict per engine">
  {#each rows as r, i (r.flavor)}
    {@const e = ENGINE[r.flavor]}
    <li class="beam" class:bent={r.verdict === "differs" || r.verdict === "rejects"} tabindex="-1" id={`row-${i + 1}`}
        style={`--c:${e.color}`}>
      <div class="who">
        <Shape shape={e.shape} color={e.color} />
        <strong>{e.name}</strong>
        <span class="muted">{e.engine}</span>
      </div>
      <div class="verdict">
        {#if r.verdict === "source"}
          <span class="tag">source</span> <span class="muted">the pattern as written</span>
        {:else if r.verdict === "same"}
          <span class="tag ok">same meaning</span> <span class="muted">{r.note}</span>
        {:else if r.verdict === "same-bounded"}
          <span class="tag warn">no difference up to length {r.max_len}</span>
          <span class="muted">back-references are not regular: bounded, not proven</span>
        {:else if r.verdict === "differs" && r.witness}
          <span class="tag bad">differs</span>
          <span>on <code class="witness">{visible(r.witness.text)}</code></span>
          <span class="muted">
            {ENGINE[from].name} {word(!!r.source_matches)}, {e.name} {word(!r.source_matches)}
          </span>
          <button type="button" class="link" onclick={() => onlens(r.witness!.text)}>see the path</button>
          {@const l = live(r)}
          {#if l}<span class="evidence">{l}</span>{/if}
        {:else if r.verdict === "rejects"}
          <span class="tag bad">rejects the pattern</span>
          <span class="muted">{r.error?.message}</span>
        {:else}
          <span class="tag">not analysed</span> <span class="muted">{r.note}</span>
        {/if}
      </div>
    </li>
  {/each}
</ol>
<p class="fineprint">
  Verdicts are computed in this page (WebAssembly). JavaScript witnesses are replayed live on this
  browser; the other engines are confirmed by the CLI (<code>refract refract</code>) on the real engines.
</p>
