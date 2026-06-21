<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { onMount } from "svelte";
  import type { ChartConfiguration } from "chart.js/auto";
  import { chartjs } from "$lib/chartAction";

  type Card = {
    id: string;
    name: string;
    set_code: string;
    set_name: string | null;
    collector_number: string;
    mana_cost: string | null;
    cmc: number;
    type_line: string | null;
    colors: string[];
    color_identity: string[];
    rarity: string;
    arena_id: number | null;
    image_small: string | null;
    image_normal: string | null;
    legalities: Record<string, string>;
  };

  type DatabaseStatus = {
    card_count: number;
    last_updated: string | null;
    source_updated_at: string | null;
  };

  type Progress = { phase: string; current: number; total: number };

  type UpdateCheck = {
    known_count: number;
    available_count: number;
    new_cards: number;
    update_available: boolean;
  };

  type DeckSection = "commander" | "companion" | "main" | "sideboard";

  type DeckEntry = {
    quantity: number;
    name: string;
    set_code: string | null;
    collector_number: string | null;
    section: DeckSection;
    card: Card | null;
    matched: boolean;
  };

  type ParsedDeck = {
    entries: DeckEntry[];
    total_cards: number;
    unmatched: number;
  };

  type NamedCount = { label: string; count: number };
  type CurveBucket = { cmc: number; count: number };
  type FormatLegality = { format: string; illegal: string[] };
  type DeckAnalysis = {
    total_cards: number;
    lands: number;
    nonlands: number;
    average_cmc: number;
    mana_curve: CurveBucket[];
    color_pips: NamedCount[];
    type_distribution: NamedCount[];
    rarity_distribution: NamedCount[];
    format_legality: FormatLegality[];
  };

  let status = $state<DatabaseStatus | null>(null);
  let updating = $state(false);
  let progress = $state<Progress | null>(null);
  let error = $state("");
  let updateInfo = $state<UpdateCheck | null>(null);

  let query = $state("");
  let results = $state<Card[]>([]);
  let selected = $state<Card | null>(null);
  let searching = $state(false);
  let searchTimer: ReturnType<typeof setTimeout> | undefined;

  let deckText = $state("");
  let deck = $state<ParsedDeck | null>(null);
  let importing = $state(false);
  let deckError = $state("");
  let copyMsg = $state("");
  let analysis = $state<DeckAnalysis | null>(null);
  let selectedFormat = $state("");

  type DeckSummary = { id: number; name: string; updated_at: string };
  let deckId = $state<number | null>(null);
  let deckName = $state("");
  let savedDecks = $state<DeckSummary[]>([]);
  let deckMsg = $state("");

  const sectionOrder: DeckSection[] = ["commander", "companion", "main", "sideboard"];
  const sectionLabels: Record<DeckSection, string> = {
    commander: "Commander",
    companion: "Companion",
    main: "Deck",
    sideboard: "Sideboard",
  };

  onMount(async () => {
    await loadStatus();
    await listen<Progress>("db-progress", (event) => {
      progress = event.payload;
    });
    try {
      // Make sure set names are available (fetched once if missing).
      await invoke("ensure_set_names");
    } catch {
      // Offline: set names will simply be missing until next time.
    }
    await checkUpdates();
    await loadSavedDecks();
  });

  function setLabel(card: Card): string {
    const code = card.set_code.toUpperCase();
    return card.set_name ? `${card.set_name} (${code}) ${card.collector_number}` : `${code} ${card.collector_number}`;
  }

  async function loadStatus() {
    try {
      status = await invoke<DatabaseStatus>("get_database_status");
    } catch (e) {
      error = String(e);
    }
  }

  async function checkUpdates() {
    if (!status || status.card_count === 0) {
      updateInfo = null;
      return;
    }
    try {
      updateInfo = await invoke<UpdateCheck>("check_for_updates");
    } catch {
      // Offline or network error: do not bother the user.
      updateInfo = null;
    }
  }

  async function runUpdate() {
    error = "";
    updating = true;
    progress = { phase: "index", current: 0, total: 0 };
    try {
      status = await invoke<DatabaseStatus>("update_card_database");
    } catch (e) {
      error = String(e);
    } finally {
      updating = false;
      progress = null;
      updateInfo = null;
    }
  }

  function onQueryInput() {
    clearTimeout(searchTimer);
    const q = query.trim();
    if (q.length < 2) {
      results = [];
      return;
    }
    searchTimer = setTimeout(doSearch, 200);
  }

  async function doSearch() {
    searching = true;
    try {
      results = await invoke<Card[]>("search_cards", { query, limit: 30 });
    } catch (e) {
      error = String(e);
    } finally {
      searching = false;
    }
  }

  async function onDeckFile(event: Event) {
    const input = event.target as HTMLInputElement;
    const file = input.files?.[0];
    if (!file) return;
    deckText = await file.text();
    input.value = "";
    await importDeck();
  }

  async function importDeck() {
    if (!deckText.trim()) return;
    importing = true;
    deckError = "";
    copyMsg = "";
    try {
      deck = await invoke<ParsedDeck>("import_deck", { text: deckText });
      deckId = null;
      analysis = await invoke<DeckAnalysis>("analyze_deck", { deck });
      const formats = analysis.format_legality.map((f) => f.format);
      selectedFormat = formats.includes("brawl") ? "brawl" : (formats[0] ?? "");
    } catch (e) {
      deckError = String(e);
      analysis = null;
    } finally {
      importing = false;
    }
  }

  async function copyDeck() {
    if (!deck) return;
    try {
      const text = await invoke<string>("export_deck", { deck });
      await navigator.clipboard.writeText(text);
      copyMsg = "Copied to clipboard!";
      setTimeout(() => (copyMsg = ""), 2000);
    } catch (e) {
      deckError = String(e);
    }
  }

  function entriesOf(section: DeckSection): DeckEntry[] {
    return deck ? deck.entries.filter((e) => e.section === section) : [];
  }

  function sectionCount(section: DeckSection): number {
    return entriesOf(section).reduce((sum, e) => sum + e.quantity, 0);
  }

  const AXIS_COLOR = "#c9c9d1";
  const GRID_COLOR = "rgba(255,255,255,0.08)";
  const COLOR_MAP: Record<string, string> = {
    W: "#e9e3c8",
    U: "#2a6fb0",
    B: "#7a6a86",
    R: "#c44a37",
    G: "#3f8f54",
  };
  const COLOR_NAMES: Record<string, string> = {
    W: "White",
    U: "Blue",
    B: "Black",
    R: "Red",
    G: "Green",
  };
  const RARITY_MAP: Record<string, string> = {
    common: "#8a8a93",
    uncommon: "#9bb7c4",
    rare: "#d6b24a",
    mythic: "#e0682a",
  };
  const TYPE_PALETTE = ["#3a6df0", "#1d9e75", "#d85a30", "#9a7bd0", "#d6b24a", "#5dcaa5", "#c44a37", "#888780", "#b4b2a9"];
  const FORMAT_LABELS: Record<string, string> = {
    standard: "Standard",
    alchemy: "Alchemy",
    pioneer: "Pioneer",
    historic: "Historic",
    timeless: "Timeless",
    brawl: "Brawl",
    standardbrawl: "Standard Brawl",
  };
  function formatLabel(f: string): string {
    return FORMAT_LABELS[f] ?? f;
  }

  const legendBottom = {
    legend: { position: "bottom" as const, labels: { color: AXIS_COLOR, boxWidth: 14, padding: 10 } },
  };

  function curveConfig(a: DeckAnalysis): ChartConfiguration {
    return {
      type: "bar",
      data: {
        labels: a.mana_curve.map((b) => (b.cmc >= 7 ? "7+" : String(b.cmc))),
        datasets: [{ data: a.mana_curve.map((b) => b.count), backgroundColor: "#3a6df0", borderRadius: 4 }],
      },
      options: {
        responsive: true,
        maintainAspectRatio: false,
        plugins: { legend: { display: false } },
        scales: {
          x: { ticks: { color: AXIS_COLOR }, grid: { display: false } },
          y: { beginAtZero: true, ticks: { color: AXIS_COLOR, precision: 0 }, grid: { color: GRID_COLOR } },
        },
      },
    };
  }

  function doughnut(labels: string[], data: number[], colors: string[]): ChartConfiguration {
    return {
      type: "doughnut",
      data: { labels, datasets: [{ data, backgroundColor: colors, borderColor: "#24242b", borderWidth: 2 }] },
      options: { responsive: true, maintainAspectRatio: false, plugins: legendBottom },
    };
  }

  function colorsConfig(a: DeckAnalysis): ChartConfiguration {
    return doughnut(
      a.color_pips.map((c) => COLOR_NAMES[c.label] ?? c.label),
      a.color_pips.map((c) => c.count),
      a.color_pips.map((c) => COLOR_MAP[c.label] ?? "#888780"),
    );
  }

  function typesConfig(a: DeckAnalysis): ChartConfiguration {
    return doughnut(
      a.type_distribution.map((t) => t.label),
      a.type_distribution.map((t) => t.count),
      a.type_distribution.map((_, i) => TYPE_PALETTE[i % TYPE_PALETTE.length]),
    );
  }

  function rarityConfig(a: DeckAnalysis): ChartConfiguration {
    return doughnut(
      a.rarity_distribution.map((r) => r.label),
      a.rarity_distribution.map((r) => r.count),
      a.rarity_distribution.map((r) => RARITY_MAP[r.label] ?? "#888780"),
    );
  }

  function currentLegality(): FormatLegality | undefined {
    return analysis?.format_legality.find((f) => f.format === selectedFormat);
  }

  // --- Deck editor -------------------------------------------------------

  function emptyDeck(): ParsedDeck {
    return { entries: [], total_cards: 0, unmatched: 0 };
  }

  function newDeck() {
    deck = emptyDeck();
    deckId = null;
    deckName = "";
    deckText = "";
    analysis = null;
    deckError = "";
    deckMsg = "";
  }

  /// Recomputes totals and refreshes the analysis after an edit.
  async function refreshDeck() {
    if (!deck) {
      analysis = null;
      return;
    }
    deck.total_cards = deck.entries.reduce((s, e) => s + e.quantity, 0);
    deck.unmatched = deck.entries.filter((e) => !e.matched).length;
    if (deck.entries.length > 0) {
      analysis = await invoke<DeckAnalysis>("analyze_deck", { deck });
      const formats = analysis.format_legality.map((f) => f.format);
      if (!formats.includes(selectedFormat)) {
        selectedFormat = formats.includes("brawl") ? "brawl" : (formats[0] ?? "");
      }
    } else {
      analysis = null;
    }
  }

  async function addCardToDeck(card: Card, section: DeckSection = "main") {
    if (!deck) deck = emptyDeck();
    const existing = deck.entries.find((e) => e.card?.id === card.id && e.section === section);
    if (existing) {
      existing.quantity += 1;
    } else {
      deck.entries.push({
        quantity: 1,
        name: card.name,
        set_code: card.set_code,
        collector_number: card.collector_number,
        section,
        card,
        matched: true,
      });
    }
    await refreshDeck();
  }

  async function changeQty(entry: DeckEntry, delta: number) {
    if (!deck) return;
    entry.quantity += delta;
    if (entry.quantity <= 0) {
      deck.entries = deck.entries.filter((e) => e !== entry);
    }
    await refreshDeck();
  }

  async function removeEntry(entry: DeckEntry) {
    if (!deck) return;
    deck.entries = deck.entries.filter((e) => e !== entry);
    await refreshDeck();
  }

  async function moveEntry(entry: DeckEntry, section: DeckSection) {
    entry.section = section;
    await refreshDeck();
  }

  async function loadSavedDecks() {
    try {
      savedDecks = await invoke<DeckSummary[]>("list_decks");
    } catch (e) {
      deckError = String(e);
    }
  }

  async function saveDeck() {
    if (!deck || deck.entries.length === 0) {
      deckError = "Nothing to save yet.";
      return;
    }
    if (!deckName.trim()) {
      deckError = "Please enter a deck name.";
      return;
    }
    try {
      deckId = await invoke<number>("save_deck", { id: deckId, name: deckName.trim(), deck });
      deckError = "";
      deckMsg = "Saved!";
      setTimeout(() => (deckMsg = ""), 2000);
      await loadSavedDecks();
    } catch (e) {
      deckError = String(e);
    }
  }

  async function loadSavedDeck(id: number) {
    try {
      const loaded = await invoke<{ id: number; name: string; deck: ParsedDeck }>("load_deck", { id });
      deck = loaded.deck;
      deckId = loaded.id;
      deckName = loaded.name;
      deckError = "";
      copyMsg = "";
      await refreshDeck();
    } catch (e) {
      deckError = String(e);
    }
  }

  async function deleteSavedDeck(id: number) {
    try {
      await invoke("delete_deck", { id });
      if (deckId === id) deckId = null;
      await loadSavedDecks();
    } catch (e) {
      deckError = String(e);
    }
  }

  // Chart configs recomputed only when the analysis changes (not on every
  // format-dropdown change), so the charts don't needlessly redraw.
  const curveCfg = $derived(analysis ? curveConfig(analysis) : null);
  const colorsCfg = $derived(analysis ? colorsConfig(analysis) : null);
  const typesCfg = $derived(analysis ? typesConfig(analysis) : null);
  const rarityCfg = $derived(analysis ? rarityConfig(analysis) : null);

  function mb(bytes: number): string {
    return (bytes / 1048576).toFixed(0);
  }

  function progressLabel(p: Progress): string {
    switch (p.phase) {
      case "index":
        return "Contacting Scryfall…";
      case "download":
        return p.total > 0
          ? `Downloading: ${mb(p.current)} / ${mb(p.total)} MB`
          : `Downloading: ${mb(p.current)} MB`;
      case "parse":
        return `Reading cards: ${p.current.toLocaleString("en-US")} examined`;
      case "save":
        return `Saving ${p.current.toLocaleString("en-US")} Arena cards…`;
      case "done":
        return "Done!";
      default:
        return "Processing…";
    }
  }

  function progressPercent(p: Progress): number | null {
    if (p.phase === "download" && p.total > 0) {
      return Math.round((p.current / p.total) * 100);
    }
    return null;
  }
</script>

<main>
  <h1>MTG Arena Tracker</h1>

  <section class="panel">
    <div class="panel-head">
      <h2>Card database</h2>
      {#if status}
        <span class="badge">{status.card_count.toLocaleString("en-US")} cards</span>
      {/if}
    </div>

    {#if status && status.last_updated}
      <p class="muted">Last updated: {status.last_updated.replace("T", " ").replace("Z", " UTC")}</p>
    {:else}
      <p class="muted">Database empty: download the card data to get started.</p>
    {/if}

    {#if updateInfo && updateInfo.update_available && status && status.card_count > 0}
      <div class="update-banner">
        🆕 {updateInfo.new_cards.toLocaleString("en-US")} new cards available on Arena — update to get them.
      </div>
    {/if}

    <button class="primary" onclick={runUpdate} disabled={updating}>
      {updating
        ? "Updating…"
        : updateInfo && updateInfo.update_available && status && status.card_count > 0
          ? "Update new cards now"
          : "Update card database"}
    </button>

    {#if progress}
      <div class="progress">
        <div class="progress-label">{progressLabel(progress)}</div>
        {#if progressPercent(progress) !== null}
          <div class="bar"><div class="bar-fill" style="width: {progressPercent(progress)}%"></div></div>
        {:else}
          <div class="bar"><div class="bar-fill indeterminate"></div></div>
        {/if}
      </div>
    {/if}

    {#if error}
      <p class="error">⚠️ {error}</p>
    {/if}
  </section>

  <section class="panel">
    <div class="panel-head">
      <h2>Search a card</h2>
    </div>
    <input
      class="search"
      placeholder="Type a card name (min. 2 letters)…"
      bind:value={query}
      oninput={onQueryInput}
      disabled={!status || status.card_count === 0}
    />
    {#if !status || status.card_count === 0}
      <p class="muted">Update the database first to be able to search.</p>
    {:else if searching}
      <p class="muted">Searching…</p>
    {:else if results.length > 0}
      <ul class="results">
        {#each results as card (card.id)}
          <li class="result-row">
            <button class="result" onclick={() => (selected = card)}>
              {#if card.image_small}
                <img src={card.image_small} alt={card.name} loading="lazy" />
              {/if}
              <span class="result-info">
                <span class="result-name">{card.name}</span>
                <span class="result-meta">
                  {card.type_line ?? ""} · {setLabel(card)} · {card.rarity}
                </span>
              </span>
            </button>
            <button class="add-btn" title="Add to deck" aria-label="Add to deck" onclick={() => addCardToDeck(card)}>+</button>
          </li>
        {/each}
      </ul>
    {:else if query.trim().length >= 2}
      <p class="muted">No cards found.</p>
    {/if}
  </section>

  <section class="panel">
    <div class="panel-head">
      <h2>Decks</h2>
      {#if deck}
        <span class="badge">{deck.total_cards} cards</span>
      {/if}
    </div>

    <div class="deck-toolbar">
      <input class="deck-name" placeholder="Deck name…" bind:value={deckName} />
      <button class="primary" onclick={saveDeck} disabled={!deck || deck.entries.length === 0}>
        {deckId ? "Update" : "Save"}
      </button>
      <button class="ghost" onclick={newDeck}>New deck</button>
      {#if deckMsg}<span class="copy-msg">{deckMsg}</span>{/if}
    </div>

    {#if savedDecks.length > 0}
      <div class="saved-decks">
        {#each savedDecks as d (d.id)}
          <div class="saved-deck" class:active={deckId === d.id}>
            <button class="saved-load" onclick={() => loadSavedDeck(d.id)}>{d.name}</button>
            <button class="saved-del" title="Delete" aria-label="Delete deck" onclick={() => deleteSavedDeck(d.id)}>🗑</button>
          </div>
        {/each}
      </div>
    {/if}

    <textarea
      class="deck-input"
      placeholder="Paste an Arena decklist here (Commander / Deck / Sideboard)…"
      bind:value={deckText}
    ></textarea>

    <div class="deck-actions">
      <button class="primary" onclick={importDeck} disabled={importing || !deckText.trim()}>
        {importing ? "Importing…" : "Import deck"}
      </button>
      <label class="file-button">
        Load .txt file
        <input type="file" accept=".txt,text/plain" onchange={onDeckFile} hidden />
      </label>
      {#if deck}
        <button class="ghost" onclick={copyDeck}>Copy to Arena</button>
        {#if copyMsg}<span class="copy-msg">{copyMsg}</span>{/if}
      {/if}
    </div>

    {#if deckError}
      <p class="error">⚠️ {deckError}</p>
    {/if}

    {#if deck}
      {#if deck.unmatched > 0}
        <p class="warn">⚠️ {deck.unmatched} line(s) could not be matched to a card.</p>
      {/if}
      {#if deck.entries.length === 0}
        <p class="muted">Empty deck — search a card above and click <strong>+</strong> to add it.</p>
      {/if}
      {#each sectionOrder as section}
        {#if entriesOf(section).length > 0}
          <div class="deck-section">
            <div class="deck-section-head">
              {sectionLabels[section]} <span class="muted">({sectionCount(section)})</span>
            </div>
            <ul class="deck-list">
              {#each entriesOf(section) as entry (entry.card?.id ?? entry.name)}
                <li class="deck-entry" class:unmatched={!entry.matched}>
                  {#if entry.card?.image_small}
                    <img src={entry.card.image_small} alt={entry.name} loading="lazy" />
                  {:else}
                    <span class="no-img">?</span>
                  {/if}
                  <span class="deck-entry-info">
                    <span class="deck-entry-name">{entry.card?.name ?? entry.name}</span>
                    <span class="deck-entry-meta">
                      {#if entry.matched && entry.card}
                        {entry.card.type_line ?? ""} · {setLabel(entry.card)} · {entry.card.rarity}
                      {:else}
                        Not found in database
                      {/if}
                    </span>
                  </span>
                  <div class="entry-controls">
                    <button class="qty-btn" aria-label="Decrease" onclick={() => changeQty(entry, -1)}>−</button>
                    <span class="qty">{entry.quantity}</span>
                    <button class="qty-btn" aria-label="Increase" onclick={() => changeQty(entry, 1)}>+</button>
                    <select
                      class="section-select"
                      value={entry.section}
                      onchange={(e) => moveEntry(entry, e.currentTarget.value as DeckSection)}
                    >
                      {#each sectionOrder as s}
                        <option value={s}>{sectionLabels[s]}</option>
                      {/each}
                    </select>
                    <button class="remove-btn" aria-label="Remove" onclick={() => removeEntry(entry)}>✕</button>
                  </div>
                </li>
              {/each}
            </ul>
          </div>
        {/if}
      {/each}
    {/if}
  </section>

  {#if analysis}
    <section class="panel">
      <div class="panel-head"><h2>Deck analysis</h2></div>

      <div class="stats-grid">
        <div class="stat"><div class="stat-label">Total</div><div class="stat-value">{analysis.total_cards}</div></div>
        <div class="stat"><div class="stat-label">Lands</div><div class="stat-value">{analysis.lands}</div></div>
        <div class="stat"><div class="stat-label">Non-lands</div><div class="stat-value">{analysis.nonlands}</div></div>
        <div class="stat"><div class="stat-label">Avg. mana value</div><div class="stat-value">{analysis.average_cmc.toFixed(2)}</div></div>
      </div>

      <div class="charts-grid">
        <div class="chart-card wide">
          <div class="chart-title">Mana curve</div>
          <div class="chart-wrap"><canvas use:chartjs={curveCfg!}></canvas></div>
        </div>
        <div class="chart-card">
          <div class="chart-title">Colors</div>
          <div class="chart-wrap"><canvas use:chartjs={colorsCfg!}></canvas></div>
        </div>
        <div class="chart-card">
          <div class="chart-title">Card types</div>
          <div class="chart-wrap"><canvas use:chartjs={typesCfg!}></canvas></div>
        </div>
        <div class="chart-card">
          <div class="chart-title">Rarity</div>
          <div class="chart-wrap"><canvas use:chartjs={rarityCfg!}></canvas></div>
        </div>
      </div>

      {#if analysis.format_legality.length > 0}
        <div class="legality">
          <div class="legality-head">
            <span class="chart-title">Legality</span>
            <select bind:value={selectedFormat}>
              {#each analysis.format_legality as f}
                <option value={f.format}>{formatLabel(f.format)}</option>
              {/each}
            </select>
          </div>
          {#if currentLegality()}
            {#if currentLegality()!.illegal.length === 0}
              <p class="legal-ok">✓ All cards are legal in {formatLabel(selectedFormat)}.</p>
            {:else}
              <p class="legal-bad">✗ {currentLegality()!.illegal.length} card(s) not legal in {formatLabel(selectedFormat)}:</p>
              <ul class="legal-list">
                {#each currentLegality()!.illegal as name}<li>{name}</li>{/each}
              </ul>
            {/if}
          {/if}
        </div>
      {/if}
    </section>
  {/if}

  {#if selected}
    <section class="panel detail">
      <div class="panel-head">
        <h2>{selected.name}</h2>
        <button class="close" onclick={() => (selected = null)}>✕</button>
      </div>
      <div class="detail-body">
        {#if selected.image_normal}
          <img class="detail-img" src={selected.image_normal} alt={selected.name} />
        {/if}
        <dl>
          <dt>Cost</dt><dd>{selected.mana_cost || "—"} (CMC {selected.cmc})</dd>
          <dt>Type</dt><dd>{selected.type_line ?? "—"}</dd>
          <dt>Colors</dt><dd>{selected.colors.length ? selected.colors.join(", ") : "Colorless"}</dd>
          <dt>Rarity</dt><dd>{selected.rarity}</dd>
          <dt>Set</dt><dd>{selected.set_name ? selected.set_name + " " : ""}({selected.set_code.toUpperCase()}) no. {selected.collector_number}</dd>
          <dt>Brawl</dt><dd>{selected.legalities["brawl"] ?? "—"}</dd>
          <dt>Standard</dt><dd>{selected.legalities["standard"] ?? "—"}</dd>
        </dl>
      </div>
    </section>
  {/if}
</main>

<style>
  :global(body) {
    margin: 0;
    background: #1a1a1f;
    color: #e8e8ea;
    font-family: Inter, system-ui, Avenir, Helvetica, Arial, sans-serif;
  }

  main {
    max-width: 720px;
    margin: 0 auto;
    padding: 24px 20px 48px;
  }

  h1 {
    font-size: 24px;
    font-weight: 600;
    margin: 0 0 20px;
  }

  h2 {
    font-size: 16px;
    font-weight: 600;
    margin: 0;
  }

  .panel {
    background: #24242b;
    border: 1px solid #34343d;
    border-radius: 12px;
    padding: 16px 18px;
    margin-bottom: 16px;
  }

  .panel-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 8px;
  }

  .badge {
    background: #2e6f4e;
    color: #d8f5e6;
    font-size: 13px;
    padding: 3px 10px;
    border-radius: 20px;
  }

  .muted {
    color: #9a9aa3;
    font-size: 14px;
    margin: 6px 0;
  }

  .error {
    color: #ff9b9b;
    font-size: 14px;
  }

  .update-banner {
    background: #2b3a5e;
    border: 1px solid #3a6df0;
    color: #cfe0ff;
    font-size: 14px;
    padding: 8px 12px;
    border-radius: 8px;
    margin: 8px 0;
  }

  .warn {
    color: #ffce8a;
    font-size: 14px;
  }

  .deck-input {
    width: 100%;
    box-sizing: border-box;
    min-height: 110px;
    resize: vertical;
    background: #1c1c22;
    border: 1px solid #3a3a45;
    border-radius: 8px;
    padding: 10px 12px;
    color: #e8e8ea;
    font-size: 13px;
    font-family: ui-monospace, Menlo, Consolas, monospace;
  }

  .deck-actions {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-top: 10px;
    flex-wrap: wrap;
  }
  .file-button {
    background: transparent;
    color: #cfd2dc;
    border: 1px solid #3a3a45;
    border-radius: 8px;
    padding: 9px 14px;
    font-size: 14px;
    cursor: pointer;
  }
  .file-button:hover {
    border-color: #3a6df0;
  }
  .ghost {
    background: transparent;
    color: #cfd2dc;
    border: 1px solid #3a3a45;
    border-radius: 8px;
    padding: 9px 14px;
    font-size: 14px;
  }
  .ghost:hover {
    border-color: #2e6f4e;
    color: #d8f5e6;
  }
  .copy-msg {
    color: #7fe0b0;
    font-size: 13px;
  }

  .deck-section {
    margin-top: 16px;
  }
  .deck-section-head {
    font-size: 14px;
    font-weight: 600;
    margin-bottom: 8px;
    border-bottom: 1px solid #34343d;
    padding-bottom: 4px;
  }
  .deck-list {
    list-style: none;
    padding: 0;
    margin: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .deck-entry {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 4px 6px;
    border-radius: 6px;
  }
  .deck-entry.unmatched {
    background: #3a2a2a;
  }
  .deck-entry img,
  .deck-entry .no-img {
    width: 32px;
    height: 45px;
    border-radius: 3px;
    flex-shrink: 0;
    object-fit: cover;
  }
  .deck-entry .no-img {
    display: flex;
    align-items: center;
    justify-content: center;
    background: #2a2a31;
    color: #8a8a93;
    font-size: 14px;
  }
  .deck-entry-info {
    display: flex;
    flex-direction: column;
  }
  .deck-entry-name {
    font-size: 14px;
  }
  .deck-entry-meta {
    font-size: 12px;
    color: #9a9aa3;
  }

  .deck-entry-info {
    flex: 1;
    min-width: 0;
  }

  .result-row {
    display: flex;
    align-items: stretch;
    gap: 6px;
  }
  .result-row .result {
    flex: 1;
  }
  .add-btn {
    flex-shrink: 0;
    width: 38px;
    border-radius: 8px;
    background: #2e6f4e;
    color: #d8f5e6;
    border: none;
    font-size: 18px;
    cursor: pointer;
  }
  .add-btn:hover {
    background: #37855d;
  }

  .deck-toolbar {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-bottom: 10px;
    flex-wrap: wrap;
  }
  .deck-name {
    flex: 1;
    min-width: 160px;
    background: #1c1c22;
    border: 1px solid #3a3a45;
    border-radius: 8px;
    padding: 9px 12px;
    color: #e8e8ea;
    font-size: 14px;
    font-family: inherit;
  }

  .saved-decks {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    margin-bottom: 12px;
  }
  .saved-deck {
    display: flex;
    align-items: center;
    background: #1c1c22;
    border: 1px solid #34343d;
    border-radius: 8px;
    overflow: hidden;
  }
  .saved-deck.active {
    border-color: #3a6df0;
  }
  .saved-load {
    background: none;
    border: none;
    color: #e8e8ea;
    padding: 7px 10px;
    font-size: 13px;
    cursor: pointer;
  }
  .saved-load:hover {
    color: #9fc0ff;
  }
  .saved-del {
    background: none;
    border: none;
    color: #9a9aa3;
    padding: 7px 8px;
    cursor: pointer;
  }
  .saved-del:hover {
    color: #ff9b9b;
  }

  .entry-controls {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-shrink: 0;
  }
  .qty-btn {
    width: 24px;
    height: 24px;
    border-radius: 6px;
    border: 1px solid #3a3a45;
    background: #24242b;
    color: #e8e8ea;
    font-size: 15px;
    line-height: 1;
    cursor: pointer;
  }
  .qty-btn:hover {
    border-color: #3a6df0;
  }
  .qty {
    min-width: 20px;
    text-align: center;
    font-size: 14px;
  }
  .section-select {
    background: #1c1c22;
    color: #cfd2dc;
    border: 1px solid #3a3a45;
    border-radius: 6px;
    padding: 4px 6px;
    font-size: 12px;
    font-family: inherit;
  }
  .remove-btn {
    width: 24px;
    height: 24px;
    border-radius: 6px;
    border: 1px solid #3a3a45;
    background: #24242b;
    color: #9a9aa3;
    cursor: pointer;
  }
  .remove-btn:hover {
    border-color: #c44a37;
    color: #ff9b9b;
  }

  .stats-grid {
    display: grid;
    grid-template-columns: repeat(4, 1fr);
    gap: 10px;
    margin-bottom: 16px;
  }
  .stat {
    background: #1c1c22;
    border-radius: 8px;
    padding: 10px 12px;
  }
  .stat-label {
    font-size: 12px;
    color: #9a9aa3;
  }
  .stat-value {
    font-size: 22px;
    font-weight: 600;
    margin-top: 2px;
  }

  .charts-grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 14px;
  }
  .chart-card {
    background: #1c1c22;
    border: 1px solid #34343d;
    border-radius: 10px;
    padding: 12px;
  }
  .chart-card.wide {
    grid-column: 1 / -1;
  }
  .chart-title {
    font-size: 13px;
    font-weight: 600;
    color: #c9c9d1;
  }
  .chart-wrap {
    position: relative;
    height: 220px;
    margin-top: 8px;
  }

  .legality {
    margin-top: 16px;
  }
  .legality-head {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-bottom: 8px;
  }
  .legality select {
    background: #1c1c22;
    color: #e8e8ea;
    border: 1px solid #3a3a45;
    border-radius: 6px;
    padding: 5px 8px;
    font-size: 13px;
    font-family: inherit;
  }
  .legal-ok {
    color: #7fe0b0;
    font-size: 14px;
  }
  .legal-bad {
    color: #ff9b9b;
    font-size: 14px;
    margin-bottom: 4px;
  }
  .legal-list {
    margin: 0;
    padding-left: 18px;
    font-size: 13px;
    color: #cfd2dc;
  }

  button {
    font-family: inherit;
    cursor: pointer;
  }

  .primary {
    background: #3a6df0;
    color: #fff;
    border: none;
    border-radius: 8px;
    padding: 10px 16px;
    font-size: 14px;
    font-weight: 500;
    margin-top: 6px;
  }
  .primary:disabled {
    background: #3a3a45;
    color: #8a8a93;
    cursor: default;
  }

  .progress {
    margin-top: 14px;
  }
  .progress-label {
    font-size: 13px;
    color: #c9c9d1;
    margin-bottom: 6px;
  }
  .bar {
    height: 8px;
    background: #34343d;
    border-radius: 4px;
    overflow: hidden;
  }
  .bar-fill {
    height: 100%;
    background: #3a6df0;
    transition: width 0.2s ease;
  }
  .bar-fill.indeterminate {
    width: 35%;
    animation: slide 1.1s infinite ease-in-out;
  }
  @keyframes slide {
    0% { margin-left: -35%; }
    100% { margin-left: 100%; }
  }

  .search {
    width: 100%;
    box-sizing: border-box;
    background: #1c1c22;
    border: 1px solid #3a3a45;
    border-radius: 8px;
    padding: 10px 12px;
    color: #e8e8ea;
    font-size: 14px;
    font-family: inherit;
  }
  .search:disabled {
    opacity: 0.5;
  }

  .results {
    list-style: none;
    padding: 0;
    margin: 12px 0 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .result {
    display: flex;
    align-items: center;
    gap: 12px;
    width: 100%;
    text-align: left;
    background: #1c1c22;
    border: 1px solid #34343d;
    border-radius: 8px;
    padding: 8px 10px;
    color: inherit;
  }
  .result:hover {
    border-color: #3a6df0;
  }
  .result img {
    width: 40px;
    border-radius: 4px;
    flex-shrink: 0;
  }
  .result-info {
    display: flex;
    flex-direction: column;
  }
  .result-name {
    font-size: 14px;
    font-weight: 500;
  }
  .result-meta {
    font-size: 12px;
    color: #9a9aa3;
  }

  .detail-body {
    display: flex;
    gap: 16px;
    align-items: flex-start;
  }
  .detail-img {
    width: 220px;
    border-radius: 10px;
    flex-shrink: 0;
  }
  dl {
    margin: 0;
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 4px 12px;
    font-size: 14px;
  }
  dt {
    color: #9a9aa3;
  }
  .close {
    background: none;
    border: none;
    color: #9a9aa3;
    font-size: 16px;
  }
</style>
