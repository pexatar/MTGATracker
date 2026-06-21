//! Strutture dati delle carte: quelle "grezze" lette da Scryfall e quelle
//! "pulite" usate dal database e dall'interfaccia.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Indirizzi immagine forniti da Scryfall.
#[derive(Debug, Clone, Deserialize)]
pub struct ImageUris {
    pub small: Option<String>,
    pub normal: Option<String>,
}

/// Una "faccia" della carta (per le carte fronte-retro / a doppia faccia).
#[derive(Debug, Clone, Deserialize)]
pub struct CardFace {
    pub mana_cost: Option<String>,
    pub type_line: Option<String>,
    pub image_uris: Option<ImageUris>,
}

/// Carta come arriva dal file bulk di Scryfall. Contiene solo i campi che
/// ci servono: tutti gli altri vengono ignorati automaticamente da serde.
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
    /// `true` se la carta è giocabile su MTG Arena.
    pub fn is_on_arena(&self) -> bool {
        self.games.iter().any(|g| g == "arena")
    }

    /// Immagine "normale": dal livello carta, oppure dalla prima faccia.
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

    /// Immagine piccola (per le anteprime nelle liste).
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

    /// Costo di mana: dal livello carta, oppure unendo le facce.
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

    /// Linea di tipo: dal livello carta, oppure dalla prima faccia.
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

    /// Converte la carta grezza nel formato pulito usato dall'app.
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

/// Carta "pulita": questo è il formato salvato nel database e inviato all'interfaccia.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Card {
    pub id: String,
    pub oracle_id: Option<String>,
    pub name: String,
    pub set_code: String,
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

/// Stato del database delle carte, mostrato nell'interfaccia.
#[derive(Debug, Clone, Serialize)]
pub struct DatabaseStatus {
    pub card_count: i64,
    pub last_updated: Option<String>,
    pub source_updated_at: Option<String>,
}

/// Esito del controllo aggiornamenti: dice se ci sono carte nuove disponibili.
#[derive(Debug, Clone, Serialize)]
pub struct UpdateCheck {
    /// Conteggio salvato all'ultimo aggiornamento (metrica Scryfall).
    pub known_count: i64,
    /// Conteggio attuale su Scryfall (stessa metrica).
    pub available_count: i64,
    /// Quante carte nuove rispetto all'ultimo aggiornamento.
    pub new_cards: i64,
    /// `true` se conviene aggiornare (carte nuove o database vuoto).
    pub update_available: bool,
}
