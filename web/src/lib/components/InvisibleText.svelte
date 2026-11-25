<!-- Draws characters a reader could miss (spec: Type): newline as a return glyph, tab as an
     arrow, non-breaking space as a boxed dot; other controls as U+XXXX. Glyphs are an overlay
     style, not font-dependent, and carry a spoken name. -->
<script lang="ts">
  let { text }: { text: string } = $props();

  type Piece = { kind: "text"; value: string } | { kind: "glyph"; glyph: string; name: string };

  const NAMES: Record<string, [string, string]> = {
    "\n": ["↵", "newline"],
    "\r": ["␍", "carriage return"],
    "\t": ["→", "tab"],
    " ": ["·", "non-breaking space"],
    " ": ["⤶", "line separator"],
    " ": ["¶", "paragraph separator"],
    "\u0085": ["⏎", "next line"],
    "﻿": ["◌", "byte order mark"],
    "​": ["◌", "zero-width space"],
  };

  const pieces = $derived.by(() => {
    const out: Piece[] = [];
    for (const ch of text) {
      const known = NAMES[ch];
      const cp = ch.codePointAt(0) ?? 0;
      if (known) out.push({ kind: "glyph", glyph: known[0], name: known[1] });
      else if (cp < 0x20 || cp === 0x7f)
        out.push({ kind: "glyph", glyph: `U+${cp.toString(16).toUpperCase().padStart(4, "0")}`, name: `control character U+${cp.toString(16).toUpperCase().padStart(4, "0")}` });
      else {
        const last = out[out.length - 1];
        if (last && last.kind === "text") last.value += ch;
        else out.push({ kind: "text", value: ch });
      }
    }
    return out;
  });
</script>

<span class="mono invisible-text">{#if text === ""}<span class="empty">(empty string)</span>{/if}{#each pieces as p, i (i)}{#if p.kind === "text"}{p.value}{:else}<span class="glyph" role="img" aria-label={p.name} title={p.name}>{p.glyph}</span>{/if}{/each}</span>

<style>
  .invisible-text { white-space: pre; }
  .glyph {
    display: inline-block;
    margin: 0 1px;
    padding: 0 3px;
    border: var(--line) solid var(--hairline);
    border-radius: 3px;
    color: var(--ink-muted);
    font-size: 0.85em;
    line-height: 1.2;
    vertical-align: 0.05em;
  }
  .empty { color: var(--ink-muted); font-style: italic; }
</style>
