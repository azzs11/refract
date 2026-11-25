<!-- Top bar, 48 px (spec: Bench → Layout): wordmark, page tabs, command palette, Share. -->
<script lang="ts">
  let menuOpen = $state(false);
  let shared = $state(false);
  const TABS = [
    { label: "Bench", href: "./", ready: true },
    { label: "Lens", href: "", ready: false, when: "UI4" },
    { label: "Learn", href: "", ready: false, when: "UI5" },
    { label: "Engines", href: "", ready: false, when: "UI5" },
    { label: "About", href: "", ready: false, when: "UI5" },
  ];

  async function share() {
    try {
      await navigator.clipboard.writeText(location.href);
      shared = true;
      setTimeout(() => (shared = false), 2000);
    } catch {
      shared = false;
    }
  }
</script>

<header class="topbar">
  <a class="wordmark" href="./" aria-label="Refract, the Bench">
    <svg width="22" height="22" viewBox="0 0 22 22" aria-hidden="true" focusable="false">
      <path class="in" d="M0 12.5 H7" />
      <path class="glass" d="M4 19 L11 4 L18 19 Z" />
      <path class="out1" d="M14 13 L22 10" /><path class="out2" d="M14 14.5 L22 16" />
    </svg>
    <span>Refract</span>
  </a>
  <button class="menu-button" type="button" aria-expanded={menuOpen} aria-controls="page-tabs" onclick={() => (menuOpen = !menuOpen)}>Menu</button>
  <nav id="page-tabs" class:open={menuOpen} aria-label="Pages">
    <ul>
      {#each TABS as t (t.label)}
        <li>
          {#if t.ready}
            <a href={t.href} aria-current="page">{t.label}</a>
          {:else}
            <span class="soon" aria-disabled="true" title={`Arrives in ${t.when}`}>{t.label}</span>
          {/if}
        </li>
      {/each}
    </ul>
  </nav>
  <div class="actions">
    <button type="button" class="ghost palette" aria-disabled="true" title="Command palette arrives in UI6">
      <span>Commands</span> <kbd>⌘K</kbd>
    </button>
    <button type="button" class="ghost" onclick={share}>{shared ? "Link copied" : "Share"}</button>
  </div>
</header>

<style>
  .topbar {
    position: sticky;
    top: 0;
    z-index: 5;
    display: flex;
    align-items: center;
    gap: var(--s-6);
    height: var(--topbar-h);
    padding: 0 var(--s-4);
    background: var(--surface);
    border-bottom: var(--line) solid var(--hairline);
  }
  .wordmark { display: inline-flex; align-items: center; gap: var(--s-2); color: var(--ink); text-decoration: none; font-family: var(--font-serif); font-size: var(--text-20); font-weight: 500; }
  .wordmark svg path { fill: none; stroke: var(--ink); stroke-width: 1.3; stroke-linecap: round; stroke-linejoin: round; }
  .wordmark .glass { fill: var(--surface); }
  .wordmark .out1, .wordmark .out2 { stroke: var(--witness); }
  nav ul { display: flex; gap: var(--s-1); margin: 0; padding: 0; list-style: none; }
  nav a, nav .soon { display: inline-flex; align-items: center; height: var(--target); padding: 0 var(--s-3); border-radius: var(--radius); text-decoration: none; color: var(--ink-muted); }
  nav a[aria-current="page"] { color: var(--ink); font-weight: 600; box-shadow: inset 0 -2px 0 var(--ink); border-radius: 0; }
  nav .soon { cursor: default; }
  .actions { margin-left: auto; display: flex; gap: var(--s-2); }
  .ghost { display: inline-flex; align-items: center; gap: var(--s-2); height: var(--target); min-width: var(--target); padding: 0 var(--s-3); border: var(--line) solid var(--hairline); border-radius: var(--radius); background: var(--surface); cursor: pointer; transition: border-color var(--t-hover) var(--ease); }
  .ghost:hover { border-color: var(--ink-muted); }
  .palette[aria-disabled="true"] { color: var(--ink-muted); cursor: default; }
  kbd { font-size: var(--text-12); padding: 0 var(--s-1); border: var(--line) solid var(--hairline); border-radius: 4px; }
  .menu-button { display: none; }
  @media (max-width: 639px) {
    .menu-button { display: inline-flex; align-items: center; height: var(--target); padding: 0 var(--s-3); border: var(--line) solid var(--hairline); border-radius: var(--radius); background: var(--surface); }
    nav { display: none; position: absolute; top: var(--topbar-h); left: 0; right: 0; background: var(--surface); border-bottom: var(--line) solid var(--hairline); }
    nav.open { display: block; }
    nav ul { flex-direction: column; padding: var(--s-2); }
    .palette span { display: none; }
    .topbar { gap: var(--s-2); }
  }
</style>
