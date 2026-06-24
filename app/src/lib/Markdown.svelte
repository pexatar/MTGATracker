<script module lang="ts">
  // A small, dependency-free renderer for the limited Markdown the local AI
  // emits (headings, bold/italic, inline code, ordered/unordered lists,
  // paragraphs). The input is escaped first, so rendering it with {@html} is
  // safe: no raw HTML from the model can reach the DOM.

  // Sentinel used to park code-span contents; a NUL byte cannot appear in the
  // escaped text, so it never collides with real prose (e.g. "turn 3 ...").
  const NUL = String.fromCharCode(0);

  function escapeHtml(s: string): string {
    return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  }

  /// Inline formatting on an already HTML-escaped string. Code spans are pulled
  /// out behind the NUL sentinel so the bold/italic passes and ordinary digits
  /// in the prose never alter their contents.
  function inline(s: string): string {
    const codes: string[] = [];
    let out = s.replace(/`([^`]+)`/g, (_, c) => `${NUL}${codes.push(c) - 1}${NUL}`);
    out = out.replace(/\*\*([^*]+?)\*\*/g, "<strong>$1</strong>");
    out = out.replace(/\*([^*\n]+?)\*/g, "<em>$1</em>");
    out = out.replace(/_([^_\n]+?)_/g, "<em>$1</em>");
    return out.replace(new RegExp(`${NUL}(\\d+)${NUL}`, "g"), (_, i) => `<code>${codes[+i]}</code>`);
  }

  export function renderMarkdown(src: string): string {
    const lines = escapeHtml(src ?? "").replace(/\r\n?/g, "\n").split("\n");
    const out: string[] = [];
    let para: string[] = [];
    // Open lists, innermost last, tracked by their indentation depth.
    const stack: { indent: number; tag: "ul" | "ol" }[] = [];

    const flushPara = () => {
      if (para.length) {
        out.push(`<p>${para.map(inline).join("<br>")}</p>`);
        para = [];
      }
    };
    const closeListsTo = (indent: number) => {
      while (stack.length && stack[stack.length - 1].indent >= indent) {
        out.push(`</li></${stack.pop()!.tag}>`);
      }
    };

    const heading = /^(#{1,6})\s+(.*)$/;
    const item = /^(\s*)([-*+]|\d+[.)])\s+(.*)$/;

    for (const line of lines) {
      if (line.trim() === "") {
        flushPara();
        continue;
      }

      const h = heading.exec(line);
      if (h) {
        flushPara();
        closeListsTo(0);
        const level = h[1].length;
        out.push(`<h${level}>${inline(h[2])}</h${level}>`);
        continue;
      }

      const it = item.exec(line);
      if (it) {
        flushPara();
        const indent = it[1].replace(/\t/g, "    ").length;
        const ordered = /\d/.test(it[2]);
        const text = inline(it[3]);
        // Close any lists deeper than this item, then either continue the list
        // at this level or open a new (nested) one.
        while (stack.length && stack[stack.length - 1].indent > indent) {
          out.push(`</li></${stack.pop()!.tag}>`);
        }
        if (stack.length && stack[stack.length - 1].indent === indent) {
          out.push(`</li><li>${text}`);
        } else {
          const tag = ordered ? "ol" : "ul";
          out.push(`<${tag}><li>${text}`);
          stack.push({ indent, tag });
        }
        continue;
      }

      // Plain text: a paragraph ends any open list.
      closeListsTo(0);
      para.push(line.trim());
    }

    flushPara();
    closeListsTo(0);
    return out.join("");
  }
</script>

<script lang="ts">
  let { source = "" }: { source?: string } = $props();
  const html = $derived(renderMarkdown(source));
</script>

<div class="md">{@html html}</div>

<style>
  .md :global(h1),
  .md :global(h2),
  .md :global(h3),
  .md :global(h4),
  .md :global(h5),
  .md :global(h6) {
    font-weight: 600;
    margin: 0.75rem 0 0.35rem;
    line-height: 1.25;
  }
  .md :global(h1) { font-size: 1.15rem; }
  .md :global(h2) { font-size: 1.05rem; }
  .md :global(h3) { font-size: 0.98rem; }
  .md :global(h4),
  .md :global(h5),
  .md :global(h6) { font-size: 0.92rem; }
  .md :global(p) { margin: 0.4rem 0; }
  .md :global(ul),
  .md :global(ol) { margin: 0.35rem 0; padding-left: 1.35rem; }
  .md :global(ul) { list-style: disc; }
  .md :global(ol) { list-style: decimal; }
  .md :global(li) { margin: 0.15rem 0; }
  .md :global(strong) { font-weight: 600; }
  .md :global(em) { font-style: italic; }
  .md :global(code) {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 0.85em;
    background: var(--color-surface, rgba(127, 127, 127, 0.15));
    padding: 0.05rem 0.3rem;
    border-radius: 0.25rem;
  }
  /* Trim the outer margins so the block sits flush in its container. */
  .md :global(> :first-child) { margin-top: 0; }
  .md :global(> :last-child) { margin-bottom: 0; }
</style>
