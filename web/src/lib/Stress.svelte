<script lang="ts">
  import type { Op, Redos } from "./api";
  import { visible } from "./engines";
  import { browserEngine, timeAttack } from "./live";

  let { pred, pattern, op, from }: { pred: Redos; pattern: string; op: Op; from: string } = $props();
  let points = $state<{ n: number; len: number; ms: number | null }[]>([]);
  let running = $state(false);

  const build = (n: number) => {
    const a = pred.attack!;
    return a.prefix + a.pumps.map((p) => p.repeat(n)).join("") + a.suffix;
  };

  async function measure() {
    points = [];
    running = true;
    const exp = pred.growth === "exponential";
    const ns = exp ? Array.from({ length: 20 }, (_, i) => 2 * (i + 1)) : Array.from({ length: 12 }, (_, i) => 2 ** (i + 4));
    await timeAttack(pattern, op, build, ns, (n, ms) => { points = [...points, { n, len: build(n).length, ms }]; });
    running = false;
  }

  const fmt = (ms: number | null) => (ms === null ? "timed out (2 s)" : ms < 1 ? `${(ms * 1000).toFixed(0)} µs` : `${ms.toFixed(1)} ms`);
  const maxMs = $derived(Math.max(1, ...points.map((p) => p.ms ?? 2000)));
</script>

<div class="stress">
  <p><span class="tag" class:bad={pred.growth !== "linear"}>{pred.growth}</span> {pred.reason}</p>
  {#if pred.attack}
    <p>Attack: <code class="witness">{visible(pred.attack.prefix)}</code> + (<code class="witness">{visible(pred.attack.pumps.join(""))}</code>)×n + <code class="witness">{visible(pred.attack.suffix)}</code></p>
    {#if from === "js"}
      <button type="button" onclick={measure} disabled={running}>{running ? "measuring…" : `Measure on ${browserEngine()}`}</button>
    {:else}
      <p class="muted">Timing other engines needs them installed: <code>refract redos -f {from} '{pattern}'</code></p>
    {/if}
    {#if points.length}
      <table>
        <thead><tr><th>n</th><th>length</th><th>time</th><th aria-hidden="true"></th></tr></thead>
        <tbody>
          {#each points as p (p.n)}
            <tr><td>{p.n}</td><td>{p.len}</td><td>{fmt(p.ms)}</td>
              <td aria-hidden="true"><span class="bar" style={`width:${Math.max(2, (100 * (p.ms ?? 2000)) / maxMs)}%`}></span></td></tr>
          {/each}
        </tbody>
      </table>
    {/if}
  {/if}
  <p class="fineprint">Predictions model each engine (CPython, V8, the JDK's loop memo; Go and Rust are linear). Measured confirmation on every engine: <code>refract redos</code> (ADR-0012).</p>
</div>
