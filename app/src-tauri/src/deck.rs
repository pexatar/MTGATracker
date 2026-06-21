//! Parsing, matching and export of MTG Arena decklists.
//!
//! Arena's text format uses section headers on their own line
//! (`Commander`, `Companion`, `Deck`, `Sideboard`) followed by card lines like
//! `1 Omnath, Locus of Creation (ZNR) 232`. The set code and collector number
//! are optional (some exports only list quantity and name).

use crate::db;
use crate::models::Card;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

/// Which section of the deck a line belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Section {
    Commander,
    Companion,
    Main,
    Sideboard,
}

/// A single line of a decklist, with the card resolved against the database
/// when a match is found.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeckEntry {
    pub quantity: u32,
    pub name: String,
    pub set_code: Option<String>,
    pub collector_number: Option<String>,
    pub section: Section,
    /// The matching card from the local database, if any.
    pub card: Option<Card>,
    pub matched: bool,
}

/// A parsed (and resolved) decklist.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedDeck {
    pub entries: Vec<DeckEntry>,
    /// Total number of cards (sum of quantities).
    pub total_cards: u32,
    /// How many distinct lines could not be matched to a card.
    pub unmatched: u32,
}

/// Recognizes a section header line; returns `None` for normal card lines.
fn section_from_header(line: &str) -> Option<Section> {
    match line.trim().to_ascii_lowercase().as_str() {
        "commander" => Some(Section::Commander),
        "companion" => Some(Section::Companion),
        "deck" => Some(Section::Main),
        "sideboard" => Some(Section::Sideboard),
        _ => None,
    }
}

/// Parses a single card line into (quantity, name, set_code, collector_number).
/// Returns `None` if the line is not a valid card line.
fn parse_card_line(line: &str) -> Option<(u32, String, Option<String>, Option<String>)> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }

    // Leading quantity, then a space.
    let (qty_str, rest) = line.split_once(' ')?;
    let quantity: u32 = qty_str.parse().ok()?;
    let rest = rest.trim();
    if rest.is_empty() {
        return None;
    }

    // Optional trailing "(SET) Number": find the last "(...)" group.
    if let Some(close) = rest.rfind(')') {
        if let Some(open) = rest[..close].rfind('(') {
            let set_code = rest[open + 1..close].trim().to_string();
            let after = rest[close + 1..].trim();
            let name = rest[..open].trim().to_string();
            if !set_code.is_empty() && !after.is_empty() && !name.is_empty() {
                return Some((quantity, name, Some(set_code), Some(after.to_string())));
            }
        }
    }

    // Fallback: quantity + name only.
    Some((quantity, rest.to_string(), None, None))
}

/// Resolves a single entry against the database: first by set + collector
/// number, then by exact name.
fn resolve(conn: &Connection, entry: &mut DeckEntry) -> rusqlite::Result<()> {
    if let (Some(set), Some(num)) = (&entry.set_code, &entry.collector_number) {
        if let Some(card) = db::get_by_set_and_number(conn, set, num)? {
            entry.card = Some(card);
            entry.matched = true;
            return Ok(());
        }
    }
    if let Some(card) = db::get_by_exact_name(conn, &entry.name)? {
        entry.card = Some(card);
        entry.matched = true;
    }
    Ok(())
}

/// Parses a decklist and resolves every line against the local database.
pub fn parse_and_resolve(conn: &Connection, text: &str) -> rusqlite::Result<ParsedDeck> {
    // Lines before any explicit header are treated as the main deck.
    let mut current = Section::Main;
    let mut entries: Vec<DeckEntry> = Vec::new();

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(section) = section_from_header(line) {
            current = section;
            continue;
        }
        if let Some((quantity, name, set_code, collector_number)) = parse_card_line(line) {
            let mut entry = DeckEntry {
                quantity,
                name,
                set_code,
                collector_number,
                section: current,
                card: None,
                matched: false,
            };
            resolve(conn, &mut entry)?;
            entries.push(entry);
        }
    }

    let total_cards = entries.iter().map(|e| e.quantity).sum();
    let unmatched = entries.iter().filter(|e| !e.matched).count() as u32;
    Ok(ParsedDeck {
        entries,
        total_cards,
        unmatched,
    })
}

/// Rebuilds an Arena-compatible decklist from a parsed deck. When a line was
/// matched, the canonical name/set/number from the database are used.
pub fn export(deck: &ParsedDeck) -> String {
    let section_label = |s: Section| match s {
        Section::Commander => "Commander",
        Section::Companion => "Companion",
        Section::Main => "Deck",
        Section::Sideboard => "Sideboard",
    };

    let line_for = |e: &DeckEntry| -> String {
        // For Arena fidelity, echo the original set/number when the imported
        // line already had them (Arena's codes can differ from Scryfall's).
        // Only fall back to the matched card to complete name-only lines.
        let (name, set, num) = match (&e.set_code, &e.collector_number) {
            (Some(s), Some(n)) => (e.name.clone(), Some(s.clone()), Some(n.clone())),
            _ => match &e.card {
                Some(c) => (
                    c.name.clone(),
                    Some(c.set_code.clone()),
                    Some(c.collector_number.clone()),
                ),
                None => (e.name.clone(), None, None),
            },
        };
        match (set, num) {
            (Some(s), Some(n)) => format!("{} {} ({}) {}", e.quantity, name, s.to_uppercase(), n),
            _ => format!("{} {}", e.quantity, name),
        }
    };

    // Keep Arena's conventional section order.
    let order = [
        Section::Commander,
        Section::Companion,
        Section::Main,
        Section::Sideboard,
    ];

    let mut blocks: Vec<String> = Vec::new();
    for section in order {
        let lines: Vec<String> = deck
            .entries
            .iter()
            .filter(|e| e.section == section)
            .map(line_for)
            .collect();
        if lines.is_empty() {
            continue;
        }
        let mut block = String::new();
        block.push_str(section_label(section));
        block.push('\n');
        block.push_str(&lines.join("\n"));
        blocks.push(block);
    }

    blocks.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_and_name_only_lines() {
        assert_eq!(
            parse_card_line("1 Omnath, Locus of Creation (ZNR) 232").unwrap(),
            (
                1,
                "Omnath, Locus of Creation".to_string(),
                Some("ZNR".to_string()),
                Some("232".to_string())
            )
        );
        assert_eq!(
            parse_card_line("4 Llanowar Elves").unwrap(),
            (4, "Llanowar Elves".to_string(), None, None)
        );
        // Header words are not card lines.
        assert!(parse_card_line("Deck").is_none());
    }

    #[test]
    fn detects_section_headers() {
        assert_eq!(section_from_header("Commander"), Some(Section::Commander));
        assert_eq!(section_from_header("sideboard"), Some(Section::Sideboard));
        assert_eq!(section_from_header("1 Forest (MOM) 290"), None);
    }
}
