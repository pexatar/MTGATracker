mod db;
mod deck;
mod models;
mod scryfall;

use deck::{DeckAnalysis, LoadedDeck, ParsedDeck};
use models::{Card, DatabaseStatus, DeckSummary, UpdateCheck};
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
fn save_deck(app: AppHandle, id: Option<i64>, name: String, deck: ParsedDeck) -> Result<i64, String> {
    let path = db_path(&app)?;
    let conn = db::open(&path).map_err(|e| e.to_string())?;
    let text = deck::export(&deck);
    match id {
        Some(existing) => {
            db::update_deck(&conn, existing, &name, &text).map_err(|e| e.to_string())?;
            Ok(existing)
        }
        None => db::insert_deck(&conn, &name, &text).map_err(|e| e.to_string()),
    }
}

/// Lists the saved decks (most recent first).
#[tauri::command]
fn list_decks(app: AppHandle) -> Result<Vec<DeckSummary>, String> {
    let path = db_path(&app)?;
    let conn = db::open(&path).map_err(|e| e.to_string())?;
    db::list_decks(&conn).map_err(|e| e.to_string())
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
    db::delete_deck(&conn, id).map_err(|e| e.to_string())
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
        let id = db::insert_deck(&conn, "My Brawl", text).unwrap();

        let list = db::list_decks(&conn).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "My Brawl");

        let (name, saved) = db::get_deck(&conn, id).unwrap().unwrap();
        assert_eq!(name, "My Brawl");
        let parsed = deck::parse_and_resolve(&conn, &saved).unwrap();
        assert_eq!(parsed.total_cards, 10);
        assert_eq!(parsed.unmatched, 0);

        db::update_deck(&conn, id, "Renamed", text).unwrap();
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
        .invoke_handler(tauri::generate_handler![
            get_database_status,
            search_cards,
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
            delete_deck
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
