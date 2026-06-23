//! Card data structures: the "raw" shape read from Scryfall and the "clean"
//! shape used by the database and the UI.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Image URLs provided by Scryfall.
#[derive(Debug, Clone, Deserialize)]
pub struct ImageUris {
    pub small: Option<String>,
    pub normal: Option<String>,
}

/// One face of a card (for double-faced / modal cards).
#[derive(Debug, Clone, Deserialize)]
pub struct CardFace {
    pub mana_cost: Option<String>,
    pub type_line: Option<String>,
    pub image_uris: Option<ImageUris>,
}

/// A card as it arrives from the Scryfall bulk file. It only declares the
/// fields we need: any other field is automatically ignored by serde.
#[derive(Debug, Clone, Deserialize)]
pub struct ScryfallCard {
    pub id: String,
    pub oracle_id: Option<String>,
    pub name: String,
    pub lang: String,
    pub set: String,
    pub collector_number: String,
    pub mana_cost: Option<String>,
    pub cmc: Option<f64>,
    pub type_line: Option<String>,
    pub colors: Option<Vec<String>>,
    pub color_identity: Option<Vec<String>>,
    pub rarity: String,
    pub layout: String,
    #[serde(default)]
    pub games: Vec<String>,
    pub arena_id: Option<i64>,
    pub image_uris: Option<ImageUris>,
    pub card_faces: Option<Vec<CardFace>>,
    #[serde(default)]
    pub legalities: HashMap<String, String>,
}

impl ScryfallCard {
    /// `true` if the card is playable on MTG Arena.
    pub fn is_on_arena(&self) -> bool {
        self.games.iter().any(|g| g == "arena")
    }

    /// "Normal" sized image: from the card level, otherwise from the first face.
    fn image_normal(&self) -> Option<String> {
        if let Some(img) = &self.image_uris {
            if img.normal.is_some() {
                return img.normal.clone();
            }
        }
        self.card_faces
            .as_ref()
            .and_then(|faces| faces.first())
            .and_then(|f| f.image_uris.as_ref())
            .and_then(|i| i.normal.clone())
    }

    /// Small image (used for list thumbnails).
    fn image_small(&self) -> Option<String> {
        if let Some(img) = &self.image_uris {
            if img.small.is_some() {
                return img.small.clone();
            }
        }
        self.card_faces
            .as_ref()
            .and_then(|faces| faces.first())
            .and_then(|f| f.image_uris.as_ref())
            .and_then(|i| i.small.clone())
    }

    /// Mana cost: from the card level, otherwise joining the faces.
    fn resolved_mana_cost(&self) -> Option<String> {
        match &self.mana_cost {
            Some(mc) if !mc.is_empty() => Some(mc.clone()),
            _ => self.card_faces.as_ref().map(|faces| {
                faces
                    .iter()
                    .filter_map(|f| f.mana_cost.clone())
                    .filter(|mc| !mc.is_empty())
                    .collect::<Vec<_>>()
                    .join(" // ")
            }),
        }
    }

    /// Type line: from the card level, otherwise from the first face.
    fn resolved_type_line(&self) -> Option<String> {
        match &self.type_line {
            Some(t) if !t.is_empty() => Some(t.clone()),
            _ => self
                .card_faces
                .as_ref()
                .and_then(|faces| faces.first())
                .and_then(|f| f.type_line.clone()),
        }
    }

    /// Converts the raw card into the clean shape used by the app.
    pub fn into_card(self) -> Card {
        let image_normal = self.image_normal();
        let image_small = self.image_small();
        let mana_cost = self.resolved_mana_cost();
        let type_line = self.resolved_type_line();
        Card {
            id: self.id,
            oracle_id: self.oracle_id,
            name: self.name,
            set_code: self.set,
            set_name: None,
            collector_number: self.collector_number,
            mana_cost,
            cmc: self.cmc.unwrap_or(0.0),
            type_line,
            colors: self.colors.unwrap_or_default(),
            color_identity: self.color_identity.unwrap_or_default(),
            rarity: self.rarity,
            layout: self.layout,
            arena_id: self.arena_id,
            image_small,
            image_normal,
            legalities: self.legalities,
        }
    }
}

/// Clean card: this is the shape stored in the database and sent to the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Card {
    pub id: String,
    pub oracle_id: Option<String>,
    pub name: String,
    pub set_code: String,
    /// Full human-readable set name (resolved from the sets table); may be absent.
    #[serde(default)]
    pub set_name: Option<String>,
    pub collector_number: String,
    pub mana_cost: Option<String>,
    pub cmc: f64,
    pub type_line: Option<String>,
    pub colors: Vec<String>,
    pub color_identity: Vec<String>,
    pub rarity: String,
    pub layout: String,
    pub arena_id: Option<i64>,
    pub image_small: Option<String>,
    pub image_normal: Option<String>,
    pub legalities: HashMap<String, String>,
}

/// State of the card database, shown in the UI.
#[derive(Debug, Clone, Serialize)]
pub struct DatabaseStatus {
    pub card_count: i64,
    pub last_updated: Option<String>,
    pub source_updated_at: Option<String>,
}

/// Summary of a saved deck, shown in the decks gallery.
#[derive(Debug, Clone, Serialize)]
pub struct DeckSummary {
    pub id: i64,
    pub name: String,
    pub updated_at: String,
    /// User-assigned Arena format (may be empty).
    pub format: String,
    /// Color identity letters in WUBRG order (e.g. "GR"); empty = colorless.
    pub colors: String,
    pub card_count: i64,
    /// Cover artwork (a representative card image), if any.
    pub cover_image: Option<String>,
    /// Matches won/lost with this deck (from log tracking).
    #[serde(default)]
    pub wins: i64,
    #[serde(default)]
    pub losses: i64,
}

/// A completed match parsed from the Arena log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchRecord {
    pub match_id: String,
    /// When the match was played (epoch milliseconds).
    pub played_at_ms: i64,
    /// Human-readable format derived from the Arena event id.
    pub format: String,
    pub event_id: String,
    pub opponent: String,
    /// "win", "loss" or "draw".
    pub result: String,
    pub games_won: i64,
    pub games_lost: i64,
    /// Arena card ids of the deck the local player used (for deck matching).
    #[serde(default)]
    pub deck_cards: Vec<i64>,
    /// Name of the deck used, resolved from the Arena log (may be empty).
    #[serde(default)]
    pub deck_name: String,
}

/// Player inventory summary read from the Arena log (the full card collection
/// is no longer available in the logs since Arena removed it in 2021).
#[derive(Debug, Clone, Serialize)]
pub struct Inventory {
    pub wc_common: i64,
    pub wc_uncommon: i64,
    pub wc_rare: i64,
    pub wc_mythic: i64,
    pub gold: i64,
    pub gems: i64,
    pub vault: i64,
}

/// Result of the update check: tells whether new cards are available.
#[derive(Debug, Clone, Serialize)]
pub struct UpdateCheck {
    /// Count saved at the last update (Scryfall metric).
    pub known_count: i64,
    /// Current count on Scryfall (same metric).
    pub available_count: i64,
    /// How many new cards compared to the last update.
    pub new_cards: i64,
    /// `true` if updating is worthwhile (new cards or empty database).
    pub update_available: bool,
}
