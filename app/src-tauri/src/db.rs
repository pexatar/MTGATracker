//! Local SQLite card database management.

use crate::models::{Card, DatabaseStatus};
use rusqlite::{params, Connection};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Path of the database file, inside the app data directory.
/// Creates the directory if it does not exist.
pub fn database_path(app_data_dir: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(app_data_dir)?;
    Ok(app_data_dir.join("cards.sqlite"))
}

/// Opens the connection and makes sure the schema exists.
pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    init_schema(&conn)?;
    Ok(conn)
}

fn init_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS cards (
            id                TEXT PRIMARY KEY,
            oracle_id         TEXT,
            name              TEXT NOT NULL,
            set_code          TEXT NOT NULL,
            collector_number  TEXT NOT NULL,
            mana_cost         TEXT,
            cmc               REAL NOT NULL DEFAULT 0,
            type_line         TEXT,
            colors            TEXT NOT NULL DEFAULT '[]',
            color_identity    TEXT NOT NULL DEFAULT '[]',
            rarity            TEXT NOT NULL DEFAULT '',
            layout            TEXT NOT NULL DEFAULT '',
            arena_id          INTEGER,
            image_small       TEXT,
            image_normal      TEXT,
            legalities        TEXT NOT NULL DEFAULT '{}'
        );
        CREATE INDEX IF NOT EXISTS idx_cards_name ON cards(name);
        CREATE INDEX IF NOT EXISTS idx_cards_setnum ON cards(set_code, collector_number);

        CREATE TABLE IF NOT EXISTS meta (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        ",
    )
}

pub fn set_meta(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO meta(key, value) VALUES(?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

pub fn get_meta(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT value FROM meta WHERE key = ?1",
        params![key],
        |row| row.get::<_, String>(0),
    )
    .map(Some)
    .or_else(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        other => Err(other),
    })
}

/// Replaces all stored cards with the new ones (in a single transaction, for
/// speed). Also records the update timestamps.
pub fn replace_all_cards(
    conn: &mut Connection,
    cards: &[Card],
    source_updated_at: &str,
    source_arena_count: i64,
) -> rusqlite::Result<usize> {
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM cards", [])?;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO cards (
                id, oracle_id, name, set_code, collector_number, mana_cost, cmc,
                type_line, colors, color_identity, rarity, layout, arena_id,
                image_small, image_normal, legalities
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16
            )",
        )?;
        for c in cards {
            stmt.execute(params![
                c.id,
                c.oracle_id,
                c.name,
                c.set_code,
                c.collector_number,
                c.mana_cost,
                c.cmc,
                c.type_line,
                serde_json::to_string(&c.colors).unwrap_or_else(|_| "[]".into()),
                serde_json::to_string(&c.color_identity).unwrap_or_else(|_| "[]".into()),
                c.rarity,
                c.layout,
                c.arena_id,
                c.image_small,
                c.image_normal,
                serde_json::to_string(&c.legalities).unwrap_or_else(|_| "{}".into()),
            ])?;
        }
    }
    let now = iso_now();
    set_meta(&tx, "last_updated", &now)?;
    set_meta(&tx, "source_updated_at", source_updated_at)?;
    set_meta(&tx, "source_arena_count", &source_arena_count.to_string())?;
    tx.commit()?;
    Ok(cards.len())
}

/// Number of stored cards.
pub fn count(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("SELECT COUNT(*) FROM cards", [], |row| row.get(0))
}

/// Database status for the UI.
pub fn status(conn: &Connection) -> rusqlite::Result<DatabaseStatus> {
    Ok(DatabaseStatus {
        card_count: count(conn)?,
        last_updated: get_meta(conn, "last_updated")?,
        source_updated_at: get_meta(conn, "source_updated_at")?,
    })
}

/// The "official" Scryfall count saved at the last update (used to compare it
/// with the current one and tell whether new cards have been released).
pub fn source_arena_count(conn: &Connection) -> rusqlite::Result<Option<i64>> {
    Ok(get_meta(conn, "source_arena_count")?.and_then(|v| v.parse::<i64>().ok()))
}

/// Searches cards by name (partial, case-insensitive).
pub fn search(conn: &Connection, query: &str, limit: i64) -> rusqlite::Result<Vec<Card>> {
    let pattern = format!("%{}%", query.trim());
    let mut stmt = conn.prepare(
        "SELECT id, oracle_id, name, set_code, collector_number, mana_cost, cmc,
                type_line, colors, color_identity, rarity, layout, arena_id,
                image_small, image_normal, legalities
         FROM cards
         WHERE name LIKE ?1
         ORDER BY length(name), name
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![pattern, limit], row_to_card)?;
    rows.collect()
}

/// Returns a single card given its identifier.
pub fn get_by_id(conn: &Connection, id: &str) -> rusqlite::Result<Option<Card>> {
    let mut stmt = conn.prepare(
        "SELECT id, oracle_id, name, set_code, collector_number, mana_cost, cmc,
                type_line, colors, color_identity, rarity, layout, arena_id,
                image_small, image_normal, legalities
         FROM cards WHERE id = ?1",
    )?;
    let mut rows = stmt.query_map(params![id], row_to_card)?;
    match rows.next() {
        Some(card) => Ok(Some(card?)),
        None => Ok(None),
    }
}

const CARD_COLUMNS: &str = "id, oracle_id, name, set_code, collector_number, mana_cost, cmc,
        type_line, colors, color_identity, rarity, layout, arena_id,
        image_small, image_normal, legalities";

/// Looks up a specific printing by set code (case-insensitive) and collector number.
pub fn get_by_set_and_number(
    conn: &Connection,
    set_code: &str,
    collector_number: &str,
) -> rusqlite::Result<Option<Card>> {
    let sql = format!(
        "SELECT {CARD_COLUMNS} FROM cards
         WHERE set_code = ?1 COLLATE NOCASE AND collector_number = ?2 LIMIT 1"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query_map(params![set_code, collector_number], row_to_card)?;
    match rows.next() {
        Some(card) => Ok(Some(card?)),
        None => Ok(None),
    }
}

/// Looks up a card by exact name (case-insensitive); returns one printing.
pub fn get_by_exact_name(conn: &Connection, name: &str) -> rusqlite::Result<Option<Card>> {
    let sql = format!(
        "SELECT {CARD_COLUMNS} FROM cards
         WHERE name = ?1 COLLATE NOCASE ORDER BY length(collector_number), collector_number LIMIT 1"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query_map(params![name], row_to_card)?;
    match rows.next() {
        Some(card) => Ok(Some(card?)),
        None => Ok(None),
    }
}

fn row_to_card(row: &rusqlite::Row) -> rusqlite::Result<Card> {
    let colors_json: String = row.get(8)?;
    let identity_json: String = row.get(9)?;
    let legalities_json: String = row.get(15)?;
    Ok(Card {
        id: row.get(0)?,
        oracle_id: row.get(1)?,
        name: row.get(2)?,
        set_code: row.get(3)?,
        collector_number: row.get(4)?,
        mana_cost: row.get(5)?,
        cmc: row.get(6)?,
        type_line: row.get(7)?,
        colors: serde_json::from_str(&colors_json).unwrap_or_default(),
        color_identity: serde_json::from_str(&identity_json).unwrap_or_default(),
        rarity: row.get(10)?,
        layout: row.get(11)?,
        arena_id: row.get(12)?,
        image_small: row.get(13)?,
        image_normal: row.get(14)?,
        legalities: serde_json::from_str::<HashMap<String, String>>(&legalities_json)
            .unwrap_or_default(),
    })
}

/// Current date/time in ISO 8601 format (UTC), without external dependencies.
fn iso_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Simple conversion from a UNIX timestamp to a readable UTC date/time.
    let days = secs / 86400;
    let rem = secs % 86400;
    let (h, mi, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, m, d) = civil_from_days(days as i64);
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, m, d, h, mi, s)
}

/// Converts a number of days since the UNIX epoch into (year, month, day).
/// Howard Hinnant's algorithm, free of external dependencies.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}
