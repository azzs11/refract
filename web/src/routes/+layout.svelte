<script lang="ts">
  import "@fontsource-variable/inter";
  import "@fontsource-variable/newsreader";
  import "@fontsource-variable/jetbrains-mono";
  import "$lib/styles/tokens.css";
  import PatternDefs from "$lib/components/PatternDefs.svelte";
  import TopNav from "$lib/components/TopNav.svelte";
  import type { Snippet } from "svelte";

  let { children }: { children: Snippet } = $props();
</script>

<svelte:head><title>Refract: the Bench</title></svelte:head>

<PatternDefs />
<TopNav />
{@render children()}
