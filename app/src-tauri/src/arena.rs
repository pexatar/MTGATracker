//! Parsing of the MTG Arena `Player.log` to extract completed matches.
//!
//! The relevant event is `MatchGameRoomStateChangedEvent` with state
//! `MatchCompleted`: it carries both players (`reservedPlayers`, each with a
//! `teamId`) and the winner (`finalMatchResult.resultList`). The line just
//! above it (`Match to <opponentUserId>: ...`) tells us which player is the
//! opponent, so we know which side is the local user.

use crate::models::MatchRecord;
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Deserialize)]
struct Wrapper {
    timestamp: Option<String>,
    #[serde(rename = "matchGameRoomStateChangedEvent")]
    event: Option<RoomEvent>,
}

#[derive(Deserialize)]
struct RoomEvent {
    #[serde(rename = "gameRoomInfo")]
    game_room_info: RoomInfo,
}

#[derive(Deserialize)]
struct RoomInfo {
    #[serde(rename = "gameRoomConfig")]
    config: RoomConfig,
    #[serde(rename = "stateType")]
    state_type: String,
    #[serde(rename = "finalMatchResult")]
    final_result: Option<FinalResult>,
}

#[derive(Deserialize)]
struct RoomConfig {
    #[serde(rename = "reservedPlayers")]
    players: Vec<LogPlayer>,
    #[serde(rename = "matchId")]
    match_id: String,
}

#[derive(Deserialize)]
struct LogPlayer {
    #[serde(rename = "userId")]
    user_id: String,
    #[serde(rename = "playerName")]
    player_name: Option<String>,
    #[serde(rename = "teamId")]
    team_id: i64,
    #[serde(rename = "eventId")]
    event_id: Option<String>,
}

#[derive(Deserialize)]
struct FinalResult {
    #[serde(rename = "resultList")]
    results: Vec<GameResult>,
}

#[derive(Deserialize)]
struct GameResult {
    scope: String,
    #[serde(rename = "winningTeamId")]
    winning_team_id: Option<i64>,
}

/// Default Arena log paths (current + previous session) under the user profile.
pub fn default_log_paths() -> Vec<PathBuf> {
    if let Some(home) = dirs::home_dir() {
        let base = home.join("AppData/LocalLow/Wizards Of The Coast/MTGA");
        return vec![base.join("Player-prev.log"), base.join("Player.log")];
    }
    Vec::new()
}

/// Maps an Arena `eventId` to a readable format name (best effort).
fn format_from_event_id(event_id: &str) -> String {
    let e = event_id.to_lowercase();
    let label = if e.contains("standardbrawl") || e.contains("standard_brawl") {
        "Standard Brawl"
    } else if e.contains("brawl") {
        "Brawl"
    } else if e.contains("alchemy") {
        "Alchemy"
    } else if e.contains("historic") {
        "Historic"
    } else if e.contains("timeless") {
        "Timeless"
    } else if e.contains("pioneer") || e.contains("explorer") {
        "Pioneer"
    } else if e.contains("standard") || e.contains("ladder") {
        "Standard"
    } else if e.contains("draft") || e.contains("sealed") || e.contains("limited") {
        "Limited"
    } else {
        return event_id.to_string();
    };
    label.to_string()
}

/// Finds the local player's screen name (logged at login as "screenName").
fn find_local_name(log: &str) -> Option<String> {
    let key = "\"screenName\"";
    let idx = log.find(key)?;
    let rest = &log[idx + key.len()..];
    let colon = rest.find(':')?;
    let after = &rest[colon + 1..];
    let q1 = after.find('"')?;
    let inner = &after[q1 + 1..];
    let q2 = inner.find('"')?;
    Some(inner[..q2].to_string())
}

/// Extracts the integer array that follows `"deckCards":` on a line, if any.
fn extract_deck_cards(line: &str) -> Option<Vec<i64>> {
    let key = "\"deckCards\"";
    let idx = line.find(key)?;
    let after = &line[idx + key.len()..];
    let open = after.find('[')?;
    let close = after[open..].find(']')? + open;
    let cards = after[open + 1..close]
        .split(',')
        .filter_map(|s| s.trim().parse::<i64>().ok())
        .collect::<Vec<_>>();
    if cards.is_empty() {
        None
    } else {
        Some(cards)
    }
}

/// Parses the whole log text and returns the completed matches it contains.
pub fn parse_matches(log: &str) -> Vec<MatchRecord> {
    let mut out = Vec::new();
    // The "Match to <userId>" marker line identifies the LOCAL user.
    let mut pending_local_id: Option<String> = None;
    let mut pending_deck: Vec<i64> = Vec::new();
    let local_name = find_local_name(log);

    for line in log.lines() {
        if line.contains("\"deckCards\"") {
            if let Some(cards) = extract_deck_cards(line) {
                pending_deck = cards;
            }
        }

        if let Some(idx) = line.find("Match to ") {
            let rest = &line[idx + "Match to ".len()..];
            if let Some(end) = rest.find(": MatchGameRoomStateChangedEvent") {
                pending_local_id = Some(rest[..end].trim().to_string());
                continue;
            }
        }

        let trimmed = line.trim_start();
        if !trimmed.starts_with('{') || !trimmed.contains("\"matchGameRoomStateChangedEvent\"") {
            continue;
        }

        let Ok(wrapper) = serde_json::from_str::<Wrapper>(trimmed) else {
            continue;
        };
        let Some(event) = wrapper.event else { continue };
        let info = event.game_room_info;
        if !info.state_type.contains("MatchCompleted") {
            continue;
        }
        let Some(final_result) = info.final_result else {
            continue;
        };

        if let Some(record) = build_record(
            &info.config,
            &final_result,
            wrapper.timestamp.as_deref(),
            local_name.as_deref(),
            pending_local_id.as_deref(),
            std::mem::take(&mut pending_deck),
        ) {
            out.push(record);
        }
        pending_local_id = None;
    }

    // Label each match with the deck name, matching its cards to a named deck.
    let named = parse_named_decks(log);
    for m in &mut out {
        if m.deck_cards.is_empty() {
            continue;
        }
        let mset: std::collections::HashSet<i64> = m.deck_cards.iter().copied().collect();
        let mut best: Option<(&str, f64)> = None;
        for (name, ids) in &named {
            let inter = mset.iter().filter(|id| ids.contains(*id)).count();
            let ratio = inter as f64 / mset.len() as f64;
            if best.map_or(true, |(_, b)| ratio > b) {
                best = Some((name, ratio));
            }
        }
        if let Some((name, ratio)) = best {
            if ratio >= 0.6 {
                m.deck_name = name.to_string();
            }
        }
    }

    out
}

fn build_record(
    config: &RoomConfig,
    final_result: &FinalResult,
    timestamp: Option<&str>,
    local_name: Option<&str>,
    local_id: Option<&str>,
    deck_cards: Vec<i64>,
) -> Option<MatchRecord> {
    // Identify the local player by screen name, falling back to the
    // "Match to <userId>" marker (which points at the local user).
    let me = local_name
        .and_then(|n| config.players.iter().find(|p| p.player_name.as_deref() == Some(n)))
        .or_else(|| local_id.and_then(|id| config.players.iter().find(|p| p.user_id == id)))
        .or_else(|| config.players.first())?;
    let opponent = config.players.iter().find(|p| p.user_id != me.user_id)?;
    let my_team = me.team_id;

    let match_winner = final_result
        .results
        .iter()
        .find(|r| r.scope == "MatchScope_Match")
        .and_then(|r| r.winning_team_id);

    let result = match match_winner {
        Some(t) if t == my_team => "win",
        Some(_) => "loss",
        None => "draw",
    }
    .to_string();

    let mut games_won = 0;
    let mut games_lost = 0;
    for r in &final_result.results {
        if r.scope != "MatchScope_Game" {
            continue;
        }
        match r.winning_team_id {
            Some(t) if t == my_team => games_won += 1,
            Some(_) => games_lost += 1,
            None => {}
        }
    }

    Some(MatchRecord {
        match_id: config.match_id.clone(),
        played_at_ms: timestamp.and_then(|t| t.parse::<i64>().ok()).unwrap_or(0),
        format: me
            .event_id
            .as_deref()
            .map(format_from_event_id)
            .unwrap_or_default(),
        event_id: me.event_id.clone().unwrap_or_default(),
        opponent: opponent
            .player_name
            .clone()
            .unwrap_or_else(|| "Unknown".to_string()),
        result,
        games_won,
        games_lost,
        deck_cards,
        deck_name: String::new(),
    })
}

/// Parses the named decks (Name + Arena card ids) listed in the log, so a
/// match can be labelled with the deck the player actually used.
fn parse_named_decks(log: &str) -> Vec<(String, std::collections::HashSet<i64>)> {
    let mut decks = Vec::new();
    let mut pos = 0;
    while let Some(rel) = log[pos..].find("\"CourseDeckSummary\"") {
        let start = pos + rel;
        pos = start + "\"CourseDeckSummary\"".len();
        let tail = &log[start..];
        let name = read_string_after(tail, "\"Name\":\"");
        if let Some(md) = tail.find("\"MainDeck\":[") {
            let block_start = md + "\"MainDeck\":[".len();
            if let Some(end) = tail[block_start..].find(']') {
                let ids = extract_card_ids(&tail[block_start..block_start + end]);
                if let Some(n) = name {
                    if !n.is_empty() && !ids.is_empty() {
                        decks.push((n, ids));
                    }
                }
            }
        }
    }
    decks
}

fn read_string_after(s: &str, key: &str) -> Option<String> {
    let i = s.find(key)?;
    let after = &s[i + key.len()..];
    let end = after.find('"')?;
    Some(after[..end].to_string())
}

fn extract_card_ids(block: &str) -> std::collections::HashSet<i64> {
    let mut set = std::collections::HashSet::new();
    let key = "\"cardId\":";
    let mut p = 0;
    while let Some(i) = block[p..].find(key) {
        let after = &block[p + i + key.len()..];
        let num: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(n) = num.parse::<i64>() {
            set.insert(n);
        }
        p += i + key.len();
    }
    set
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_completed_match() {
        // Real shape from the Arena log (names anonymized). The local user is
        // "Me"; the "Match to ME999" marker points at the local user.
        let log = r#"[Accounts] "screenName":"Me"
[UnityCrossThreadLogger]23/06/2026 02:55:59: Match to ME999: MatchGameRoomStateChangedEvent
{ "timestamp": "1782176169741", "matchGameRoomStateChangedEvent": { "gameRoomInfo": { "gameRoomConfig": { "reservedPlayers": [ { "userId": "OPP123", "playerName":"Rival", "systemSeatId": 1, "teamId": 1, "eventId": "Play_Brawl_Historic" }, { "userId": "ME999", "playerName":"Me", "systemSeatId": 2, "teamId": 2, "eventId": "Play_Brawl_Historic" } ], "matchId": "match-abc" }, "stateType": "MatchGameRoomStateType_MatchCompleted", "finalMatchResult": { "matchId": "match-abc", "resultList": [ { "scope": "MatchScope_Game", "result": "ResultType_WinLoss", "winningTeamId": 2 }, { "scope": "MatchScope_Match", "result": "ResultType_WinLoss", "winningTeamId": 2 } ] } } } }
[UnityCrossThreadLogger]STATE CHANGED"#;

        let matches = parse_matches(log);
        assert_eq!(matches.len(), 1);
        let m = &matches[0];
        assert_eq!(m.match_id, "match-abc");
        assert_eq!(m.opponent, "Rival");
        assert_eq!(m.format, "Brawl");
        assert_eq!(m.result, "win"); // local user is team 2, winner is team 2
        assert_eq!(m.games_won, 1);
        assert_eq!(m.games_lost, 0);
        assert_eq!(m.played_at_ms, 1782176169741);
    }

    #[test]
    fn loss_when_opponent_wins() {
        let log = r#""screenName":"Me"
Match to ME999: MatchGameRoomStateChangedEvent
{ "timestamp": "1", "matchGameRoomStateChangedEvent": { "gameRoomInfo": { "gameRoomConfig": { "reservedPlayers": [ { "userId": "OPP123", "playerName":"Rival", "teamId": 1, "eventId": "Ladder" }, { "userId": "ME999", "playerName":"Me", "teamId": 2, "eventId": "Ladder" } ], "matchId": "m2" }, "stateType": "MatchGameRoomStateType_MatchCompleted", "finalMatchResult": { "resultList": [ { "scope": "MatchScope_Match", "winningTeamId": 1 } ] } } } }"#;
        let matches = parse_matches(log);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].opponent, "Rival");
        assert_eq!(matches[0].result, "loss"); // local is team 2, winner is team 1
        assert_eq!(matches[0].format, "Standard");
    }
}
