//! Communication with Scryfall: discovering the "bulk" file, downloading it
//! with progress, and reading it as a stream to avoid exhausting memory.

use crate::models::{Card, ScryfallCard};
use futures_util::StreamExt;
use serde::de::{SeqAccess, Visitor};
use std::fmt;
use std::io::Write;
use std::path::Path;

const SCRYFALL_BULK_INDEX: &str = "https://api.scryfall.com/bulk-data";
const SCRYFALL_ARENA_COUNT: &str =
    "https://api.scryfall.com/cards/search?q=game%3Aarena&unique=prints";
const USER_AGENT: &str = "MTGArenaTracker/0.1";

/// Information about the chosen bulk file.
pub struct BulkInfo {
    pub download_uri: String,
    pub size: u64,
    pub updated_at: String,
}

/// Creates an HTTP client with the User-Agent required by Scryfall.
pub fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| e.to_string())
}

/// Queries the Scryfall bulk index and returns the info for the "default_cards"
/// dataset (every printing, with set/number/rarity/image). The file URL changes
/// at every update, so we read it dynamically.
pub async fn fetch_default_cards_info(client: &reqwest::Client) -> Result<BulkInfo, String> {
    let resp = client
        .get(SCRYFALL_BULK_INDEX)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| format!("Network error contacting Scryfall: {e}"))?;
    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Unreadable Scryfall response: {e}"))?;
    let data = json
        .get("data")
        .and_then(|d| d.as_array())
        .ok_or("Invalid Scryfall bulk index")?;
    let entry = data
        .iter()
        .find(|e| e.get("type").and_then(|t| t.as_str()) == Some("default_cards"))
        .ok_or("'default_cards' dataset not found on Scryfall")?;
    Ok(BulkInfo {
        download_uri: entry
            .get("download_uri")
            .and_then(|v| v.as_str())
            .ok_or("Missing download URL")?
            .to_string(),
        size: entry.get("size").and_then(|v| v.as_u64()).unwrap_or(0),
        updated_at: entry
            .get("updated_at")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
    })
}

/// Asks Scryfall how many cards (printings) are currently available on Arena.
/// This is a tiny request (a few bytes): we use it to tell whether new cards
/// have been released, without downloading the whole file.
pub async fn fetch_arena_card_count(client: &reqwest::Client) -> Result<i64, String> {
    let resp = client
        .get(SCRYFALL_ARENA_COUNT)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| format!("Network error contacting Scryfall: {e}"))?;
    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Unreadable Scryfall response: {e}"))?;
    json.get("total_cards")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| "Card count not available".to_string())
}

/// Downloads a file to disk, reporting progress (bytes downloaded, total bytes).
pub async fn download_to_file<F: Fn(u64, u64)>(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    size_hint: u64,
    on_progress: F,
) -> Result<(), String> {
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("Error starting the download: {e}"))?;
    let total = resp.content_length().unwrap_or(size_hint);
    let mut file =
        std::fs::File::create(dest).map_err(|e| format!("Could not create the file: {e}"))?;
    let mut stream = resp.bytes_stream();
    let mut downloaded: u64 = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("Download interrupted: {e}"))?;
        file.write_all(&chunk)
            .map_err(|e| format!("Error writing to disk: {e}"))?;
        downloaded += chunk.len() as u64;
        on_progress(downloaded, total);
    }
    file.flush().map_err(|e| e.to_string())?;
    Ok(())
}

/// Streams a Scryfall bulk file and returns only the English-language Arena
/// cards. Reports how many cards it has examined so far.
pub fn parse_arena_cards<R: std::io::Read, F: FnMut(usize)>(
    reader: R,
    on_progress: F,
) -> serde_json::Result<Vec<Card>> {
    let mut de = serde_json::Deserializer::from_reader(reader);
    serde::Deserializer::deserialize_seq(&mut de, ArenaCardsVisitor { on_progress })
}

/// Visitor that walks the JSON array one card at a time, keeping Arena cards.
struct ArenaCardsVisitor<F: FnMut(usize)> {
    on_progress: F,
}

impl<'de, F: FnMut(usize)> Visitor<'de> for ArenaCardsVisitor<F> {
    type Value = Vec<Card>;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("an array of Scryfall cards")
    }

    fn visit_seq<A: SeqAccess<'de>>(mut self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut out: Vec<Card> = Vec::new();
        let mut processed: usize = 0;
        while let Some(card) = seq.next_element::<ScryfallCard>()? {
            processed += 1;
            if card.lang == "en" && card.is_on_arena() {
                out.push(card.into_card());
            }
            if processed % 5000 == 0 {
                (self.on_progress)(processed);
            }
        }
        (self.on_progress)(processed);
        Ok(out)
    }
}
