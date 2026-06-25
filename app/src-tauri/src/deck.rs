//! Parsing, matching and export of MTG Arena decklists.
//!
//! Arena's text format uses section headers on their own line
//! (`Commander`, `Companion`, `Deck`, `Sideboard`) followed by card lines like
//! `1 Omnath, Locus of Creation (ZNR) 232`. The set code and collector number
//! are optional (some exports only list quantity and name).

use crate::db;
use crate::models::{Card, MatchRecord};
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

/// A saved deck loaded back from the database (with its cards re-resolved).
#[derive(Debug, Clone, Serialize)]
pub struct LoadedDeck {
    pub id: i64,
    pub name: String,
    pub deck: ParsedDeck,
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

/// Resolves a single entry against the database.
///
/// Rebalanced (Alchemy) cards are exported by Arena with an "A-" prefix but
/// reuse the *original* card's set/number, so matching by set+number would pick
/// the wrong (non-rebalanced) printing with different legalities. For those we
/// match by name first. Otherwise: by set + collector number, then by name.
fn resolve(conn: &Connection, entry: &mut DeckEntry) -> rusqlite::Result<()> {
    let is_rebalanced = entry.name.starts_with("A-");

    if is_rebalanced {
        if let Some(card) = db::get_by_exact_name(conn, &entry.name)? {
            entry.card = Some(card);
            entry.matched = true;
            return Ok(());
        }
    }

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

// ---------------------------------------------------------------------------
// Deck analysis (statistics for charts)
// ---------------------------------------------------------------------------

/// A label/count pair (for color, type and rarity charts).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamedCount {
    pub label: String,
    pub count: u32,
}

/// One bucket of the mana curve (`cmc` 7 means "7 or more").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CurveBucket {
    pub cmc: u32,
    pub count: u32,
}

/// Cards that are not legal in a given format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormatLegality {
    pub format: String,
    pub illegal: Vec<String>,
}

/// Aggregated statistics of a deck, used to draw the charts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeckAnalysis {
    pub total_cards: u32,
    pub lands: u32,
    pub nonlands: u32,
    pub average_cmc: f64,
    pub mana_curve: Vec<CurveBucket>,
    pub color_pips: Vec<NamedCount>,
    pub type_distribution: Vec<NamedCount>,
    pub rarity_distribution: Vec<NamedCount>,
    pub format_legality: Vec<FormatLegality>,
}

/// Construction rules for an Arena format, so the model judges the deck against
/// the correct constraints (deck size, singleton, sideboard) instead of guessing
/// — the root cause of it mistaking a legal 60+15 Best-of-Three deck for "75
/// cards, reduce to 60". Keyed by the same canonical format strings used as
/// Scryfall legality keys elsewhere (no hidden hardcoded values).
fn format_rules(format: &str) -> &'static str {
    match format {
        "brawl" => "Brawl: 100 carte totali (1 comandante + 99). Formato singleton: \
            massimo 1 copia per carta, eccetto le terre base. Nessun sideboard.",
        "standardbrawl" => "Standard Brawl: 60 carte totali (1 comandante + 59). Formato \
            singleton: massimo 1 copia per carta, eccetto le terre base. Nessun sideboard.",
        _ => "Mazzo principale di almeno 60 carte, massimo 4 copie per carta (eccetto le \
            terre base). Sideboard opzionale di massimo 15 carte, usato solo nel Best-of-Three.",
    }
}

/// Builds a natural-language prompt describing the deck (real card list + real
/// statistics) for the local AI to produce a coaching analysis. The data comes
/// from the deck and the card database, so the model is grounded; it is also
/// explicitly asked not to invent cards.
///
/// `format` is the target Arena format (e.g. `standard`, `brawl`). The main deck
/// and the sideboard are listed in separate blocks, and the format's construction
/// rules are spelled out, so the model never conflates the sideboard with the main
/// deck nor judges legality against the wrong format.
/// `in_depth` picks the kind of analysis: `false` is a quick, practical summary
/// (experienced-player level), `true` a thorough technical breakdown that
/// correlates the data with the deck's archetype and format (expert level).
/// `matches` are the games tracked for this deck (empty for an unsaved deck);
/// the in-depth analysis factors them in and reflects on whether they suffice.
pub fn analysis_prompt(
    deck: &ParsedDeck,
    analysis: &DeckAnalysis,
    format: &str,
    in_depth: bool,
    matches: &[MatchRecord],
) -> String {
    let mut p = String::new();
    p.push_str("Analizza il mazzo di Magic: The Gathering Arena seguente.\n\n");

    let line_for = |e: &DeckEntry| -> String {
        let detail = e
            .card
            .as_ref()
            .map(|c| format!(" — {} (costo {})", c.type_line.as_deref().unwrap_or("?"), c.cmc))
            .unwrap_or_default();
        format!("{}x {}{}\n", e.quantity, e.name, detail)
    };

    let sideboard: Vec<&DeckEntry> = deck
        .entries
        .iter()
        .filter(|e| e.section == Section::Sideboard)
        .collect();
    // A non-empty sideboard implies Best-of-Three; Brawl formats have none (BO1).
    let bo3 = !sideboard.is_empty();

    p.push_str(&format!("FORMATO: {format}\n"));
    p.push_str(&format!("REGOLE DI COSTRUZIONE — {}\n", format_rules(format)));
    p.push_str(&format!(
        "MODALITÀ: {}\n\n",
        if bo3 {
            "Best-of-Three (BO3). Il mazzo ha un sideboard di carte di scambio: è \
             perfettamente legale e SEPARATO dal mazzo principale. NON sommare le carte del \
             sideboard a quelle del mazzo principale, e non chiedere di rimuoverle."
        } else {
            "Best-of-One (BO1). Nessun sideboard."
        }
    ));

    // Main deck = everything actually played (commander, companion, deck), listed
    // separately from the sideboard so the model never conflates the two.
    p.push_str("MAZZO PRINCIPALE:\n");
    for e in deck.entries.iter().filter(|e| e.section != Section::Sideboard) {
        p.push_str(&line_for(e));
    }
    if bo3 {
        p.push_str("\nSIDEBOARD (carte di scambio, NON parte del mazzo principale):\n");
        for e in &sideboard {
            p.push_str(&line_for(e));
        }
    }

    p.push_str("\nSTATISTICHE (solo mazzo principale):\n");
    p.push_str(&format!(
        "- Totale carte: {} (terre: {}, non-terre: {})\n",
        analysis.total_cards, analysis.lands, analysis.nonlands
    ));
    p.push_str(&format!("- Costo medio (non-terre): {:.2}\n", analysis.average_cmc));
    let join = |items: &[NamedCount]| {
        items
            .iter()
            .map(|n| format!("{} {}", n.label, n.count))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let curve = analysis
        .mana_curve
        .iter()
        .map(|b| format!("{}:{}", b.cmc, b.count))
        .collect::<Vec<_>>()
        .join(", ");
    p.push_str(&format!("- Curva di mana (costo:numero): {curve}\n"));
    if !analysis.color_pips.is_empty() {
        p.push_str(&format!("- Simboli colore: {}\n", join(&analysis.color_pips)));
    }
    if !analysis.type_distribution.is_empty() {
        p.push_str(&format!("- Tipi: {}\n", join(&analysis.type_distribution)));
    }
    if !analysis.rarity_distribution.is_empty() {
        p.push_str(&format!("- Rarità: {}\n", join(&analysis.rarity_distribution)));
    }
    // Legality is reported only for the target format (judging the deck against
    // the other six would be noise and could trigger irrelevant warnings).
    match analysis.format_legality.iter().find(|fl| fl.format == format) {
        Some(fl) if !fl.illegal.is_empty() => {
            p.push_str(&format!("- Carte NON legali in {format}: {}\n", fl.illegal.join(", ")));
        }
        _ => p.push_str(&format!("- Tutte le carte sono legali in {format}.\n")),
    }

    // The deck's tracked games feed the in-depth analysis (its step 5 weighs the
    // record and whether the sample is large enough to draw conclusions from).
    if in_depth && !matches.is_empty() {
        let wins = matches.iter().filter(|m| m.result == "win").count();
        let losses = matches.iter().filter(|m| m.result == "loss").count();
        p.push_str(&format!(
            "\nPARTITE TRACCIATE CON QUESTO MAZZO: {} (vittorie {}, sconfitte {}).\n",
            matches.len(),
            wins,
            losses
        ));
        for m in matches.iter().take(25) {
            p.push_str(&format!(
                "- vs {}: {} ({}-{})\n",
                m.opponent, m.result, m.games_won, m.games_lost
            ));
        }
    }

    // The two modes differ in substance, not just speed/length: In-depth is a
    // rigorous, data-correlating expert breakdown; Fast a crisp discursive read.
    // (The caller also enables/disables the model's reasoning to match.)
    if in_depth {
        p.push_str(
            "\nSei un pool di coach esperti di Magic: The Gathering Arena che ragiona insieme \
             adottando più punti di vista (aggro, control, combo, analista del meta) e consegna \
             UNA sola analisi coerente e integrata, non quattro analisi separate. Rivolgiti al \
             giocatore dando del \"tu\" (seconda persona singolare). Niente preamboli né frasi di \
             cortesia: entra subito nel merito, in italiano. Sii onesto e critico, mai \
             compiacente.\n\
             Obiettivo: un'analisi tecnica, dettagliata, metodica e rigorosa che CORRELA tra loro \
             tutti i dati forniti e va ben oltre un semplice elenco di pro e contro. Cita SEMPRE \
             carte concrete del mazzo a sostegno di ogni affermazione; mantieni i nomi delle carte \
             in inglese.\n\
             Grounding: non inventare carte inesistenti e non attribuire al mazzo carte non \
             elencate; puoi e devi però usare la tua conoscenza del gioco, dei formati e del meta \
             per ragionare su strategie e matchup.\n\
             Metodo (usalo come guida, poi SINTETIZZA in un testo scorrevole e ben strutturato, \
             senza trasformarlo in una checklist meccanica): 1) Identità e piano di gioco \
             (archetipo), da carte, curva e colori; 2) Coerenza interna: curva, base di mana e \
             colori sostengono il piano? Ruoli (minacce/risposte/card advantage/ramp) bilanciati?; \
             3) Sinergie e carte chiave: motori, combo, dipendenze, gestione delle copie singole; \
             4) Matchup: anticipa quali strategie/archetipi mettono in difficoltà il mazzo e quali \
             favoriscono, e il ruolo del sideboard in BO3; 5) Dati empirici: usa il record delle \
             partite se presente e rifletti esplicitamente se è SUFFICIENTE per conclusioni \
             affidabili (segnala campioni piccoli o informazioni mancanti, es. i colori avversari); \
             6) Come pilotarlo: linee di gioco, sequencing, come valorizzare i punti di forza; \
             7) Migliorie concrete e prioritizzate: indica cosa serve per ruolo/effetto (es. \"più \
             rimozione a basso costo\"); se nomini carte, solo esempi noti e legali nel formato.\n\
             Chiudi ricordando in una riga che la modalità Fast offre una lettura più rapida e \
             sintetica.\n",
        );
    } else {
        p.push_str(
            "\nSei un giocatore esperto di Magic: The Gathering Arena. Niente preamboli: scrivi in \
             italiano una valutazione breve e discorsiva (2-3 paragrafi brevi, ~120-180 parole; \
             prosa scorrevole, non un elenco freddo) che evidenzi con chiarezza cosa funziona, cosa \
             non funziona e cosa migliorare, con un paio di consigli pratici. Cita 2-3 carte \
             concrete del mazzo a sostegno; mantieni i nomi delle carte in inglese. Sii onesto, mai \
             compiacente. Non inventare carte inesistenti (ma puoi usare la tua conoscenza del \
             meta). Chiudi ricordando in una riga che la modalità In-depth offre un'analisi tecnica \
             molto più approfondita.\n",
        );
    }
    p
}

/// Classifies a card into a single primary type category (priority order, so
/// e.g. "Artifact Creature" counts as a Creature).
fn type_bucket(type_line: &str) -> &'static str {
    const ORDER: [&str; 8] = [
        "Land",
        "Creature",
        "Planeswalker",
        "Battle",
        "Instant",
        "Sorcery",
        "Artifact",
        "Enchantment",
    ];
    for t in ORDER {
        if type_line.contains(t) {
            return t;
        }
    }
    "Other"
}

/// Computes aggregated statistics for the matched cards of a deck.
pub fn analyze(deck: &ParsedDeck) -> DeckAnalysis {
    use std::collections::BTreeMap;

    const COLORS: [char; 5] = ['W', 'U', 'B', 'R', 'G'];
    const TYPE_ORDER: [&str; 9] = [
        "Creature",
        "Instant",
        "Sorcery",
        "Artifact",
        "Enchantment",
        "Planeswalker",
        "Battle",
        "Land",
        "Other",
    ];
    const RARITY_ORDER: [&str; 4] = ["common", "uncommon", "rare", "mythic"];
    // The constructed formats that actually exist on MTG Arena (Scryfall keys),
    // in display order. Other paper-only formats are intentionally hidden.
    const ARENA_FORMATS: [&str; 7] = [
        "standard",
        "alchemy",
        "pioneer",
        "historic",
        "timeless",
        "brawl",
        "standardbrawl",
    ];

    let mut lands: u32 = 0;
    let mut nonlands: u32 = 0;
    let mut cmc_sum: f64 = 0.0;
    let mut curve = [0u32; 8];
    let mut pips: BTreeMap<char, u32> = BTreeMap::new();
    let mut types: BTreeMap<&str, u32> = BTreeMap::new();
    let mut rarities: BTreeMap<String, u32> = BTreeMap::new();
    let mut legality: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for entry in &deck.entries {
        let Some(card) = &entry.card else { continue };
        let q = entry.quantity;
        let type_line = card.type_line.as_deref().unwrap_or("");

        // Legality is judged over the WHOLE deck: in Best-of-Three the sideboard
        // cards must be legal too, so they are checked here even though they are
        // excluded from the curve/colors/counts below.
        for (format, status) in &card.legalities {
            // Every format gets an entry (so fully-legal formats appear too);
            // "legal" and "restricted" are playable, anything else is not.
            let illegal = legality.entry(format.clone()).or_default();
            if status != "legal" && status != "restricted" && !illegal.contains(&card.name) {
                illegal.push(card.name.clone());
            }
        }

        // The remaining statistics describe the deck you actually play, so the
        // sideboard is excluded — it must not skew the curve, colors or counts.
        if entry.section == Section::Sideboard {
            continue;
        }

        let is_land = type_line.contains("Land");
        if is_land {
            lands += q;
        } else {
            nonlands += q;
            cmc_sum += card.cmc * q as f64;
            let bucket = (card.cmc.round() as i64).clamp(0, 7) as usize;
            curve[bucket] += q;
            if let Some(cost) = &card.mana_cost {
                for color in COLORS {
                    let n = cost.matches(color).count() as u32;
                    if n > 0 {
                        *pips.entry(color).or_insert(0) += n * q;
                    }
                }
            }
        }

        *types.entry(type_bucket(type_line)).or_insert(0) += q;
        *rarities.entry(card.rarity.clone()).or_insert(0) += q;
    }

    let mana_curve = (0..=7)
        .map(|cmc| CurveBucket {
            cmc,
            count: curve[cmc as usize],
        })
        .collect();

    let color_pips = COLORS
        .iter()
        .filter_map(|c| {
            pips.get(c).map(|&count| NamedCount {
                label: c.to_string(),
                count,
            })
        })
        .collect();

    let type_distribution = TYPE_ORDER
        .iter()
        .filter_map(|t| {
            types.get(*t).map(|&count| NamedCount {
                label: (*t).to_string(),
                count,
            })
        })
        .collect();

    // Known rarities first (in a sensible order), then any extra alphabetically.
    let mut rarity_distribution: Vec<NamedCount> = Vec::new();
    for r in RARITY_ORDER {
        if let Some(&count) = rarities.get(r) {
            rarity_distribution.push(NamedCount {
                label: r.to_string(),
                count,
            });
        }
    }
    for (label, &count) in &rarities {
        if !RARITY_ORDER.contains(&label.as_str()) {
            rarity_distribution.push(NamedCount {
                label: label.clone(),
                count,
            });
        }
    }

    // Keep only Arena's formats, in display order.
    let format_legality = ARENA_FORMATS
        .iter()
        .filter_map(|f| {
            legality.get(*f).map(|illegal| FormatLegality {
                format: (*f).to_string(),
                illegal: illegal.clone(),
            })
        })
        .collect();

    let average_cmc = if nonlands > 0 {
        (cmc_sum / nonlands as f64 * 100.0).round() / 100.0
    } else {
        0.0
    };

    DeckAnalysis {
        total_cards: lands + nonlands,
        lands,
        nonlands,
        average_cmc,
        mana_curve,
        color_pips,
        type_distribution,
        rarity_distribution,
        format_legality,
    }
}

/// Computes gallery metadata for a deck: total cards, color-identity string
/// (WUBRG order) and a cover image (commander's, else a non-land card's).
pub fn summary_metadata(deck: &ParsedDeck) -> (i64, String, Option<String>) {
    const ORDER: [&str; 5] = ["W", "U", "B", "R", "G"];

    let card_count: i64 = deck.entries.iter().map(|e| e.quantity as i64).sum();

    let mut present = [false; 5];
    for entry in &deck.entries {
        if let Some(card) = &entry.card {
            for ci in &card.color_identity {
                if let Some(pos) = ORDER.iter().position(|x| *x == ci) {
                    present[pos] = true;
                }
            }
        }
    }
    let colors: String = ORDER
        .iter()
        .enumerate()
        .filter(|(i, _)| present[*i])
        .map(|(_, c)| *c)
        .collect();

    (card_count, colors, pick_cover(deck))
}

/// Picks a representative artwork: the commander, then a non-land card, then any.
fn pick_cover(deck: &ParsedDeck) -> Option<String> {
    let image = |e: &DeckEntry| e.card.as_ref().and_then(|c| c.image_normal.clone());
    let is_land = |e: &DeckEntry| {
        e.card
            .as_ref()
            .and_then(|c| c.type_line.as_deref())
            .map_or(false, |t| t.contains("Land"))
    };

    if let Some(img) = deck
        .entries
        .iter()
        .find(|e| e.section == Section::Commander)
        .and_then(image)
    {
        return Some(img);
    }
    if let Some(img) = deck.entries.iter().filter(|e| !is_land(e)).find_map(image) {
        return Some(img);
    }
    deck.entries.iter().find_map(image)
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

    fn entry(
        quantity: u32,
        name: &str,
        cmc: f64,
        type_line: &str,
        mana_cost: Option<&str>,
        rarity: &str,
        legalities: &[(&str, &str)],
    ) -> DeckEntry {
        let card = Card {
            id: name.to_string(),
            oracle_id: None,
            name: name.to_string(),
            set_code: "tst".to_string(),
            set_name: None,
            collector_number: "1".to_string(),
            mana_cost: mana_cost.map(|s| s.to_string()),
            cmc,
            type_line: Some(type_line.to_string()),
            colors: vec![],
            color_identity: vec![],
            rarity: rarity.to_string(),
            layout: "normal".to_string(),
            arena_id: None,
            image_small: None,
            image_normal: None,
            legalities: legalities
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        };
        DeckEntry {
            quantity,
            name: name.to_string(),
            set_code: Some("TST".to_string()),
            collector_number: Some("1".to_string()),
            section: Section::Main,
            card: Some(card),
            matched: true,
        }
    }

    #[test]
    fn analyzes_curve_colors_types_and_legality() {
        let deck = ParsedDeck {
            entries: vec![
                entry(4, "Llanowar Elves", 1.0, "Creature — Elf Druid", Some("{G}"), "common", &[("standard", "not_legal"), ("brawl", "legal")]),
                entry(1, "Big Threat", 6.0, "Creature — Dragon", Some("{4}{R}{R}"), "mythic", &[("standard", "legal")]),
                entry(9, "Forest", 0.0, "Basic Land — Forest", None, "common", &[("standard", "legal")]),
            ],
            total_cards: 14,
            unmatched: 0,
        };

        let a = analyze(&deck);
        assert_eq!(a.lands, 9);
        assert_eq!(a.nonlands, 5);
        // average cmc of nonlands: (1*4 + 6*1) / 5 = 2.0
        assert!((a.average_cmc - 2.0).abs() < 1e-9);
        // curve: 4 at cmc 1, 1 at cmc 6
        assert_eq!(a.mana_curve.iter().find(|b| b.cmc == 1).unwrap().count, 4);
        assert_eq!(a.mana_curve.iter().find(|b| b.cmc == 6).unwrap().count, 1);
        // pips: G = 4 (from 4x {G}), R = 2 (from 1x {R}{R})
        assert_eq!(a.color_pips.iter().find(|c| c.label == "G").unwrap().count, 4);
        assert_eq!(a.color_pips.iter().find(|c| c.label == "R").unwrap().count, 2);
        // types: Creature 5, Land 9
        assert_eq!(a.type_distribution.iter().find(|t| t.label == "Creature").unwrap().count, 5);
        assert_eq!(a.type_distribution.iter().find(|t| t.label == "Land").unwrap().count, 9);
        // rarity order: common before mythic
        assert_eq!(a.rarity_distribution[0].label, "common");
        // legality: Llanowar Elves is not legal in standard
        let std = a.format_legality.iter().find(|f| f.format == "standard").unwrap();
        assert_eq!(std.illegal, vec!["Llanowar Elves".to_string()]);
    }

    #[test]
    fn analyze_excludes_sideboard_from_stats_but_not_from_legality() {
        let mut side = entry(
            3,
            "Sideboard Bomb",
            5.0,
            "Creature — Demon",
            Some("{3}{B}{B}"),
            "rare",
            &[("standard", "not_legal")],
        );
        side.section = Section::Sideboard;

        let deck = ParsedDeck {
            entries: vec![
                entry(2, "Main Creature", 2.0, "Creature — Soldier", Some("{1}{W}"), "common", &[("standard", "legal")]),
                side,
                entry(1, "Plains", 0.0, "Basic Land — Plains", None, "common", &[("standard", "legal")]),
            ],
            total_cards: 6,
            unmatched: 0,
        };

        let a = analyze(&deck);
        // Stats cover the main deck only: 2 nonlands + 1 land = 3 (the 3 sideboard
        // cards are excluded), so it must NOT report 6 total cards.
        assert_eq!(a.total_cards, 3);
        assert_eq!(a.lands, 1);
        assert_eq!(a.nonlands, 2);
        // The sideboard creature must not appear in the mana curve.
        assert_eq!(a.mana_curve.iter().find(|b| b.cmc == 5).unwrap().count, 0);
        // But legality is still judged over the whole deck, sideboard included.
        let std = a.format_legality.iter().find(|f| f.format == "standard").unwrap();
        assert_eq!(std.illegal, vec!["Sideboard Bomb".to_string()]);
    }
}
