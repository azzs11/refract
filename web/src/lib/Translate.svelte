<script lang="ts">
  import type { Flavor, Op, Translation } from "./api";
  import { ENGINE, visible } from "./engines";
  import { browserEngine, jsMatches } from "./live";
  import Shape from "./Shape.svelte";

  let { result, from, pattern }: { result: Translation; from: Flavor; pattern: string } = $props();
  let copied = $state("");
  let expanded = $state<Record<string, boolean>>({});
  const LONG = 160;

  /** Replays the coverage strings on this browser for the JavaScript side of a translation. */
  function live(text: string, to: Flavor, op: string, cases: { text: string; matches: boolean }[]): string | null {
    const js = to === "js" ? text : from === "js" ? pattern : null;
    if (js === null || cases.length === 0) return null;
    for (const c of cases) {
      const got = jsMatches(js, op === "search" ? "search" : "full" as Op, c.text);
      if (typeof got === "string") return `live check failed: ${got}`;
      if (got !== c.matches) return `this browser disagrees on ${visible(c.text)}: engines differ`;
    }
    return `${cases.length} test strings confirmed live on ${browserEngine()}`;
  }

  async function copy(text: string) {
    await navigator.clipboard?.writeText(text);
    copied = text;
  }
</script>

<ol class="beams translate" aria-label="Translation per engine">
  {#each result.targets ?? [] as t (t.flavor)}
    {@const e = ENGINE[t.flavor]}
    <li class="beam" class:bent={t.status !== "verified"} style={`--c:${e.color}`}>
      <div class="who">
        <Shape shape={e.shape} color={e.color} />
        <strong>{e.name}</strong>
        <span class="muted">{e.engine}</span>
      </div>
      <div class="verdict">
        {#if t.status === "verified" || t.status === "not-verified"}
          {#if t.status === "verified"}
            <span class="tag ok seal">verified · {t.scope}</span>
          {:else}
            <span class="tag warn">not verified</span>
          {/if}
          {@const long = (t.text ?? "").length > LONG}
          <code class="out" class:clamped={long && !expanded[t.flavor]}>{t.text}</code>
          {#if long}
            <button type="button" class="link" aria-expanded={!!expanded[t.flavor]}
              onclick={() => (expanded[t.flavor] = !expanded[t.flavor])}>
              {expanded[t.flavor] ? "show less" : `show all ${t.text!.length} characters`}
            </button>
            <span class="muted note">Long because this engine's Unicode tables differ from the source's: the explicit ranges are the characters its shorthands miss.</span>
          {/if}
          <button type="button" class="link" onclick={() => copy(t.text!)}>{copied === t.text ? "copied" : "copy"}</button>
          <ul class="checks">
            {#each t.checks ?? [] as c (c.op)}
              {@const l = live(t.text!, t.flavor, c.op, c.cases)}
              <li>
                <span class="muted">{c.op}:</span> {c.verdict}
                {#if c.witness}on <code class="witness">{visible(c.witness.text)}</code>{/if}
                {#if l}<span class="evidence">{l}</span>{/if}
              </li>
            {/each}
          </ul>
        {:else if t.status === "untranslatable"}
          <span class="tag bad">no equivalent</span>
          <span>{t.construct}</span>
          {#if t.span && t.span[1] > t.span[0]}<span class="muted">at <code>{pattern.slice(t.span[0], t.span[1])}</code></span>{/if}
        {:else if t.status === "printer-bug"}
          <span class="tag bad">printer bug</span> <code>{t.text}</code> <span class="muted">{t.note}</span>
        {:else}
          <span class="tag">not analysed</span> <span class="muted">{t.note}</span>
        {/if}
      </div>
    </li>
  {/each}
</ol>
<p class="fineprint">
  Each translation is printed from the analysed meaning, re-parsed by the target's own front-end and
  proven equivalent to the source (ADR-0013); only then is it <em>verified</em>. JavaScript sides are
  replayed live on this browser; <code>refract translate</code> replays every engine.
</p>
