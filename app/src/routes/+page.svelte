<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { onMount } from "svelte";
  import type { ChartConfiguration } from "chart.js/auto";
  import { chartjs } from "$lib/chartAction";
  import Markdown from "$lib/Markdown.svelte";
  import { Search, LayoutGrid, Swords, Gem, Settings, Plus, Minus, Trash2, Copy, Upload, X, RefreshCw, ChevronLeft } from "@lucide/svelte";

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

  type DeckSummary = {
    id: number;
    name: string;
    updated_at: string;
    format: string;
    colors: string;
    card_count: number;
    cover_image: string | null;
    wins: number;
    losses: number;
  };
  let deckId = $state<number | null>(null);
  let editorMatches = $state<MatchRecord[]>([]);
  let deckName = $state("");
  let deckFormat = $state("brawl");
  let savedDecks = $state<DeckSummary[]>([]);
  let deckMsg = $state("");

  let view = $state("decks");
  let previewCard = $state<Card | null>(null);
  let previewX = $state(0);
  let previewY = $state(0);

  // Decks gallery filters.
  let deckSearch = $state("");
  let filterFormat = $state("");
  let filterColor = $state("");
  let sortBy = $state("recent");

  const FORMATS = ["standard", "alchemy", "pioneer", "historic", "timeless", "brawl", "standardbrawl"];

  type MatchRecord = {
    match_id: string;
    played_at_ms: number;
    format: string;
    event_id: string;
    opponent: string;
    result: string;
    games_won: number;
    games_lost: number;
    deck_cards: number[];
    deck_name: string;
  };
  let matches = $state<MatchRecord[]>([]);
  let matchesLoading = $state(false);

  async function loadMatches() {
    matchesLoading = true;
    try {
      await invoke("import_match_history");
      matches = await invoke<MatchRecord[]>("list_matches");
    } catch (e) {
      error = String(e);
    } finally {
      matchesLoading = false;
    }
  }

  const matchStats = $derived.by(() => {
    const wins = matches.filter((m) => m.result === "win").length;
    const losses = matches.filter((m) => m.result === "loss").length;
    const decided = wins + losses;
    const winRate = decided > 0 ? Math.round((wins / decided) * 100) : 0;
    return { wins, losses, total: matches.length, winRate };
  });

  function matchDate(ms: number): string {
    if (!ms) return "—";
    return new Date(ms).toLocaleString("en-US", { dateStyle: "medium", timeStyle: "short" });
  }

  type Inventory = {
    wc_common: number;
    wc_uncommon: number;
    wc_rare: number;
    wc_mythic: number;
    gold: number;
    gems: number;
    vault: number;
  };
  let inventory = $state<Inventory | null>(null);

  async function loadInventory() {
    try {
      inventory = await invoke<Inventory | null>("get_inventory");
    } catch {
      inventory = null;
    }
  }

  // AI engine (local llama-server sidecar) — status + first manual test.
  type AiStatus = {
    binary_found: boolean;
    model_found: boolean;
    model_name: string | null;
    running: boolean;
  };
  let aiStatus = $state<AiStatus | null>(null);
  let aiChecking = $state(false);
  let aiPrompt = $state("Ciao! Rispondi in una sola frase.");
  let aiReply = $state("");
  let aiReasoning = $state("");
  let aiThinking = $state(false);
  let aiError = $state("");
  // Whether the deck analysis lets the model reason (deeper but slower). On by
  // default; the user can switch to a fast pass that skips the reasoning.
  let aiDeepThink = $state(true);

  async function loadAiStatus() {
    aiChecking = true;
    aiError = "";
    try {
      aiStatus = await invoke<AiStatus>("ai_status");
    } catch (e) {
      aiError = String(e);
    } finally {
      aiChecking = false;
    }
  }

  // Shared streaming helper: runs an AI command and accumulates the streamed
  // reasoning/answer via the ai-delta/ai-done events.
  async function streamAi(command: string, args: Record<string, unknown>) {
    aiThinking = true;
    aiError = "";
    aiReply = "";
    aiReasoning = "";
    // Register listeners before invoking so no streamed delta is missed.
    const unlistenDelta = await listen<{ kind: string; text: string }>("ai-delta", (e) => {
      if (e.payload.kind === "reasoning") aiReasoning += e.payload.text;
      else aiReply += e.payload.text;
    });
    const unlistenDone = await listen("ai-done", () => {
      aiThinking = false;
      unlistenDelta();
      unlistenDone();
    });
    try {
      await invoke(command, args);
    } catch (e) {
      aiError = String(e);
      aiThinking = false;
      unlistenDelta();
      unlistenDone();
    }
  }

  async function runAiTest() {
    // The engine test is a quick connectivity check, so it skips reasoning.
    await streamAi("ai_chat_stream", { prompt: aiPrompt, think: false });
    loadAiStatus();
  }

  async function analyzeDeckWithAI() {
    if (!deck) return;
    await streamAi("ai_analyze_deck", { deck, format: deckFormat, think: aiDeepThink });
  }

  const BASIC_LANDS = ["plains", "island", "swamp", "mountain", "forest", "wastes"];

  // Wildcards needed to build the current deck from scratch, by rarity.
  const craftCost = $derived.by(() => {
    const c = { common: 0, uncommon: 0, rare: 0, mythic: 0 };
    if (deck) {
      for (const e of deck.entries) {
        const card = e.card;
        if (!card || BASIC_LANDS.includes(card.name.toLowerCase())) continue;
        if (card.rarity in c) c[card.rarity as keyof typeof c] += e.quantity;
      }
    }
    return c;
  });

  // Cards view — advanced search filters.
  let cardQuery = $state("");
  let cardColors = $state<string[]>([]);
  let cardTypes = $state<string[]>([]);
  let cardRarities = $state<string[]>([]);
  let cardFormat = $state("");
  let mvMin = $state("");
  let mvMax = $state("");
  let cardResults = $state<Card[]>([]);
  let cardSearching = $state(false);
  let cardTimer: ReturnType<typeof setTimeout> | undefined;

  const COLOR_FILTER = ["W", "U", "B", "R", "G"];
  const TYPE_FILTER = ["Creature", "Instant", "Sorcery", "Artifact", "Enchantment", "Planeswalker", "Land"];
  const RARITY_FILTER = ["common", "uncommon", "rare", "mythic"];

  function toggle(arr: string[], v: string): string[] {
    return arr.includes(v) ? arr.filter((x) => x !== v) : [...arr, v];
  }

  function anyCardFilter(): boolean {
    return (
      cardQuery.trim().length > 0 ||
      cardColors.length > 0 ||
      cardTypes.length > 0 ||
      cardRarities.length > 0 ||
      cardFormat !== "" ||
      mvMin !== "" ||
      mvMax !== ""
    );
  }

  function scheduleCardSearch() {
    clearTimeout(cardTimer);
    cardTimer = setTimeout(runCardSearch, 200);
  }

  async function runCardSearch() {
    if (!anyCardFilter()) {
      cardResults = [];
      return;
    }
    cardSearching = true;
    try {
      cardResults = await invoke<Card[]>("search_cards_advanced", {
        filters: {
          query: cardQuery,
          colors: cardColors,
          types: cardTypes,
          rarities: cardRarities,
          format: cardFormat || null,
          mv_min: mvMin === "" ? null : Number(mvMin),
          mv_max: mvMax === "" ? null : Number(mvMax),
          limit: 60,
        },
      });
    } catch (e) {
      error = String(e);
    } finally {
      cardSearching = false;
    }
  }

  const sectionOrder: DeckSection[] = ["commander", "companion", "main", "sideboard"];
  const sectionLabels: Record<DeckSection, string> = {
    commander: "Commander",
    companion: "Companion",
    main: "Deck",
    sideboard: "Sideboard",
  };

  const NAV = [
    { id: "cards", label: "Cards", icon: Search },
    { id: "decks", label: "Decks", icon: LayoutGrid },
    { id: "matches", label: "Matches", icon: Swords },
    { id: "collection", label: "Collection", icon: Gem },
    { id: "settings", label: "Settings", icon: Settings },
  ];

  onMount(async () => {
    await loadStatus();
    await listen<Progress>("db-progress", (event) => {
      progress = event.payload;
    });
    try {
      await invoke("ensure_set_names");
    } catch {
      // Offline: set names will simply be missing until next time.
    }
    await checkUpdates();
    await loadSavedDecks();
    await listen("matches-updated", async () => {
      matches = await invoke<MatchRecord[]>("list_matches");
      await loadSavedDecks();
      await loadDeckMatches();
    });
    await loadMatches();
    await loadInventory();
  });

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
      results = await invoke<Card[]>("search_cards", { query, limit: 36 });
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
      copyMsg = "Copied!";
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
  const COLOR_MAP: Record<string, string> = { W: "#e9e3c8", U: "#2a6fb0", B: "#7a6a86", R: "#c44a37", G: "#3f8f54" };
  const COLOR_NAMES: Record<string, string> = { W: "White", U: "Blue", B: "Black", R: "Red", G: "Green" };
  const RARITY_MAP: Record<string, string> = { common: "#8a8a93", uncommon: "#9bb7c4", rare: "#d6b24a", mythic: "#e0682a" };
  const TYPE_PALETTE = ["#4b82f0", "#1d9e75", "#d85a30", "#9a7bd0", "#d6b24a", "#5dcaa5", "#c44a37", "#888780", "#b4b2a9"];
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
    legend: { position: "bottom" as const, labels: { color: AXIS_COLOR, boxWidth: 12, padding: 8, font: { size: 11 } } },
  };

  function curveConfig(a: DeckAnalysis): ChartConfiguration {
    return {
      type: "bar",
      data: {
        labels: a.mana_curve.map((b) => (b.cmc >= 7 ? "7+" : String(b.cmc))),
        datasets: [{ data: a.mana_curve.map((b) => b.count), backgroundColor: "#4b82f0", borderRadius: 4 }],
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
      data: { labels, datasets: [{ data, backgroundColor: colors, borderColor: "#1c1c23", borderWidth: 2 }] },
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
    deckFormat = "brawl";
    deckText = "";
    analysis = null;
    deckError = "";
    deckMsg = "";
    editorMatches = [];
    view = "editor";
  }

  async function openDeck(d: DeckSummary) {
    await loadSavedDeck(d.id);
    deckFormat = d.format || "brawl";
    view = "editor";
    await loadDeckMatches();
  }

  async function loadDeckMatches() {
    if (!deckId) {
      editorMatches = [];
      return;
    }
    try {
      editorMatches = await invoke<MatchRecord[]>("deck_matches", { deckId });
    } catch {
      editorMatches = [];
    }
  }

  const editorStats = $derived.by(() => {
    const wins = editorMatches.filter((m) => m.result === "win").length;
    const losses = editorMatches.filter((m) => m.result === "loss").length;
    const decided = wins + losses;
    return { wins, losses, winRate: decided > 0 ? Math.round((wins / decided) * 100) : 0 };
  });

  function backToGallery() {
    view = "decks";
    loadSavedDecks();
  }

  // Gallery: filtered + sorted view of the saved decks.
  const filteredDecks = $derived.by(() => {
    let list = savedDecks;
    const q = deckSearch.trim().toLowerCase();
    if (q) list = list.filter((d) => d.name.toLowerCase().includes(q));
    if (filterFormat) list = list.filter((d) => d.format === filterFormat);
    if (filterColor === "C") list = list.filter((d) => d.colors === "");
    else if (filterColor === "M") list = list.filter((d) => d.colors.length > 1);
    else if (filterColor) list = list.filter((d) => d.colors.includes(filterColor));
    list = [...list];
    if (sortBy === "name") list.sort((a, b) => a.name.localeCompare(b.name));
    else if (sortBy === "count") list.sort((a, b) => b.card_count - a.card_count);
    return list;
  });

  function previewStyle(): string {
    const w = 224;
    const left = Math.min(previewX + 18, window.innerWidth - w - 12);
    const top = Math.min(Math.max(previewY - 150, 10), window.innerHeight - 320);
    return `left:${left}px; top:${top}px`;
  }

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
      deckId = await invoke<number>("save_deck", { id: deckId, name: deckName.trim(), format: deckFormat, deck });
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
        return p.total > 0 ? `Downloading: ${mb(p.current)} / ${mb(p.total)} MB` : `Downloading: ${mb(p.current)} MB`;
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

<div class="flex h-screen overflow-hidden">
  <aside class="w-[182px] shrink-0 bg-surface border-r border-border flex flex-col px-3 py-4 gap-1">
    <div class="flex items-center gap-2.5 px-2 pb-4">
      <div class="size-7 rounded-lg bg-accent-soft grid place-items-center text-accent"><LayoutGrid size={16} /></div>
      <span class="text-sm font-medium">MTG Tracker</span>
    </div>
    {#each NAV as item}
      {@const Icon = item.icon}
      <button
        onclick={() => (view = item.id)}
        class="flex items-center gap-3 px-3 py-2 rounded-md text-sm text-left transition-colors {view === item.id
          ? 'bg-accent-soft text-accent'
          : 'text-muted hover:bg-surface-2 hover:text-text'}"
      >
        <Icon size={18} />
        {item.label}
      </button>
    {/each}
    <div class="mt-auto px-2 pt-3 text-xs text-faint">
      {#if status}{status.card_count.toLocaleString("en-US")} cards{/if}
    </div>
  </aside>

  <main class="flex-1 min-w-0 overflow-y-auto">
    {#if view === "cards"}
      <div class="p-6 max-w-5xl mx-auto">
        <h1 class="text-xl font-medium">Cards</h1>
        <p class="text-sm text-muted mb-4">Search and filter the {status?.card_count.toLocaleString("en-US") ?? "…"} Arena cards.</p>

        <input
          class="w-full bg-surface border border-border rounded-lg px-4 py-2.5 text-sm outline-none focus:border-accent"
          placeholder="Search by name…"
          bind:value={cardQuery}
          oninput={scheduleCardSearch}
          disabled={!status || status.card_count === 0}
        />

        <div class="flex flex-wrap items-center gap-2 mt-3">
          <div class="flex gap-1.5">
            {#each COLOR_FILTER as c}
              <button
                onclick={() => { cardColors = toggle(cardColors, c); runCardSearch(); }}
                title={COLOR_NAMES[c]}
                aria-label={COLOR_NAMES[c]}
                class="size-7 rounded-full border-2 transition {cardColors.includes(c) ? 'border-text' : 'border-transparent opacity-70 hover:opacity-100'}"
                style="background:{COLOR_MAP[c]}"
              ></button>
            {/each}
          </div>
          <select bind:value={cardFormat} onchange={runCardSearch} class="bg-surface border border-border rounded-md px-2 py-1.5 text-sm">
            <option value="">Any format</option>
            {#each FORMATS as f}<option value={f}>{formatLabel(f)}</option>{/each}
          </select>
          <input type="number" min="0" bind:value={mvMin} oninput={scheduleCardSearch} placeholder="MV min" class="w-24 bg-surface border border-border rounded-md px-2 py-1.5 text-sm" />
          <input type="number" min="0" bind:value={mvMax} oninput={scheduleCardSearch} placeholder="MV max" class="w-24 bg-surface border border-border rounded-md px-2 py-1.5 text-sm" />
        </div>

        <div class="flex flex-wrap gap-1.5 mt-2">
          {#each TYPE_FILTER as t}
            <button onclick={() => { cardTypes = toggle(cardTypes, t); runCardSearch(); }} class="px-2 py-1 rounded text-xs border transition {cardTypes.includes(t) ? 'border-accent text-accent' : 'border-border text-muted hover:text-text'}">{t}</button>
          {/each}
          <span class="w-px bg-border mx-1"></span>
          {#each RARITY_FILTER as r}
            <button onclick={() => { cardRarities = toggle(cardRarities, r); runCardSearch(); }} class="px-2 py-1 rounded text-xs border transition capitalize {cardRarities.includes(r) ? 'border-accent text-accent' : 'border-border text-muted hover:text-text'}">{r}</button>
          {/each}
        </div>

        {#if !status || status.card_count === 0}
          <p class="text-sm text-muted mt-4">Update the card database first (Settings).</p>
        {:else if cardSearching}
          <p class="text-sm text-muted mt-4">Searching…</p>
        {:else if cardResults.length > 0}
          <div class="grid grid-cols-[repeat(auto-fill,minmax(140px,1fr))] gap-3 mt-4">
            {#each cardResults as card (card.id)}
              <div
                class="group relative rounded-lg overflow-hidden border border-border bg-surface hover:border-accent transition-colors"
                role="listitem"
                onmouseenter={() => (previewCard = card)}
                onmouseleave={() => (previewCard = null)}
                onmousemove={(e) => { previewX = e.clientX; previewY = e.clientY; }}
              >
                <button class="block w-full" onclick={() => (selected = card)} aria-label={card.name}>
                  {#if card.image_normal}
                    <img src={card.image_normal} alt={card.name} loading="lazy" class="w-full aspect-[5/7] object-cover" />
                  {:else}
                    <div class="w-full aspect-[5/7] grid place-items-center text-faint text-xs">No image</div>
                  {/if}
                </button>
                <div class="p-2">
                  <div class="text-xs font-medium truncate">{card.name}</div>
                  <div class="text-[11px] text-muted truncate">{card.set_code.toUpperCase()} {card.collector_number}</div>
                </div>
                <button
                  onclick={() => addCardToDeck(card)}
                  title="Add to deck"
                  aria-label="Add to deck"
                  class="absolute top-2 right-2 size-7 rounded-md bg-success text-white grid place-items-center opacity-0 group-hover:opacity-100 transition hover:brightness-110"
                >
                  <Plus size={16} />
                </button>
              </div>
            {/each}
          </div>
        {:else if anyCardFilter()}
          <p class="text-sm text-muted mt-4">No cards match the filters.</p>
        {:else}
          <p class="text-sm text-muted mt-4">Type a name or use the filters above to browse cards.</p>
        {/if}
      </div>
    {:else if view === "decks"}
      <div class="p-6 max-w-5xl mx-auto">
        <div class="flex items-center justify-between mb-4">
          <div>
            <h1 class="text-xl font-medium">Decks</h1>
            <p class="text-sm text-muted">{savedDecks.length} saved deck{savedDecks.length === 1 ? "" : "s"}</p>
          </div>
          <button onclick={newDeck} class="inline-flex items-center gap-2 rounded-md bg-accent px-3 py-2 text-sm font-medium text-white hover:opacity-90">
            <Plus size={16} /> New deck
          </button>
        </div>

        <div class="flex flex-wrap items-center gap-2 mb-4">
          <input bind:value={deckSearch} placeholder="Search decks…" class="flex-1 min-w-[160px] bg-surface border border-border rounded-md px-3 py-2 text-sm outline-none focus:border-accent" />
          <select bind:value={filterFormat} class="bg-surface border border-border rounded-md px-2 py-2 text-sm">
            <option value="">All formats</option>
            {#each FORMATS as f}<option value={f}>{formatLabel(f)}</option>{/each}
          </select>
          <select bind:value={filterColor} class="bg-surface border border-border rounded-md px-2 py-2 text-sm">
            <option value="">All colors</option>
            <option value="W">White</option>
            <option value="U">Blue</option>
            <option value="B">Black</option>
            <option value="R">Red</option>
            <option value="G">Green</option>
            <option value="M">Multicolor</option>
            <option value="C">Colorless</option>
          </select>
          <select bind:value={sortBy} class="bg-surface border border-border rounded-md px-2 py-2 text-sm">
            <option value="recent">Recent</option>
            <option value="name">Name</option>
            <option value="count">Card count</option>
          </select>
        </div>

        {#if savedDecks.length === 0}
          <div class="rounded-lg border border-dashed border-border p-12 text-center text-sm text-muted">
            No saved decks yet — click <span class="text-accent">New deck</span> to build one, or import from Arena.
          </div>
        {:else if filteredDecks.length === 0}
          <p class="text-sm text-muted">No decks match the filters.</p>
        {:else}
          <div class="grid grid-cols-[repeat(auto-fill,minmax(190px,1fr))] gap-3">
            {#each filteredDecks as d (d.id)}
              <div class="group relative rounded-lg overflow-hidden border border-border bg-surface hover:border-accent transition-colors">
                <button onclick={() => openDeck(d)} class="block w-full text-left">
                  <div class="h-24 bg-surface-2 overflow-hidden">
                    {#if d.cover_image}
                      <img src={d.cover_image} alt="" class="w-full h-full object-cover object-[center_22%] opacity-90 group-hover:opacity-100 transition-opacity" />
                    {/if}
                  </div>
                  <div class="p-3">
                    <div class="text-sm font-medium truncate pr-5">{d.name}</div>
                    <div class="flex items-center gap-2 mt-2">
                      {#if d.format}<span class="text-[10px] px-2 py-0.5 rounded bg-accent-soft text-accent">{formatLabel(d.format)}</span>{/if}
                      {#if d.colors}<span class="flex gap-0.5">{#each d.colors.split("") as c}<span class="size-2.5 rounded-full" style="background:{COLOR_MAP[c]}"></span>{/each}</span>{/if}
                      <span class="text-[11px] text-muted ml-auto">{d.card_count} cards</span>
                    </div>
                    {#if d.wins + d.losses > 0}
                      <div class="mt-1.5 text-[11px]">
                        <span class={d.wins >= d.losses ? "text-success" : "text-danger"}>{d.wins}W–{d.losses}L</span>
                        <span class="text-muted"> · {Math.round((d.wins / (d.wins + d.losses)) * 100)}% win rate</span>
                      </div>
                    {/if}
                  </div>
                </button>
                <button onclick={() => deleteSavedDeck(d.id)} title="Delete" aria-label="Delete deck" class="absolute top-2 right-2 size-7 rounded-md bg-black/40 text-white grid place-items-center opacity-0 group-hover:opacity-100 transition hover:bg-danger"><Trash2 size={14} /></button>
              </div>
            {/each}
          </div>
        {/if}
      </div>
    {:else if view === "editor"}
      <div class="p-6 max-w-5xl mx-auto">
        <div class="flex items-center gap-3 mb-4">
          <button onclick={backToGallery} aria-label="Back to decks" class="size-8 rounded-md border border-border grid place-items-center text-muted hover:border-accent hover:text-text"><ChevronLeft size={18} /></button>
          <div>
            <h1 class="text-xl font-medium">{deckId ? "Edit deck" : "New deck"}</h1>
            <p class="text-sm text-muted">{#if deck}{deck.total_cards} cards{:else}Build or import a deck{/if}</p>
          </div>
        </div>

        <div class="flex flex-wrap items-center gap-2 mb-3">
          <input class="flex-1 min-w-[160px] bg-surface border border-border rounded-md px-3 py-2 text-sm outline-none focus:border-accent" placeholder="Deck name…" bind:value={deckName} />
          <select bind:value={deckFormat} class="bg-surface border border-border rounded-md px-2 py-2 text-sm">
            {#each FORMATS as f}<option value={f}>{formatLabel(f)}</option>{/each}
          </select>
          <button onclick={saveDeck} disabled={!deck || deck.entries.length === 0} class="inline-flex items-center gap-2 rounded-md bg-accent px-3 py-2 text-sm font-medium text-white hover:opacity-90 disabled:opacity-40">
            {deckId ? "Update" : "Save"}
          </button>
          {#if deck}
            <button onclick={copyDeck} class="inline-flex items-center gap-2 rounded-md border border-border px-3 py-2 text-sm text-muted hover:border-accent hover:text-text"><Copy size={15} /> Copy to Arena</button>
          {/if}
          {#if deckMsg}<span class="text-sm text-success">{deckMsg}</span>{/if}
          {#if copyMsg}<span class="text-sm text-success">{copyMsg}</span>{/if}
        </div>

        <details class="mb-4 rounded-md border border-border bg-surface">
          <summary class="cursor-pointer select-none px-3 py-2 text-sm text-muted">Import from Arena (paste or .txt file)</summary>
          <div class="p-3 pt-0">
            <textarea class="w-full min-h-[90px] bg-surface-2 border border-border rounded-md p-2.5 text-[13px] font-mono outline-none focus:border-accent" placeholder="Paste an Arena decklist…" bind:value={deckText}></textarea>
            <div class="flex gap-2 mt-2">
              <button onclick={importDeck} disabled={importing || !deckText.trim()} class="inline-flex items-center gap-2 rounded-md bg-accent px-3 py-2 text-sm font-medium text-white hover:opacity-90 disabled:opacity-40">{importing ? "Importing…" : "Import"}</button>
              <label class="inline-flex items-center gap-2 rounded-md border border-border px-3 py-2 text-sm text-muted hover:border-accent cursor-pointer"><Upload size={15} /> Load .txt<input type="file" accept=".txt,text/plain" onchange={onDeckFile} hidden /></label>
            </div>
          </div>
        </details>

        <div class="mb-4">
          <input
            bind:value={query}
            oninput={onQueryInput}
            placeholder="Add a card — search by name…"
            class="w-full bg-surface border border-border rounded-md px-3 py-2 text-sm outline-none focus:border-accent"
          />
          {#if query.trim().length >= 2 && results.length > 0}
            <div class="mt-2 max-h-64 overflow-y-auto rounded-md border border-border bg-surface divide-y divide-border">
              {#each results as card (card.id)}
                <div
                  class="flex items-center gap-2.5 px-2 py-1.5 hover:bg-surface-2"
                  role="listitem"
                  onmouseenter={() => (previewCard = card)}
                  onmouseleave={() => (previewCard = null)}
                  onmousemove={(e) => { previewX = e.clientX; previewY = e.clientY; }}
                >
                  {#if card.image_small}<img src={card.image_small} alt="" class="w-7 rounded" />{/if}
                  <div class="flex-1 min-w-0">
                    <div class="text-sm truncate">{card.name}</div>
                    <div class="text-[11px] text-muted truncate">{card.type_line ?? ""} · {card.rarity}</div>
                  </div>
                  <button onclick={() => addCardToDeck(card)} aria-label="Add to deck" class="size-6 rounded bg-success text-white grid place-items-center hover:brightness-110"><Plus size={14} /></button>
                </div>
              {/each}
            </div>
          {/if}
        </div>

        {#if deckError}<p class="text-sm text-danger mb-3">⚠️ {deckError}</p>{/if}

        {#if deck}
          <div class="grid md:grid-cols-[1.1fr_0.9fr] gap-5">
            <div>
              {#if deck.unmatched > 0}<p class="text-sm text-warning mb-2">⚠️ {deck.unmatched} line(s) not matched.</p>{/if}
              {#if deck.entries.length === 0}
                <div class="rounded-lg border border-dashed border-border p-8 text-center text-sm text-muted">Empty deck — search cards and click <span class="text-success">+</span> to add them.</div>
              {/if}
              {#each sectionOrder as section}
                {#if entriesOf(section).length > 0}
                  <div class="mb-4">
                    <div class="text-xs font-medium uppercase tracking-wide text-faint mb-1.5 border-b border-border pb-1">
                      {sectionLabels[section]} <span class="text-muted normal-case">({sectionCount(section)})</span>
                    </div>
                    <div class="flex flex-col">
                      {#each entriesOf(section) as entry (entry.card?.id ?? entry.name)}
                        <div
                          class="flex items-center gap-2.5 px-1.5 py-1 rounded-md hover:bg-surface-2"
                          role="listitem"
                          onmouseenter={() => (previewCard = entry.card)}
                          onmouseleave={() => (previewCard = null)}
                          onmousemove={(e) => { previewX = e.clientX; previewY = e.clientY; }}
                        >
                          {#if entry.card?.image_small}
                            <img src={entry.card.image_small} alt="" class="w-7 rounded" />
                          {:else}
                            <div class="w-7 h-[39px] rounded bg-surface-2 grid place-items-center text-faint text-xs">?</div>
                          {/if}
                          <div class="flex-1 min-w-0">
                            <div class="text-sm truncate">{entry.card?.name ?? entry.name}</div>
                            <div class="text-[11px] text-muted truncate">
                              {#if entry.matched && entry.card}{entry.card.type_line ?? ""} · {entry.card.rarity}{:else}Not found{/if}
                            </div>
                          </div>
                          <div class="flex items-center gap-1.5 shrink-0">
                            <button onclick={() => changeQty(entry, -1)} aria-label="Decrease" class="size-6 rounded border border-border grid place-items-center hover:border-accent"><Minus size={13} /></button>
                            <span class="w-5 text-center text-sm">{entry.quantity}</span>
                            <button onclick={() => changeQty(entry, 1)} aria-label="Increase" class="size-6 rounded border border-border grid place-items-center hover:border-accent"><Plus size={13} /></button>
                            <select value={entry.section} onchange={(e) => moveEntry(entry, e.currentTarget.value as DeckSection)} class="bg-surface-2 border border-border rounded px-1 py-0.5 text-xs">
                              {#each sectionOrder as s}<option value={s}>{sectionLabels[s]}</option>{/each}
                            </select>
                            <button onclick={() => removeEntry(entry)} aria-label="Remove" class="size-6 rounded border border-border grid place-items-center text-muted hover:text-danger hover:border-danger"><X size={13} /></button>
                          </div>
                        </div>
                      {/each}
                    </div>
                  </div>
                {/if}
              {/each}
            </div>

            {#if analysis}
              <div class="flex flex-col gap-3">
                {#if deckId}
                  <div class="rounded-lg border border-border bg-surface p-3">
                    <div class="text-xs font-medium text-muted mb-2">This deck's matches</div>
                    {#if editorMatches.length > 0}
                      <div class="flex gap-5 mb-2.5">
                        <div><div class="text-[11px] text-muted">Record</div><div class="text-lg font-medium">{editorStats.wins}–{editorStats.losses}</div></div>
                        <div><div class="text-[11px] text-muted">Win rate</div><div class="text-lg font-medium {editorStats.winRate >= 50 ? 'text-success' : 'text-danger'}">{editorStats.winRate}%</div></div>
                      </div>
                      <div class="flex flex-col gap-1">
                        {#each editorMatches.slice(0, 8) as m (m.match_id)}
                          <div class="flex items-center gap-2 text-xs">
                            <span class="w-4 font-medium {m.result === 'win' ? 'text-success' : m.result === 'loss' ? 'text-danger' : 'text-muted'}">{m.result === "win" ? "W" : m.result === "loss" ? "L" : "D"}</span>
                            <span class="flex-1 truncate text-muted">vs {m.opponent}</span>
                            <span class="text-faint">{m.games_won}–{m.games_lost}</span>
                          </div>
                        {/each}
                      </div>
                    {:else}
                      <p class="text-xs text-muted">No tracked matches for this deck yet. Play a game with it!</p>
                    {/if}
                  </div>
                {/if}
                <div class="rounded-lg border border-border bg-surface p-3">
                  <div class="text-xs font-medium text-muted mb-2">Craft cost (from scratch)</div>
                  <div class="grid grid-cols-[1fr_auto] gap-x-5 gap-y-1 text-sm">
                    <span class="text-muted">Common</span><span class="text-right font-medium">{craftCost.common}</span>
                    <span class="text-muted">Uncommon</span><span class="text-right font-medium">{craftCost.uncommon}</span>
                    <span class="text-muted">Rare</span><span class="text-right font-medium" style="color:#d6b24a">{craftCost.rare}</span>
                    <span class="text-muted">Mythic</span><span class="text-right font-medium" style="color:#e0682a">{craftCost.mythic}</span>
                  </div>
                  <p class="text-[11px] text-faint mt-2">Wildcards to craft this deck if you owned none of its cards. Arena no longer logs your collection, so we can't show only the cards you're missing.</p>
                </div>
                <div class="grid grid-cols-2 gap-2">
                  <div class="bg-surface border border-border rounded-lg px-3 py-2"><div class="text-[11px] text-muted">Cards</div><div class="text-lg font-medium">{analysis.total_cards}</div></div>
                  <div class="bg-surface border border-border rounded-lg px-3 py-2"><div class="text-[11px] text-muted">Lands</div><div class="text-lg font-medium">{analysis.lands}</div></div>
                  <div class="bg-surface border border-border rounded-lg px-3 py-2"><div class="text-[11px] text-muted">Non-lands</div><div class="text-lg font-medium">{analysis.nonlands}</div></div>
                  <div class="bg-surface border border-border rounded-lg px-3 py-2"><div class="text-[11px] text-muted">Avg MV</div><div class="text-lg font-medium">{analysis.average_cmc.toFixed(2)}</div></div>
                </div>
                <div class="rounded-lg border border-border bg-surface p-3">
                  <div class="text-xs font-medium text-muted mb-2">Mana curve</div>
                  <div class="relative h-[170px]"><canvas use:chartjs={curveCfg!}></canvas></div>
                </div>
                <div class="grid grid-cols-2 gap-3">
                  <div class="rounded-lg border border-border bg-surface p-3"><div class="text-xs font-medium text-muted mb-2">Colors</div><div class="relative h-[150px]"><canvas use:chartjs={colorsCfg!}></canvas></div></div>
                  <div class="rounded-lg border border-border bg-surface p-3"><div class="text-xs font-medium text-muted mb-2">Types</div><div class="relative h-[150px]"><canvas use:chartjs={typesCfg!}></canvas></div></div>
                </div>
                <div class="rounded-lg border border-border bg-surface p-3"><div class="text-xs font-medium text-muted mb-2">Rarity</div><div class="relative h-[150px]"><canvas use:chartjs={rarityCfg!}></canvas></div></div>
                {#if analysis.format_legality.length > 0}
                  <div class="rounded-lg border border-border bg-surface p-3">
                    <div class="flex items-center justify-between mb-2">
                      <span class="text-xs font-medium text-muted">Legality</span>
                      <select bind:value={selectedFormat} class="bg-surface-2 border border-border rounded px-2 py-1 text-xs">
                        {#each analysis.format_legality as f}<option value={f.format}>{formatLabel(f.format)}</option>{/each}
                      </select>
                    </div>
                    {#if currentLegality()}
                      {#if currentLegality()!.illegal.length === 0}
                        <p class="text-sm text-success">✓ Legal in {formatLabel(selectedFormat)}</p>
                      {:else}
                        <p class="text-sm text-danger mb-1">✗ {currentLegality()!.illegal.length} not legal in {formatLabel(selectedFormat)}:</p>
                        <ul class="text-xs text-muted list-disc pl-4 space-y-0.5">{#each currentLegality()!.illegal as name}<li>{name}</li>{/each}</ul>
                      {/if}
                    {/if}
                  </div>
                {/if}
              </div>

              <div class="rounded-lg border border-border bg-surface p-3">
                <div class="flex items-center justify-between mb-2">
                  <span class="text-xs font-medium text-muted">AI coach</span>
                  <div class="flex items-center gap-2">
                    <select bind:value={aiDeepThink} disabled={aiThinking} title="In-depth lets the model reason (deeper, slower); Fast skips it (quicker)" class="bg-surface-2 border border-border rounded-md px-2 py-1.5 text-xs">
                      <option value={true}>🧠 In-depth</option>
                      <option value={false}>⚡ Fast</option>
                    </select>
                    <button onclick={analyzeDeckWithAI} disabled={aiThinking} class="inline-flex items-center gap-2 rounded-md bg-accent px-3 py-1.5 text-sm font-medium text-white hover:opacity-90 disabled:opacity-50">
                      {aiThinking ? "Analyzing…" : "Analyze with AI"}
                    </button>
                  </div>
                </div>
                {#if aiReasoning}
                  <details class="text-xs text-muted mb-2"><summary class="cursor-pointer select-none">💭 Reasoning {aiThinking ? "(thinking…)" : ""}</summary><div class="mt-1 whitespace-pre-wrap rounded-md border border-border bg-surface-2 px-3 py-2">{aiReasoning}</div></details>
                {/if}
                {#if aiReply}
                  <div class="rounded-md border border-border bg-surface-2 px-3 py-2 text-sm"><Markdown source={aiReply} /></div>
                {:else if aiThinking}
                  <p class="text-sm text-muted">Starting the engine and analyzing…</p>
                {/if}
                {#if aiError}<p class="text-sm text-danger mt-2">⚠️ {aiError}</p>{/if}
              </div>
            {/if}
          </div>
        {/if}
      </div>
    {:else if view === "settings"}
      <div class="p-6 max-w-3xl mx-auto">
        <h1 class="text-xl font-medium mb-4">Settings</h1>
        <div class="rounded-lg border border-border bg-surface p-4">
          <div class="flex items-center justify-between mb-1">
            <h2 class="text-base font-medium">Card database</h2>
            {#if status}<span class="text-xs px-2.5 py-1 rounded-md bg-accent-soft text-accent">{status.card_count.toLocaleString("en-US")} cards</span>{/if}
          </div>
          {#if status?.last_updated}
            <p class="text-sm text-muted">Last updated: {status.last_updated.replace("T", " ").replace("Z", " UTC")}</p>
          {:else}
            <p class="text-sm text-muted">Database empty — download the card data to begin.</p>
          {/if}
          {#if updateInfo?.update_available && status && status.card_count > 0}
            <div class="mt-3 rounded-md border border-accent bg-accent-soft px-3 py-2 text-sm text-accent">🆕 {updateInfo.new_cards.toLocaleString("en-US")} new cards available on Arena.</div>
          {/if}
          <button onclick={runUpdate} disabled={updating} class="mt-3 inline-flex items-center gap-2 rounded-md bg-accent px-4 py-2 text-sm font-medium text-white hover:opacity-90 disabled:opacity-50">
            <RefreshCw size={15} class={updating ? "animate-spin" : ""} />
            {updating ? "Updating…" : updateInfo?.update_available ? "Update now" : "Update card database"}
          </button>
          {#if progress}
            <div class="mt-3">
              <div class="text-xs text-muted mb-1">{progressLabel(progress)}</div>
              <div class="h-2 rounded bg-surface-2 overflow-hidden">
                {#if progressPercent(progress) !== null}
                  <div class="h-full bg-accent transition-all" style="width:{progressPercent(progress)}%"></div>
                {:else}
                  <div class="h-full w-1/3 bg-accent animate-pulse"></div>
                {/if}
              </div>
            </div>
          {/if}
          {#if error}<p class="text-sm text-danger mt-2">⚠️ {error}</p>{/if}
        </div>

        <div class="rounded-lg border border-border bg-surface p-4 mt-4">
          <div class="flex items-center justify-between mb-1">
            <h2 class="text-base font-medium">AI engine (local)</h2>
            <button onclick={loadAiStatus} disabled={aiChecking} class="inline-flex items-center gap-2 rounded-md border border-border px-3 py-1.5 text-sm text-muted hover:border-accent hover:text-text disabled:opacity-40">
              <RefreshCw size={14} class={aiChecking ? "animate-spin" : ""} /> Check
            </button>
          </div>
          {#if aiStatus}
            <ul class="text-sm text-muted mt-2 space-y-1">
              <li>{aiStatus.binary_found ? "✅" : "❌"} llama-server {aiStatus.binary_found ? "found" : "not found"}</li>
              <li>{aiStatus.model_found ? "✅" : "❌"} Model {aiStatus.model_name ? `(${aiStatus.model_name})` : "not found"}</li>
              <li>{aiStatus.running ? "🟢 Engine running" : "⚪ Engine stopped"}</li>
            </ul>
          {:else}
            <p class="text-sm text-muted mt-2">Click "Check" to detect the local AI engine. Place <span class="text-text">llama-server</span> and a <span class="text-text">.gguf</span> model in an <span class="text-text">ai</span> folder next to the app.</p>
          {/if}

          <div class="mt-4">
            <label for="ai-test-prompt" class="text-xs font-medium text-muted">Test prompt</label>
            <textarea id="ai-test-prompt" bind:value={aiPrompt} rows="2" class="mt-1 w-full rounded-md border border-border bg-surface-2 px-3 py-2 text-sm"></textarea>
            <button onclick={runAiTest} disabled={aiThinking} class="mt-2 inline-flex items-center gap-2 rounded-md bg-accent px-4 py-2 text-sm font-medium text-white hover:opacity-90 disabled:opacity-50">
              {aiThinking ? "Thinking…" : "Test AI"}
            </button>
          </div>
          {#if aiReasoning}
            <details class="mt-3 text-xs text-muted">
              <summary class="cursor-pointer select-none">💭 Reasoning {aiThinking ? "(thinking…)" : ""}</summary>
              <div class="mt-1 whitespace-pre-wrap rounded-md border border-border bg-surface-2 px-3 py-2">{aiReasoning}</div>
            </details>
          {/if}
          {#if aiReply}
            <div class="mt-3 rounded-md border border-border bg-surface-2 px-3 py-2 text-sm"><Markdown source={aiReply} /></div>
          {/if}
          {#if aiThinking && !aiReply && !aiReasoning}
            <p class="text-sm text-muted mt-3">Starting the engine and thinking…</p>
          {/if}
          {#if aiError}<p class="text-sm text-danger mt-2">⚠️ {aiError}</p>{/if}
        </div>
      </div>
    {:else if view === "matches"}
      <div class="p-6 max-w-3xl mx-auto">
        <div class="flex items-center justify-between mb-4">
          <div>
            <h1 class="text-xl font-medium">Matches</h1>
            <p class="text-sm text-muted">Tracked automatically from the Arena logs</p>
          </div>
          <button onclick={loadMatches} disabled={matchesLoading} class="inline-flex items-center gap-2 rounded-md border border-border px-3 py-2 text-sm text-muted hover:border-accent hover:text-text disabled:opacity-40">
            <RefreshCw size={15} class={matchesLoading ? "animate-spin" : ""} /> Refresh
          </button>
        </div>

        {#if matches.length > 0}
          <div class="grid grid-cols-4 gap-2 mb-5">
            <div class="bg-surface border border-border rounded-lg px-3 py-2"><div class="text-[11px] text-muted">Matches</div><div class="text-lg font-medium">{matchStats.total}</div></div>
            <div class="bg-surface border border-border rounded-lg px-3 py-2"><div class="text-[11px] text-muted">Wins</div><div class="text-lg font-medium text-success">{matchStats.wins}</div></div>
            <div class="bg-surface border border-border rounded-lg px-3 py-2"><div class="text-[11px] text-muted">Losses</div><div class="text-lg font-medium text-danger">{matchStats.losses}</div></div>
            <div class="bg-surface border border-border rounded-lg px-3 py-2"><div class="text-[11px] text-muted">Win rate</div><div class="text-lg font-medium">{matchStats.winRate}%</div></div>
          </div>

          <div class="flex flex-col gap-1.5">
            {#each matches as m (m.match_id)}
              <div class="flex items-center gap-3 bg-surface border border-border rounded-lg px-3 py-2">
                <span class="w-12 text-center text-sm font-medium {m.result === 'win' ? 'text-success' : m.result === 'loss' ? 'text-danger' : 'text-muted'}">
                  {m.result === "win" ? "Win" : m.result === "loss" ? "Loss" : "Draw"}
                </span>
                <span class="text-xs text-muted w-10 text-center">{m.games_won}–{m.games_lost}</span>
                <div class="flex-1 min-w-0">
                  <div class="text-sm truncate">
                    {#if m.deck_name}<span class="text-text">{m.deck_name}</span>{:else}<span class="text-muted">Unknown deck</span>{/if}
                    <span class="text-muted">vs {m.opponent}</span>
                  </div>
                  <div class="text-[11px] text-muted">{matchDate(m.played_at_ms)}</div>
                </div>
                {#if m.format}<span class="text-[10px] px-2 py-0.5 rounded bg-accent-soft text-accent shrink-0">{m.format}</span>{/if}
              </div>
            {/each}
          </div>
        {:else}
          <div class="rounded-lg border border-dashed border-border p-10 text-center">
            <p class="text-sm text-muted">No matches tracked yet.</p>
            <p class="text-faint text-xs mt-2 leading-relaxed">
              Make sure <span class="text-muted">Detailed Logs (Plugin Support)</span> is enabled in Arena
              (Settings → Account), then play a game. Matches appear here automatically.
            </p>
          </div>
        {/if}
      </div>
    {:else}
      <div class="p-6 max-w-3xl mx-auto">
        <h1 class="text-xl font-medium">Collection</h1>
        <p class="text-sm text-muted mb-4">Wildcards and currencies, read from the Arena log.</p>
        {#if inventory}
          <div class="text-xs font-medium text-muted mb-2">Wildcards</div>
          <div class="grid grid-cols-2 sm:grid-cols-4 gap-3 mb-5">
            <div class="bg-surface border border-border rounded-lg p-3"><div class="text-[11px] text-muted">Common</div><div class="text-2xl font-medium">{inventory.wc_common}</div></div>
            <div class="bg-surface border border-border rounded-lg p-3"><div class="text-[11px] text-muted">Uncommon</div><div class="text-2xl font-medium">{inventory.wc_uncommon}</div></div>
            <div class="bg-surface border border-border rounded-lg p-3"><div class="text-[11px] text-muted">Rare</div><div class="text-2xl font-medium" style="color:#d6b24a">{inventory.wc_rare}</div></div>
            <div class="bg-surface border border-border rounded-lg p-3"><div class="text-[11px] text-muted">Mythic</div><div class="text-2xl font-medium" style="color:#e0682a">{inventory.wc_mythic}</div></div>
          </div>
          <div class="text-xs font-medium text-muted mb-2">Currencies</div>
          <div class="grid grid-cols-3 gap-3">
            <div class="bg-surface border border-border rounded-lg p-3"><div class="text-[11px] text-muted">Gold</div><div class="text-lg font-medium">{inventory.gold.toLocaleString("en-US")}</div></div>
            <div class="bg-surface border border-border rounded-lg p-3"><div class="text-[11px] text-muted">Gems</div><div class="text-lg font-medium">{inventory.gems.toLocaleString("en-US")}</div></div>
            <div class="bg-surface border border-border rounded-lg p-3"><div class="text-[11px] text-muted">Vault</div><div class="text-lg font-medium">{(inventory.vault / 10).toFixed(1)}%</div></div>
          </div>
          <p class="text-faint text-xs mt-5 leading-relaxed">
            Note: Arena stopped logging the full card collection in 2021, so "cards you own / are missing" can't be read by a local tracker. Wildcards and currencies are read live from the log; deck craft cost is shown per deck in the editor.
          </p>
        {:else}
          <p class="text-sm text-muted">No inventory data found yet. Open MTG Arena (its home screen logs your wildcards) and reopen this view.</p>
        {/if}
      </div>
    {/if}
  </main>
</div>

{#if previewCard?.image_normal}
  <img
    src={previewCard.image_normal}
    alt={previewCard.name}
    class="fixed z-40 w-[224px] rounded-xl border border-border-strong shadow-2xl pointer-events-none"
    style={previewStyle()}
  />
{/if}

{#if selected}
  <div class="fixed inset-0 z-50 bg-black/60 grid place-items-center p-4" onclick={() => (selected = null)} role="presentation">
    <div class="bg-surface border border-border rounded-xl max-w-xl w-full p-5 flex gap-5" onclick={(e) => e.stopPropagation()} role="presentation">
      {#if selected.image_normal}<img src={selected.image_normal} alt={selected.name} class="w-[230px] rounded-lg shrink-0" />{/if}
      <div class="min-w-0 flex-1">
        <div class="flex items-start justify-between gap-3">
          <h3 class="text-lg font-medium">{selected.name}</h3>
          <button onclick={() => (selected = null)} aria-label="Close" class="text-muted hover:text-text"><X size={18} /></button>
        </div>
        <dl class="mt-3 text-sm grid grid-cols-[auto_1fr] gap-x-3 gap-y-1">
          <dt class="text-muted">Cost</dt><dd>{selected.mana_cost || "—"} (MV {selected.cmc})</dd>
          <dt class="text-muted">Type</dt><dd>{selected.type_line ?? "—"}</dd>
          <dt class="text-muted">Colors</dt><dd>{selected.colors.length ? selected.colors.join(", ") : "Colorless"}</dd>
          <dt class="text-muted">Rarity</dt><dd>{selected.rarity}</dd>
          <dt class="text-muted">Set</dt><dd>{selected.set_name ? selected.set_name + " " : ""}({selected.set_code.toUpperCase()}) {selected.collector_number}</dd>
          <dt class="text-muted">Brawl</dt><dd>{selected.legalities["brawl"] ?? "—"}</dd>
          <dt class="text-muted">Standard</dt><dd>{selected.legalities["standard"] ?? "—"}</dd>
        </dl>
        <button onclick={() => addCardToDeck(selected!)} class="mt-4 inline-flex items-center gap-2 rounded-md bg-success px-3 py-2 text-sm font-medium text-white hover:brightness-110">
          <Plus size={15} /> Add to deck
        </button>
      </div>
    </div>
  </div>
{/if}
