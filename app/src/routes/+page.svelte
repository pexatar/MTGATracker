<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { onMount } from "svelte";

  type Card = {
    id: string;
    name: string;
    set_code: string;
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

  onMount(async () => {
    await loadStatus();
    await listen<Progress>("db-progress", (event) => {
      progress = event.payload;
    });
    await checkUpdates();
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
      // Offline o errore di rete: non disturbiamo l'utente.
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

  function mb(bytes: number): string {
    return (bytes / 1048576).toFixed(0);
  }

  function progressLabel(p: Progress): string {
    switch (p.phase) {
      case "index":
        return "Contatto Scryfall…";
      case "download":
        return p.total > 0
          ? `Download: ${mb(p.current)} / ${mb(p.total)} MB`
          : `Download: ${mb(p.current)} MB`;
      case "parse":
        return `Lettura carte: ${p.current.toLocaleString("it-IT")} esaminate`;
      case "save":
        return `Salvataggio di ${p.current.toLocaleString("it-IT")} carte di Arena…`;
      case "done":
        return "Completato!";
      default:
        return "Elaborazione…";
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
      <h2>Database delle carte</h2>
      {#if status}
        <span class="badge">{status.card_count.toLocaleString("it-IT")} carte</span>
      {/if}
    </div>

    {#if status && status.last_updated}
      <p class="muted">Ultimo aggiornamento: {status.last_updated.replace("T", " ").replace("Z", " UTC")}</p>
    {:else}
      <p class="muted">Database vuoto: scarica i dati delle carte per iniziare.</p>
    {/if}

    {#if updateInfo && updateInfo.update_available && status && status.card_count > 0}
      <div class="update-banner">
        🆕 {updateInfo.new_cards.toLocaleString("it-IT")} nuove carte disponibili su Arena — aggiorna per averle.
      </div>
    {/if}

    <button class="primary" onclick={runUpdate} disabled={updating}>
      {updating
        ? "Aggiornamento in corso…"
        : updateInfo && updateInfo.update_available && status && status.card_count > 0
          ? "Aggiorna ora le nuove carte"
          : "Aggiorna database carte"}
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
      <h2>Cerca una carta</h2>
    </div>
    <input
      class="search"
      placeholder="Scrivi il nome di una carta (min. 2 lettere)…"
      bind:value={query}
      oninput={onQueryInput}
      disabled={!status || status.card_count === 0}
    />
    {#if !status || status.card_count === 0}
      <p class="muted">Aggiorna prima il database per poter cercare.</p>
    {:else if searching}
      <p class="muted">Ricerca…</p>
    {:else if results.length > 0}
      <ul class="results">
        {#each results as card (card.id)}
          <li>
            <button class="result" onclick={() => (selected = card)}>
              {#if card.image_small}
                <img src={card.image_small} alt={card.name} loading="lazy" />
              {/if}
              <span class="result-info">
                <span class="result-name">{card.name}</span>
                <span class="result-meta">
                  {card.type_line ?? ""} · {card.set_code.toUpperCase()} {card.collector_number} · {card.rarity}
                </span>
              </span>
            </button>
          </li>
        {/each}
      </ul>
    {:else if query.trim().length >= 2}
      <p class="muted">Nessuna carta trovata.</p>
    {/if}
  </section>

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
          <dt>Costo</dt><dd>{selected.mana_cost || "—"} (CMC {selected.cmc})</dd>
          <dt>Tipo</dt><dd>{selected.type_line ?? "—"}</dd>
          <dt>Colori</dt><dd>{selected.colors.length ? selected.colors.join(", ") : "Incolore"}</dd>
          <dt>Rarità</dt><dd>{selected.rarity}</dd>
          <dt>Set</dt><dd>{selected.set_code.toUpperCase()} · n. {selected.collector_number}</dd>
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
