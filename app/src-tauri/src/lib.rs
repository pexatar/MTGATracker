mod ai;
mod arena;
mod db;
mod deck;
mod models;
mod scryfall;

use deck::{DeckAnalysis, LoadedDeck, ParsedDeck};
use models::{Card, DatabaseStatus, DeckSummary, Inventory, MatchRecord, UpdateCheck};
use serde::Serialize;
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, Manager};

/// Update progress, sent to the UI as a "db-progress" event.
#[derive(Clone, Serialize)]
struct Progress {
    /// Current phase: "index", "download", "parse", "save", "done".
    phase: String,
    /// Current value (bytes downloaded or cards examined).
    current: u64,
    /// Total value (total bytes); 0 when unknown.
    total: u64,
}

/// Returns the database path, creating the data directory if needed.
fn db_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Data directory unavailable: {e}"))?;
    db::database_path(&dir).map_err(|e| e.to_string())
}

/// Current state of the card database.
#[tauri::command]
fn get_database_status(app: AppHandle) -> Result<DatabaseStatus, String> {
    let path = db_path(&app)?;
    let conn = db::open(&path).map_err(|e| e.to_string())?;
    db::status(&conn).map_err(|e| e.to_string())
}

/// Searches cards by name (partial).
#[tauri::command]
fn search_cards(app: AppHandle, query: String, limit: Option<i64>) -> Result<Vec<Card>, String> {
    let path = db_path(&app)?;
    let conn = db::open(&path).map_err(|e| e.to_string())?;
    db::search(&conn, &query, limit.unwrap_or(50)).map_err(|e| e.to_string())
}

/// Filters accepted by the advanced card search.
#[derive(serde::Deserialize)]
struct CardFilters {
    query: String,
    #[serde(default)]
    colors: Vec<String>,
    #[serde(default)]
    types: Vec<String>,
    #[serde(default)]
    rarities: Vec<String>,
    format: Option<String>,
    mv_min: Option<f64>,
    mv_max: Option<f64>,
    limit: Option<i64>,
}

/// Advanced card search with filters (color, type, rarity, format, mana value).
#[tauri::command]
fn search_cards_advanced(app: AppHandle, filters: CardFilters) -> Result<Vec<Card>, String> {
    let path = db_path(&app)?;
    let conn = db::open(&path).map_err(|e| e.to_string())?;
    let q = db::CardQuery {
        query: &filters.query,
        colors: &filters.colors,
        types: &filters.types,
        rarities: &filters.rarities,
        format: filters.format.as_deref(),
        mv_min: filters.mv_min,
        mv_max: filters.mv_max,
        limit: filters.limit.unwrap_or(60),
    };
    db::search_advanced(&conn, &q).map_err(|e| e.to_string())
}

/// Details of a single card.
#[tauri::command]
fn get_card(app: AppHandle, id: String) -> Result<Option<Card>, String> {
    let path = db_path(&app)?;
    let conn = db::open(&path).map_err(|e| e.to_string())?;
    db::get_by_id(&conn, &id).map_err(|e| e.to_string())
}

/// Ensures the set list (code -> full name) is available, fetching it from
/// Scryfall once if the local table is empty. Lets existing databases get
/// readable set names without re-downloading all the cards.
#[tauri::command]
async fn ensure_set_names(app: AppHandle) -> Result<i64, String> {
    let path = db_path(&app)?;
    {
        let conn = db::open(&path).map_err(|e| e.to_string())?;
        let existing = db::count_sets(&conn).map_err(|e| e.to_string())?;
        if existing > 0 {
            return Ok(existing);
        }
    }
    let client = scryfall::client()?;
    let sets = scryfall::fetch_sets(&client).await?;
    let mut conn = db::open(&path).map_err(|e| e.to_string())?;
    let n = db::replace_sets(&mut conn, &sets).map_err(|e| e.to_string())?;
    Ok(n as i64)
}

/// Imports an Arena decklist (pasted or read from a file) and resolves every
/// line against the local card database.
#[tauri::command]
fn import_deck(app: AppHandle, text: String) -> Result<ParsedDeck, String> {
    let path = db_path(&app)?;
    let conn = db::open(&path).map_err(|e| e.to_string())?;
    deck::parse_and_resolve(&conn, &text).map_err(|e| e.to_string())
}

/// Rebuilds an Arena-compatible decklist text from a parsed deck.
#[tauri::command]
fn export_deck(deck: ParsedDeck) -> Result<String, String> {
    Ok(deck::export(&deck))
}

/// Computes aggregated statistics for a deck (mana curve, colors, types,
/// rarity, per-format legality) used to draw the charts.
#[tauri::command]
fn analyze_deck(deck: ParsedDeck) -> Result<DeckAnalysis, String> {
    Ok(deck::analyze(&deck))
}

/// Saves a deck locally (creating it, or updating it when `id` is given).
/// Returns the deck id. The deck is stored as Arena text for portability.
#[tauri::command]
fn save_deck(
    app: AppHandle,
    id: Option<i64>,
    name: String,
    format: String,
    deck: ParsedDeck,
) -> Result<i64, String> {
    let path = db_path(&app)?;
    let conn = db::open(&path).map_err(|e| e.to_string())?;
    let text = deck::export(&deck);
    let (card_count, colors, cover) = deck::summary_metadata(&deck);
    let meta = db::DeckMeta {
        format: &format,
        colors: &colors,
        card_count,
        cover_image: cover.as_deref(),
    };
    let deck_id = match id {
        Some(existing) => {
            db::update_deck(&conn, existing, &name, &text, &meta).map_err(|e| e.to_string())?;
            existing
        }
        None => db::insert_deck(&conn, &name, &text, &meta).map_err(|e| e.to_string())?,
    };
    invalidate_assignments();
    Ok(deck_id)
}

/// Basic lands are excluded from deck matching so they don't dominate overlap.
const BASIC_LANDS: [&str; 6] = ["plains", "island", "swamp", "mountain", "forest", "wastes"];

fn name_set(names: impl IntoIterator<Item = String>) -> std::collections::HashSet<String> {
    names
        .into_iter()
        .map(|n| n.to_lowercase())
        .filter(|n| !BASIC_LANDS.contains(&n.as_str()))
        .collect()
}

fn deck_name_set(parsed: &ParsedDeck) -> std::collections::HashSet<String> {
    name_set(
        parsed
            .entries
            .iter()
            .filter_map(|e| e.card.as_ref().map(|c| c.name.clone())),
    )
}

/// Fraction of `a`'s cards that also appear in `b`.
fn overlap(a: &std::collections::HashSet<String>, b: &std::collections::HashSet<String>) -> f64 {
    if a.is_empty() {
        return 0.0;
    }
    let inter = a.iter().filter(|n| b.contains(*n)).count();
    inter as f64 / a.len() as f64
}

type Assignments = std::collections::HashMap<i64, Vec<MatchRecord>>;

/// Bumped whenever decks or matches change. Computing the match→deck assignments
/// re-parses every saved deck (hundreds of card lookups), so doing it on every
/// gallery / deck-open / match-list query stuttered the UI. The result is cached
/// and only recomputed after a real change (deck saved/deleted, new match).
static DATA_VERSION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn invalidate_assignments() {
    DATA_VERSION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

fn assignments_cache() -> &'static std::sync::Mutex<Option<(u64, Assignments)>> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<Option<(u64, Assignments)>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(None))
}

/// Assigns each tracked match to the best-matching saved deck (by card overlap).
/// Memoized against `DATA_VERSION`, so repeated calls without an intervening
/// change (e.g. browsing decks during a game) reuse the previous result.
fn assign_matches(conn: &rusqlite::Connection) -> Result<Assignments, String> {
    let version = DATA_VERSION.load(std::sync::atomic::Ordering::Relaxed);
    if let Ok(guard) = assignments_cache().lock() {
        if let Some((cached_version, cached)) = guard.as_ref() {
            if *cached_version == version {
                return Ok(cached.clone());
            }
        }
    }

    let summaries = db::list_decks(conn).map_err(|e| e.to_string())?;
    let mut deck_sets: Vec<(i64, std::collections::HashSet<String>)> = Vec::new();
    for s in &summaries {
        if let Some((_, text)) = db::get_deck(conn, s.id).map_err(|e| e.to_string())? {
            let parsed = deck::parse_and_resolve(conn, &text).map_err(|e| e.to_string())?;
            deck_sets.push((s.id, deck_name_set(&parsed)));
        }
    }

    let mut map: std::collections::HashMap<i64, Vec<MatchRecord>> = std::collections::HashMap::new();
    for m in db::list_matches(conn).map_err(|e| e.to_string())? {
        let names = db::card_names_by_arena_ids(conn, &m.deck_cards).map_err(|e| e.to_string())?;
        let mset = name_set(names);
        if mset.is_empty() {
            continue;
        }
        let mut best: Option<(i64, f64)> = None;
        for (id, dset) in &deck_sets {
            let o = overlap(&mset, dset);
            if best.map_or(true, |(_, bo)| o > bo) {
                best = Some((*id, o));
            }
        }
        if let Some((id, o)) = best {
            if o >= 0.6 {
                map.entry(id).or_default().push(m);
            }
        }
    }

    if let Ok(mut guard) = assignments_cache().lock() {
        *guard = Some((version, map.clone()));
    }
    Ok(map)
}

/// Lists the saved decks (most recent first), backfilling gallery metadata and
/// adding each deck's win/loss record from tracked matches.
#[tauri::command]
fn list_decks(app: AppHandle) -> Result<Vec<DeckSummary>, String> {
    let path = db_path(&app)?;
    let conn = db::open(&path).map_err(|e| e.to_string())?;
    let mut summaries = db::list_decks(&conn).map_err(|e| e.to_string())?;
    for s in &mut summaries {
        if s.card_count == 0 {
            if let Some((_, text)) = db::get_deck(&conn, s.id).map_err(|e| e.to_string())? {
                let parsed = deck::parse_and_resolve(&conn, &text).map_err(|e| e.to_string())?;
                let (cc, colors, cover) = deck::summary_metadata(&parsed);
                db::set_deck_meta(&conn, s.id, &colors, cc, cover.as_deref())
                    .map_err(|e| e.to_string())?;
                s.card_count = cc;
                s.colors = colors;
                s.cover_image = cover;
            }
        }
    }

    let assignments = assign_matches(&conn).unwrap_or_default();
    for s in &mut summaries {
        if let Some(ms) = assignments.get(&s.id) {
            s.wins = ms.iter().filter(|m| m.result == "win").count() as i64;
            s.losses = ms.iter().filter(|m| m.result == "loss").count() as i64;
        }
    }
    Ok(summaries)
}

/// Returns the tracked matches played with a given saved deck.
#[tauri::command]
fn deck_matches(app: AppHandle, deck_id: i64) -> Result<Vec<MatchRecord>, String> {
    let path = db_path(&app)?;
    let conn = db::open(&path).map_err(|e| e.to_string())?;
    let mut map = assign_matches(&conn)?;
    Ok(map.remove(&deck_id).unwrap_or_default())
}

/// Loads a saved deck and re-resolves its cards against the current database.
#[tauri::command]
fn load_deck(app: AppHandle, id: i64) -> Result<LoadedDeck, String> {
    let path = db_path(&app)?;
    let conn = db::open(&path).map_err(|e| e.to_string())?;
    let (name, text) = db::get_deck(&conn, id)
        .map_err(|e| e.to_string())?
        .ok_or("Deck not found")?;
    let deck = deck::parse_and_resolve(&conn, &text).map_err(|e| e.to_string())?;
    Ok(LoadedDeck { id, name, deck })
}

/// Deletes a saved deck.
#[tauri::command]
fn delete_deck(app: AppHandle, id: i64) -> Result<(), String> {
    let path = db_path(&app)?;
    let conn = db::open(&path).map_err(|e| e.to_string())?;
    db::delete_deck(&conn, id).map_err(|e| e.to_string())?;
    invalidate_assignments();
    Ok(())
}

/// Parses the given Arena log files and stores any matches they contain.
/// Returns the total number of stored matches. Taking explicit paths lets the
/// watcher re-read only the file that actually changed, instead of re-reading
/// the (static, possibly large) previous-session log every few seconds.
fn reimport_paths(app: &AppHandle, log_paths: &[PathBuf]) -> Result<i64, String> {
    let path = db_path(app)?;
    let conn = db::open(&path).map_err(|e| e.to_string())?;
    for log_path in log_paths {
        if let Ok(text) = std::fs::read_to_string(log_path) {
            for m in arena::parse_matches(&text) {
                let _ = db::upsert_match(&conn, &m);
            }
        }
    }
    db::count_matches(&conn).map_err(|e| e.to_string())
}

/// Parses both Arena logs (current + previous session) and stores any matches.
fn reimport_matches(app: &AppHandle) -> Result<i64, String> {
    reimport_paths(app, &arena::default_log_paths())
}

/// Imports match history from the Arena logs on demand.
#[tauri::command]
fn import_match_history(app: AppHandle) -> Result<i64, String> {
    let count = reimport_matches(&app)?;
    invalidate_assignments();
    Ok(count)
}

/// Reads the player's inventory summary (wildcards + currencies) from the log.
#[tauri::command]
fn get_inventory(_app: AppHandle) -> Result<Option<Inventory>, String> {
    for path in arena::default_log_paths().iter().rev() {
        if let Ok(text) = std::fs::read_to_string(path) {
            if let Some(inv) = arena::parse_inventory(&text) {
                return Ok(Some(inv));
            }
        }
    }
    Ok(None)
}

/// Status of the local AI engine (binary + model present, server reachable).
#[tauri::command]
async fn ai_status(app: AppHandle) -> Result<ai::AiStatus, String> {
    Ok(ai::status(&app).await)
}

/// Streams a prompt to the local AI engine. Reply text arrives via `ai-delta`
/// events ({kind: "reasoning"|"content", text}) and completion via `ai-done`.
#[tauri::command]
async fn ai_chat_stream(app: AppHandle, prompt: String, think: bool) -> Result<(), String> {
    ai::chat_stream(&app, &prompt, think).await
}

/// Streams an AI coaching analysis of a deck, grounded in its real card list
/// and computed statistics. Uses the same `ai-delta`/`ai-done` events.
#[tauri::command]
async fn ai_analyze_deck(
    app: AppHandle,
    deck: ParsedDeck,
    format: String,
    think: bool,
    matches: Vec<MatchRecord>,
) -> Result<(), String> {
    let analysis = deck::analyze(&deck);
    // `think` is the In-depth/Fast switch: In-depth both tailors the prompt for a
    // deeper analysis (and factors in the tracked games) and lets the model
    // reason; Fast keeps it crisp and quick.
    let prompt = deck::analysis_prompt(&deck, &analysis, &format, think, &matches);
    ai::chat_stream(&app, &prompt, think).await
}

/// Interactive AI coach: a tool-calling conversation. `messages` is the chat so
/// far ([{role, content}, …]); the model can call `search_cards` to look cards
/// up in the real database, and the grounded answer streams via `ai-delta`/
/// `ai-done` (with `ai-tool` events while it searches).
#[tauri::command]
async fn ai_chat_tools(
    app: AppHandle,
    messages: Vec<serde_json::Value>,
    think: bool,
    deck: Option<ParsedDeck>,
    format: String,
    matches: Vec<MatchRecord>,
) -> Result<(), String> {
    let path = db_path(&app)?;

    // The single tool the coach can use for now: a card-database lookup by name.
    let tools = serde_json::json!([{
        "type": "function",
        "function": {
            "name": "search_cards",
            "description": "Cerca carte nel database di MTG Arena per nome o parola chiave. Restituisce le carte corrispondenti con tipo, costo di mana e rarità.",
            "parameters": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Nome o parola chiave della carta da cercare" }
                },
                "required": ["query"]
            }
        }
    }]);

    // Prepend the coach persona + tool-usage instructions.
    let mut full = vec![serde_json::json!({
        "role": "system",
        "content": "Sei un coach esperto di Magic: The Gathering Arena. Rispondi in italiano dando del \"tu\". Hai il tool search_cards per cercare carte nel database reale: usalo quando ti serve sapere costo, tipo, rarità o l'esistenza di una carta specifica, e basa le risposte SOLO sui dati che restituisce (non inventare carte). Mantieni i nomi delle carte in inglese."
    })];
    // If a deck is open in the editor, seed the conversation with its context so
    // the coach's answers are about THIS deck (e.g. "are there too many lands?").
    if let Some(d) = &deck {
        let analysis = deck::analyze(d);
        let ctx = deck::chat_context(d, &analysis, &format, &matches);
        full.push(serde_json::json!({
            "role": "system",
            "content": format!("CONTESTO — il giocatore sta osservando questo mazzo nell'editor:\n{ctx}\nRispondi alle sue domande riferendoti a QUESTO mazzo quando pertinente; usa search_cards per i dettagli delle singole carte.")
        }));
    }
    full.extend(messages);

    // Tool executor (synchronous DB access; the connection lives only here).
    let exec = |name: &str, args: &str| -> Result<String, String> {
        if name != "search_cards" {
            return Ok(format!("Tool sconosciuto: {name}"));
        }
        let parsed: serde_json::Value = serde_json::from_str(args).map_err(|e| e.to_string())?;
        let query = parsed["query"].as_str().unwrap_or_default();
        let conn = db::open(&path).map_err(|e| e.to_string())?;
        let cards = db::search(&conn, query, 15).map_err(|e| e.to_string())?;
        let compact: Vec<serde_json::Value> = cards
            .iter()
            .map(|c| {
                serde_json::json!({
                    "name": c.name,
                    "type": c.type_line,
                    "cmc": c.cmc,
                    "mana_cost": c.mana_cost,
                    "rarity": c.rarity
                })
            })
            .collect();
        Ok(serde_json::to_string(&compact).unwrap_or_else(|_| "[]".to_string()))
    };

    ai::chat_with_tools(&app, full, tools, think, exec).await
}

/// Lists stored matches (most recent first). When a match links to a saved
/// deck, its deck name is replaced with the user's saved deck name (clearer
/// than Arena's auto-generated name).
#[tauri::command]
fn list_matches(app: AppHandle) -> Result<Vec<MatchRecord>, String> {
    let path = db_path(&app)?;
    let conn = db::open(&path).map_err(|e| e.to_string())?;
    let mut matches = db::list_matches(&conn).map_err(|e| e.to_string())?;

    let names: std::collections::HashMap<i64, String> = db::list_decks(&conn)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|s| (s.id, s.name))
        .collect();
    let mut saved_name: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for (deck_id, ms) in assign_matches(&conn)? {
        if let Some(name) = names.get(&deck_id) {
            for m in ms {
                saved_name.insert(m.match_id, name.clone());
            }
        }
    }
    for m in &mut matches {
        if let Some(n) = saved_name.get(&m.match_id) {
            m.deck_name = n.clone();
        }
    }
    Ok(matches)
}

/// Win/loss record grouped by a deck's color identity (e.g. "GW"), so the UI can
/// show how each color combination performs.
#[derive(Serialize)]
struct ColorPerformance {
    /// Color-identity letters in WUBRG order; empty means colorless.
    colors: String,
    wins: u32,
    losses: u32,
}

/// Aggregates the tracked matches by the color identity of the deck they were
/// played with, summing wins and losses across decks that share the same colors.
#[tauri::command]
fn color_performance(app: AppHandle) -> Result<Vec<ColorPerformance>, String> {
    let path = db_path(&app)?;
    let conn = db::open(&path).map_err(|e| e.to_string())?;
    let summaries = db::list_decks(&conn).map_err(|e| e.to_string())?;
    let assignments = assign_matches(&conn)?;

    let mut by_colors: std::collections::HashMap<String, (u32, u32)> = std::collections::HashMap::new();
    for s in &summaries {
        let Some(ms) = assignments.get(&s.id) else { continue };
        let entry = by_colors.entry(s.colors.clone()).or_insert((0, 0));
        for m in ms {
            match m.result.as_str() {
                "win" => entry.0 += 1,
                "loss" => entry.1 += 1,
                _ => {}
            }
        }
    }

    let mut out: Vec<ColorPerformance> = by_colors
        .into_iter()
        .map(|(colors, (wins, losses))| ColorPerformance { colors, wins, losses })
        .collect();
    // Most-played color combinations first.
    out.sort_by(|a, b| (b.wins + b.losses).cmp(&(a.wins + a.losses)));
    Ok(out)
}

/// Background loop: watches the Arena logs and re-imports matches when they
/// change, notifying the UI via the "matches-updated" event.
///
/// Arena rewrites its log on almost every action during a game, so reacting to
/// every change is what froze the app: each notification kicked off a full
/// re-read plus a cascade of UI queries. Two guards keep it cheap: only the log
/// file whose modification time actually changed is re-read, and the UI is
/// notified only when the match count actually changes (i.e. a game finished),
/// not on every write.
fn spawn_match_watcher(app: AppHandle) {
    std::thread::spawn(move || {
        let mut seen: std::collections::HashMap<PathBuf, std::time::SystemTime> =
            std::collections::HashMap::new();
        let mut last_count: i64 = -1;
        loop {
            let changed: Vec<PathBuf> = arena::default_log_paths()
                .into_iter()
                .filter(|p| {
                    let Some(modified) = std::fs::metadata(p).and_then(|m| m.modified()).ok() else {
                        return false;
                    };
                    if seen.get(p) == Some(&modified) {
                        return false;
                    }
                    seen.insert(p.clone(), modified);
                    true
                })
                .collect();

            if !changed.is_empty() {
                if let Ok(count) = reimport_paths(&app, &changed) {
                    if count != last_count {
                        last_count = count;
                        invalidate_assignments();
                        let _ = app.emit("matches-updated", ());
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_secs(3));
        }
    });
}

/// Lightweight check: compares the number of Arena cards on Scryfall with the
/// one from the last update to tell whether new cards have been released. It
/// does not download the whole file: it is a tiny request.
#[tauri::command]
async fn check_for_updates(app: AppHandle) -> Result<UpdateCheck, String> {
    let path = db_path(&app)?;
    let (local_count, known) = {
        let conn = db::open(&path).map_err(|e| e.to_string())?;
        (
            db::count(&conn).map_err(|e| e.to_string())?,
            db::source_arena_count(&conn).map_err(|e| e.to_string())?,
        )
    };

    let client = scryfall::client()?;
    let available = scryfall::fetch_arena_card_count(&client).await?;

    // If the database is empty, downloading is worthwhile anyway.
    if local_count == 0 {
        return Ok(UpdateCheck {
            known_count: 0,
            available_count: available,
            new_cards: available,
            update_available: true,
        });
    }

    // If we don't have a reference yet (e.g. a database created by a previous
    // version), we set it now to the current value: no useless download.
    let known_count = match known {
        Some(k) => k,
        None => {
            let conn = db::open(&path).map_err(|e| e.to_string())?;
            db::set_meta(&conn, "source_arena_count", &available.to_string())
                .map_err(|e| e.to_string())?;
            available
        }
    };

    let new_cards = (available - known_count).max(0);
    Ok(UpdateCheck {
        known_count,
        available_count: available,
        new_cards,
        update_available: new_cards > 0,
    })
}

/// Downloads from Scryfall and updates the local database with the Arena cards only.
#[tauri::command]
async fn update_card_database(app: AppHandle) -> Result<DatabaseStatus, String> {
    let emit = |phase: &str, current: u64, total: u64| {
        let _ = app.emit(
            "db-progress",
            Progress {
                phase: phase.to_string(),
                current,
                total,
            },
        );
    };

    emit("index", 0, 0);
    let client = scryfall::client()?;
    let info = scryfall::fetch_default_cards_info(&client).await?;
    // Scryfall's "official" count: we save it for future checks.
    let arena_count = scryfall::fetch_arena_card_count(&client).await.unwrap_or(0);
    // Full set list (code -> name) for human-readable set names.
    let sets = scryfall::fetch_sets(&client).await.unwrap_or_default();

    // Download to a temporary file, with byte progress.
    let tmp = std::env::temp_dir().join("mtg_arena_tracker_default_cards.json");
    {
        let app_dl = app.clone();
        scryfall::download_to_file(&client, &info.download_uri, &tmp, info.size, move |d, t| {
            let _ = app_dl.emit(
                "db-progress",
                Progress {
                    phase: "download".to_string(),
                    current: d,
                    total: t,
                },
            );
        })
        .await?;
    }

    // Stream-read + save into the database, on a dedicated thread so the UI is
    // not blocked.
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Data directory unavailable: {e}"))?;
    let updated_at = info.updated_at.clone();
    let app_bg = app.clone();
    let source_arena_count = arena_count;
    let status = tauri::async_runtime::spawn_blocking(move || -> Result<DatabaseStatus, String> {
        let file = std::fs::File::open(&tmp)
            .map_err(|e| format!("Could not open the downloaded file: {e}"))?;
        let reader = std::io::BufReader::new(file);
        let cards = scryfall::parse_arena_cards(reader, |processed| {
            let _ = app_bg.emit(
                "db-progress",
                Progress {
                    phase: "parse".to_string(),
                    current: processed as u64,
                    total: 0,
                },
            );
        })
        .map_err(|e| format!("Error reading the card data: {e}"))?;

        let _ = app_bg.emit(
            "db-progress",
            Progress {
                phase: "save".to_string(),
                current: cards.len() as u64,
                total: cards.len() as u64,
            },
        );

        let path = db::database_path(&data_dir).map_err(|e| e.to_string())?;
        let mut conn = db::open(&path).map_err(|e| e.to_string())?;
        db::replace_all_cards(&mut conn, &cards, &updated_at, source_arena_count)
            .map_err(|e| e.to_string())?;
        if !sets.is_empty() {
            db::replace_sets(&mut conn, &sets).map_err(|e| e.to_string())?;
        }
        let _ = std::fs::remove_file(&tmp);
        db::status(&conn).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("Processing interrupted: {e}"))??;

    emit("done", status.card_count as u64, status.card_count as u64);
    Ok(status)
}

#[cfg(test)]
mod tests {
    use crate::models::Card;
    use crate::{db, deck, scryfall};
    use std::collections::HashMap;

    fn mk_card(id: &str, name: &str, set: &str, num: &str) -> Card {
        Card {
            id: id.to_string(),
            oracle_id: None,
            name: name.to_string(),
            set_code: set.to_string(),
            set_name: None,
            collector_number: num.to_string(),
            mana_cost: None,
            cmc: 0.0,
            type_line: None,
            colors: vec![],
            color_identity: vec![],
            rarity: "rare".to_string(),
            layout: "normal".to_string(),
            arena_id: None,
            image_small: None,
            image_normal: None,
            legalities: HashMap::new(),
        }
    }

    /// Imports a small decklist, checks matching by set+number and by name, and
    /// verifies the export round-trip.
    #[test]
    fn deck_import_match_and_export() {
        let dir = std::env::temp_dir().join(format!("mtgdeck_{}", std::process::id()));
        let path = db::database_path(&dir).unwrap();
        let _ = std::fs::remove_file(&path);
        let mut conn = db::open(&path).unwrap();
        let cards = vec![
            mk_card("1", "Omnath, Locus of Creation", "znr", "232"),
            mk_card("2", "Forest", "mom", "290"),
            mk_card("3", "Llanowar Elves", "m19", "314"),
        ];
        db::replace_all_cards(&mut conn, &cards, "2026-06-21T00:00:00Z", 3).unwrap();
        db::replace_sets(
            &mut conn,
            &[
                ("znr".to_string(), "Zendikar Rising".to_string()),
                ("mom".to_string(), "March of the Machine".to_string()),
                ("m19".to_string(), "Core Set 2019".to_string()),
            ],
        )
        .unwrap();

        let text = "Commander\n1 Omnath, Locus of Creation (ZNR) 232\n\nDeck\n9 Forest (MOM) 290\n4 Llanowar Elves";
        let parsed = deck::parse_and_resolve(&conn, text).unwrap();

        assert_eq!(parsed.entries.len(), 3);
        assert_eq!(parsed.total_cards, 14);
        assert_eq!(parsed.unmatched, 0, "all lines should match");
        // The full set name is resolved from the sets table.
        let omnath = parsed.entries.iter().find(|e| e.name.starts_with("Omnath")).unwrap();
        assert_eq!(
            omnath.card.as_ref().unwrap().set_name.as_deref(),
            Some("Zendikar Rising")
        );
        // Name-only line resolved to a real printing.
        let elves = parsed.entries.iter().find(|e| e.name == "Llanowar Elves").unwrap();
        assert!(elves.matched && elves.card.is_some());

        let exported = deck::export(&parsed);
        assert!(exported.contains("Commander\n1 Omnath, Locus of Creation (ZNR) 232"));
        assert!(exported.contains("Deck\n9 Forest (MOM) 290"));
        // The name-only line gains a set/number from the matched card.
        assert!(exported.contains("4 Llanowar Elves (M19) 314"));

        drop(conn);
        let _ = std::fs::remove_file(&path);
    }

    /// Verifies that streaming keeps ONLY English-language Arena cards and that
    /// saving and searching in the database work.
    #[test]
    fn parse_filters_and_db_roundtrip() {
        let json = r#"[
          {"id":"a","name":"Arena Card","lang":"en","set":"znr","collector_number":"1","rarity":"rare","layout":"normal","games":["arena","paper"],"cmc":2.0,"type_line":"Creature","colors":["G"],"color_identity":["G"],"legalities":{"brawl":"legal"},"image_uris":{"small":"s","normal":"n"}},
          {"id":"b","name":"Paper Only","lang":"en","set":"abc","collector_number":"2","rarity":"rare","layout":"normal","games":["paper"]},
          {"id":"c","name":"Arena Italian","lang":"it","set":"znr","collector_number":"3","rarity":"rare","layout":"normal","games":["arena"]}
        ]"#;

        let cards = scryfall::parse_arena_cards(json.as_bytes(), |_| {}).unwrap();
        assert_eq!(cards.len(), 1, "only the English Arena card must remain");
        assert_eq!(cards[0].name, "Arena Card");
        assert_eq!(cards[0].image_normal.as_deref(), Some("n"));

        let dir = std::env::temp_dir().join(format!("mtgtest_{}", std::process::id()));
        let path = db::database_path(&dir).unwrap();
        let _ = std::fs::remove_file(&path);
        let mut conn = db::open(&path).unwrap();
        db::replace_all_cards(&mut conn, &cards, "2026-06-21T00:00:00Z", 1).unwrap();

        assert_eq!(db::count(&conn).unwrap(), 1);
        let found = db::search(&conn, "arena", 10).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].set_code, "znr");
        assert_eq!(found[0].legalities.get("brawl").map(String::as_str), Some("legal"));

        drop(conn);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn advanced_search_filters() {
        let dir = std::env::temp_dir().join(format!("mtgadv_{}", std::process::id()));
        let path = db::database_path(&dir).unwrap();
        let _ = std::fs::remove_file(&path);
        let mut conn = db::open(&path).unwrap();

        let mut elf = mk_card("1", "Llanowar Elves", "m19", "314");
        elf.cmc = 1.0;
        elf.type_line = Some("Creature — Elf Druid".into());
        elf.color_identity = vec!["G".into()];
        elf.rarity = "common".into();
        elf.legalities = HashMap::from([("standard".to_string(), "legal".to_string())]);

        let mut bolt = mk_card("2", "Lightning Bolt", "sta", "42");
        bolt.cmc = 1.0;
        bolt.type_line = Some("Instant".into());
        bolt.color_identity = vec!["R".into()];
        bolt.rarity = "rare".into();
        bolt.legalities = HashMap::from([("standard".to_string(), "not_legal".to_string())]);

        let mut relic = mk_card("3", "Ancient Relic", "m19", "200");
        relic.cmc = 3.0;
        relic.type_line = Some("Artifact".into());
        relic.color_identity = vec![];
        relic.rarity = "mythic".into();
        relic.legalities = HashMap::from([("standard".to_string(), "legal".to_string())]);

        db::replace_all_cards(&mut conn, &[elf, bolt, relic], "x", 3).unwrap();

        let none: Vec<String> = vec![];
        let base = |colors, types, rarities, format, mv_min, mv_max| db::CardQuery {
            query: "",
            colors,
            types,
            rarities,
            format,
            mv_min,
            mv_max,
            limit: 50,
        };

        // Color identity subset of {G}: green card + colorless artifact.
        let g = vec!["G".to_string()];
        assert_eq!(db::search_advanced(&conn, &base(&g, &none, &none, None, None, None)).unwrap().len(), 2);
        // Type Instant only.
        let ti = vec!["Instant".to_string()];
        let r = db::search_advanced(&conn, &base(&none, &ti, &none, None, None, None)).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].name, "Lightning Bolt");
        // Rarity mythic only.
        let rm = vec!["mythic".to_string()];
        assert_eq!(db::search_advanced(&conn, &base(&none, &none, &rm, None, None, None)).unwrap().len(), 1);
        // Legal in standard: elf + relic.
        assert_eq!(db::search_advanced(&conn, &base(&none, &none, &none, Some("standard"), None, None)).unwrap().len(), 2);
        // Mana value >= 2: only the artifact.
        assert_eq!(db::search_advanced(&conn, &base(&none, &none, &none, None, Some(2.0), None)).unwrap().len(), 1);

        drop(conn);
        let _ = std::fs::remove_file(&path);
    }

    /// Saves a deck, lists it, loads (re-resolves) it, renames and deletes it.
    #[test]
    fn deck_persistence_roundtrip() {
        let dir = std::env::temp_dir().join(format!("mtgsave_{}", std::process::id()));
        let path = db::database_path(&dir).unwrap();
        let _ = std::fs::remove_file(&path);
        let mut conn = db::open(&path).unwrap();
        let cards = vec![
            mk_card("1", "Omnath, Locus of Creation", "znr", "232"),
            mk_card("2", "Forest", "mom", "290"),
        ];
        db::replace_all_cards(&mut conn, &cards, "2026-06-21T00:00:00Z", 2).unwrap();

        let text = "Commander\n1 Omnath, Locus of Creation (ZNR) 232\n\nDeck\n9 Forest (MOM) 290";
        let meta = db::DeckMeta {
            format: "brawl",
            colors: "G",
            card_count: 10,
            cover_image: None,
        };
        let id = db::insert_deck(&conn, "My Brawl", text, &meta).unwrap();

        let list = db::list_decks(&conn).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "My Brawl");
        assert_eq!(list[0].format, "brawl");
        assert_eq!(list[0].card_count, 10);

        let (name, saved) = db::get_deck(&conn, id).unwrap().unwrap();
        assert_eq!(name, "My Brawl");
        let parsed = deck::parse_and_resolve(&conn, &saved).unwrap();
        assert_eq!(parsed.total_cards, 10);
        assert_eq!(parsed.unmatched, 0);

        db::update_deck(&conn, id, "Renamed", text, &meta).unwrap();
        assert_eq!(db::get_deck(&conn, id).unwrap().unwrap().0, "Renamed");

        db::delete_deck(&conn, id).unwrap();
        assert_eq!(db::list_decks(&conn).unwrap().len(), 0);

        drop(conn);
        let _ = std::fs::remove_file(&path);
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            spawn_match_watcher(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_database_status,
            search_cards,
            search_cards_advanced,
            get_card,
            update_card_database,
            check_for_updates,
            import_deck,
            export_deck,
            analyze_deck,
            ensure_set_names,
            save_deck,
            list_decks,
            load_deck,
            delete_deck,
            import_match_history,
            list_matches,
            color_performance,
            deck_matches,
            get_inventory,
            ai_status,
            ai_chat_stream,
            ai_analyze_deck,
            ai_chat_tools
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app_handle, event| {
            // Stop the AI sidecar when the app exits, so it doesn't linger.
            if let tauri::RunEvent::Exit = event {
                ai::stop();
            }
        });
}
