mod db;
mod models;
mod scryfall;

use models::{Card, DatabaseStatus, UpdateCheck};
use serde::Serialize;
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, Manager};

/// Avanzamento dell'aggiornamento, inviato all'interfaccia come evento "db-progress".
#[derive(Clone, Serialize)]
struct Progress {
    /// Fase corrente: "index", "download", "parse", "save", "done".
    phase: String,
    /// Valore corrente (byte scaricati o carte esaminate).
    current: u64,
    /// Valore totale (byte totali); 0 quando non noto.
    total: u64,
}

/// Restituisce il percorso del database, creando la cartella dati se serve.
fn db_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Cartella dati non disponibile: {e}"))?;
    db::database_path(&dir).map_err(|e| e.to_string())
}

/// Stato attuale del database delle carte.
#[tauri::command]
fn get_database_status(app: AppHandle) -> Result<DatabaseStatus, String> {
    let path = db_path(&app)?;
    let conn = db::open(&path).map_err(|e| e.to_string())?;
    db::status(&conn).map_err(|e| e.to_string())
}

/// Cerca carte per nome (parziale).
#[tauri::command]
fn search_cards(app: AppHandle, query: String, limit: Option<i64>) -> Result<Vec<Card>, String> {
    let path = db_path(&app)?;
    let conn = db::open(&path).map_err(|e| e.to_string())?;
    db::search(&conn, &query, limit.unwrap_or(50)).map_err(|e| e.to_string())
}

/// Dettaglio di una singola carta.
#[tauri::command]
fn get_card(app: AppHandle, id: String) -> Result<Option<Card>, String> {
    let path = db_path(&app)?;
    let conn = db::open(&path).map_err(|e| e.to_string())?;
    db::get_by_id(&conn, &id).map_err(|e| e.to_string())
}

/// Controllo leggero: confronta il numero di carte Arena su Scryfall con quello
/// dell'ultimo aggiornamento per capire se sono uscite carte nuove. Non scarica
/// l'intero file: è una richiesta piccolissima.
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

    // Se il database è vuoto, conviene comunque scaricare.
    if local_count == 0 {
        return Ok(UpdateCheck {
            known_count: 0,
            available_count: available,
            new_cards: available,
            update_available: true,
        });
    }

    // Se non abbiamo ancora un riferimento (es. database creato da una versione
    // precedente), lo impostiamo ora al valore attuale: niente download inutile.
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

/// Scarica da Scryfall e aggiorna il database locale con le sole carte di Arena.
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
    // Conteggio "ufficiale" di Scryfall: lo salviamo per i futuri controlli.
    let arena_count = scryfall::fetch_arena_card_count(&client).await.unwrap_or(0);

    // Download su file temporaneo, con avanzamento in byte.
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

    // Lettura a flusso + salvataggio nel database, su thread dedicato per non
    // bloccare l'interfaccia.
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Cartella dati non disponibile: {e}"))?;
    let updated_at = info.updated_at.clone();
    let app_bg = app.clone();
    let source_arena_count = arena_count;
    let status = tauri::async_runtime::spawn_blocking(move || -> Result<DatabaseStatus, String> {
        let file = std::fs::File::open(&tmp)
            .map_err(|e| format!("Impossibile aprire il file scaricato: {e}"))?;
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
        .map_err(|e| format!("Errore leggendo i dati delle carte: {e}"))?;

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
        let _ = std::fs::remove_file(&tmp);
        db::status(&conn).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("Elaborazione interrotta: {e}"))??;

    emit("done", status.card_count as u64, status.card_count as u64);
    Ok(status)
}

#[cfg(test)]
mod tests {
    use crate::{db, scryfall};

    /// Verifica che la lettura tenga SOLO le carte di Arena in inglese e che
    /// salvataggio e ricerca nel database funzionino.
    #[test]
    fn parse_filters_and_db_roundtrip() {
        let json = r#"[
          {"id":"a","name":"Arena Card","lang":"en","set":"znr","collector_number":"1","rarity":"rare","layout":"normal","games":["arena","paper"],"cmc":2.0,"type_line":"Creature","colors":["G"],"color_identity":["G"],"legalities":{"brawl":"legal"},"image_uris":{"small":"s","normal":"n"}},
          {"id":"b","name":"Paper Only","lang":"en","set":"abc","collector_number":"2","rarity":"rare","layout":"normal","games":["paper"]},
          {"id":"c","name":"Arena Italian","lang":"it","set":"znr","collector_number":"3","rarity":"rare","layout":"normal","games":["arena"]}
        ]"#;

        let cards = scryfall::parse_arena_cards(json.as_bytes(), |_| {}).unwrap();
        assert_eq!(cards.len(), 1, "deve restare solo la carta Arena in inglese");
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
            check_for_updates
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
