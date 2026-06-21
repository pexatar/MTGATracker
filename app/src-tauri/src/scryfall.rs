//! Comunicazione con Scryfall: scoperta del file "bulk", download con
//! avanzamento e lettura a flusso (streaming) per non saturare la memoria.

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

/// Informazioni sul file bulk scelto.
pub struct BulkInfo {
    pub download_uri: String,
    pub size: u64,
    pub updated_at: String,
}

/// Crea un client HTTP con lo User-Agent richiesto da Scryfall.
pub fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| e.to_string())
}

/// Interroga l'indice bulk di Scryfall e restituisce le info del dataset
/// "default_cards" (ogni stampa, con set/numero/rarità/immagine). L'URL del
/// file cambia ad ogni aggiornamento, perciò lo leggiamo dinamicamente.
pub async fn fetch_default_cards_info(client: &reqwest::Client) -> Result<BulkInfo, String> {
    let resp = client
        .get(SCRYFALL_BULK_INDEX)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| format!("Errore di rete contattando Scryfall: {e}"))?;
    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Risposta di Scryfall non leggibile: {e}"))?;
    let data = json
        .get("data")
        .and_then(|d| d.as_array())
        .ok_or("Indice bulk di Scryfall non valido")?;
    let entry = data
        .iter()
        .find(|e| e.get("type").and_then(|t| t.as_str()) == Some("default_cards"))
        .ok_or("Dataset 'default_cards' non trovato su Scryfall")?;
    Ok(BulkInfo {
        download_uri: entry
            .get("download_uri")
            .and_then(|v| v.as_str())
            .ok_or("URL di download mancante")?
            .to_string(),
        size: entry.get("size").and_then(|v| v.as_u64()).unwrap_or(0),
        updated_at: entry
            .get("updated_at")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
    })
}

/// Chiede a Scryfall quante carte (stampe) sono attualmente disponibili su
/// Arena. È una richiesta piccolissima (pochi byte): la usiamo per capire se
/// sono uscite carte nuove senza scaricare l'intero file.
pub async fn fetch_arena_card_count(client: &reqwest::Client) -> Result<i64, String> {
    let resp = client
        .get(SCRYFALL_ARENA_COUNT)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| format!("Errore di rete contattando Scryfall: {e}"))?;
    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Risposta di Scryfall non leggibile: {e}"))?;
    json.get("total_cards")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| "Conteggio carte non disponibile".to_string())
}

/// Scarica un file su disco, riportando l'avanzamento (byte scaricati, byte totali).
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
        .map_err(|e| format!("Errore avviando il download: {e}"))?;
    let total = resp.content_length().unwrap_or(size_hint);
    let mut file =
        std::fs::File::create(dest).map_err(|e| format!("Impossibile creare il file: {e}"))?;
    let mut stream = resp.bytes_stream();
    let mut downloaded: u64 = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("Download interrotto: {e}"))?;
        file.write_all(&chunk)
            .map_err(|e| format!("Errore scrivendo su disco: {e}"))?;
        downloaded += chunk.len() as u64;
        on_progress(downloaded, total);
    }
    file.flush().map_err(|e| e.to_string())?;
    Ok(())
}

/// Legge a flusso un file bulk di Scryfall e restituisce solo le carte di
/// Arena in lingua inglese. Riporta quante carte ha esaminato finora.
pub fn parse_arena_cards<R: std::io::Read, F: FnMut(usize)>(
    reader: R,
    on_progress: F,
) -> serde_json::Result<Vec<Card>> {
    let mut de = serde_json::Deserializer::from_reader(reader);
    serde::Deserializer::deserialize_seq(&mut de, ArenaCardsVisitor { on_progress })
}

/// Visitor che scorre l'array JSON una carta alla volta, filtrando quelle di Arena.
struct ArenaCardsVisitor<F: FnMut(usize)> {
    on_progress: F,
}

impl<'de, F: FnMut(usize)> Visitor<'de> for ArenaCardsVisitor<F> {
    type Value = Vec<Card>;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("un array di carte Scryfall")
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
