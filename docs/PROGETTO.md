# MTG Arena Tracker — Progetto completo

> Documento di progetto strutturato. Versione iniziale: 2026-06-21.
> Tutte le funzionalità sono progettate dall'inizio; la realizzazione avverrà a passi piccoli e verificabili.

---

## 1. Obiettivo
Applicazione **Windows 11**, **locale** e **portable** (nessuna installazione, eseguibile da cartella/USB) per
**Magic: The Gathering Arena**, con focus sul formato **Brawl** ma valida per tutti i formati.
Sostituisce e amplia ciò che oggi l'utente fa con Untapped.gg, aggiungendo un **editor di mazzi** e un'**IA di analisi**.

## 2. Funzionalità (tutte previste dall'inizio)

### 2.1 Import mazzi
- Lettura dei file di testo esportati da Arena (sezioni `Commander` / `Deck` / `Sideboard`).
- Parser robusto del formato `N Nome Carta (SET) Numero` (gestione di tutte le sezioni e righe vuote).
- Import anche da incolla-testo, non solo da file.

### 2.2 Export mazzi
- Esportazione nel formato testo compatibile con Arena (nomi in inglese, set, numero).
- Copia negli appunti con un click ("Copy to Arena", come Untapped).

### 2.3 Editor mazzi in app
- Creazione/modifica mazzi con ricerca carte (per nome, colore, tipo, set, costo…).
- Immagini delle carte, conteggi, gestione Commander/Companion/Sideboard.
- Salvataggio locale dei mazzi nel database dell'app.

### 2.4 Analisi e statistiche del mazzo (grafici)
- **Curva di mana** (distribuzione del costo convertito).
- **Ripartizione colori** (pip di mana e per carta).
- **Tipi di carta** (creature, istantanei, stregonerie, terre, ecc.).
- **Rarità**, **conteggio terre/non-terre**, fonti di mana per colore.
- Indicatori di coerenza (es. fonti di colore vs costi richiesti).
- **Validazione legalità** per formato (Brawl, Standard, Historic, …) con segnalazione carte illegali/bannate.

### 2.5 Tracking automatico delle partite (come Untapped)
- Lettura in tempo reale del log di Arena (`Player.log`) tramite "watcher".
- Cronologia partite con record W-L, mazzo usato, avversario, esito, durata.
- **Win rate** per mazzo e nel tempo; **matchup** per archetipo/colori avversari.
- Statistiche generali (a livello profilo) replicando le viste di Untapped degli screenshot.

### 2.6 Collezione e wildcard
- Lettura della collezione posseduta dai log di Arena.
- Per ogni mazzo: carte mancanti e **wildcard necessarie** (per rarità) per completarlo.

### 2.7 IA di analisi (configurabile)
- Provider selezionabile nelle impostazioni:
  - **Cloud** (es. Claude via chiave API) — analisi avanzate.
  - **Locale/offline** (es. Ollama) — privato, gratuito.
- L'IA analizza i dati realmente presenti nell'app: composizione mazzo, statistiche, cronologia partite,
  matchup, collezione — e fornisce consigli (es. cosa craftare, aggiustamenti del mazzo, lettura dei matchup).

## 3. Dati delle carte (approccio ibrido)
- **Database locale** delle carte (basato su dati Scryfall) per velocità e uso **offline**.
- **Aggiornamento online automatico** quando esce un nuovo set o sono disponibili dati nuovi.
- I dati delle carte includono: costo, colori, tipo, rarità, testo, legalità per formato, immagini, nomi multilingua.

## 4. Architettura tecnica (decisa dopo ricerca 2026-06-21)
- **Tipo di app:** desktop con interfaccia moderna e grafici ricchi.
- **Stack scelto: Tauri 2 + Svelte 5 + TypeScript (UI) + backend Rust.** (Framework UI cambiato da React a Svelte 5 il 2026-06-21 su scelta dell'utente: più moderno/leggero, accettando ~+5-10% effort e qualche rischio in più. TypeScript mantenuto.)
  - Database locale **SQLite** (plugin SQL ufficiale di Tauri / sqlx).
  - Grafici con libreria web (es. Recharts/ECharts).
  - Lettura log di Arena nativa in Rust (crate `notify`).
  - HTTP per Scryfall e provider IA via Rust (`reqwest`).
- **Portabilità:** eseguibile "a cartella" leggerissimo (~5–15 MB) eseguibile senza installazione (anche da USB).
  Su **Windows 11 WebView2 è già preinstallato**, quindi non serve includerlo.
- **Sorgente carte:** dati Scryfall (bulk locale + aggiornamento).
- **IA:** livello di astrazione che permette di scambiare provider (cloud/locale) da impostazioni.
- **Principio assoluto:** nessun valore hard-coded; tutto data-driven (set, formati, lingue, legalità).

### Perché Tauri e non Electron (analisi basata sui dati)
I motivi classici pro-Electron non si applicano a questo progetto:
- "Serve saper usare Rust" → lo sviluppo lo fa l'assistente (Rust gestito); per l'utente è trasparente.
- "Resa grafica uniforme cross-OS" → l'app è **solo Windows 11** (WebView2/Chromium uniforme e moderno).
- "Ecosistema maturo per ogni caso" → le nostre esigenze (SQLite, grafici, log-watch, HTTP/IA) sono tutte coperte.
Vantaggi decisivi di Tauri **sui requisiti dell'utente**: app ~25x più piccola, ~58–75% meno RAM, avvio ~4x più
rapido → ideale per un'app **portable** che non appesantisce il PC durante il gioco.
Alternative scartate: **Electron** (pesante), **.NET/Avalonia** (ottimo ma UI/grafici più lenti da realizzare),
**Flutter** (motore proprio, meno adatto ai grafici web ricchi).

## 5. Roadmap di realizzazione (a mattoni atomici, scope già completo)
Ordine di costruzione (ogni fase termina con qualcosa di verificabile insieme):
1. **Fondazione**: struttura del progetto, app portable che si avvia, database locale. ✅ COMPLETATA (2026-06-21) — scaffold Tauri 2 + **Svelte 5 + TypeScript** in `app/`, build OK, test manuale superato (greet Svelte↔Rust). NB: inizialmente React, poi rigenerato in Svelte 5 su scelta utente.
2. **Dati carte**: import/aggiornamento dataset Scryfall locale. ✅ COMPLETATA (2026-06-21) — + miglioria: tabella `sets` (Scryfall `/sets`) per mostrare i NOMI completi dei set accanto ai codici (comando `ensure_set_names`, join in tutte le query carte). — DB SQLite locale (~20 MB) con le sole ~21k carte di Arena, scaricate da Scryfall (`default_cards`, lettura a flusso); ricerca per nome con immagini; comando di aggiornamento manuale CON avanzamento; rilevamento automatico delle carte nuove all'avvio (confronto conteggio Scryfall vs salvato) + avviso e aggiornamento a un clic. Test backend + 2 test manuali utente superati. Moduli Rust: `models.rs`, `db.rs`, `scryfall.rs`; comandi: `get_database_status`, `search_cards`, `get_card`, `update_card_database`, `check_for_updates`. Utility di test: `examples/sim_update.rs`.
3. **Import/Export mazzi**: parser + export Arena + copia appunti. ✅ COMPLETATA (2026-06-21) — modulo `deck.rs` (parser sezioni Commander/Companion/Deck/Sideboard, righe `N Nome (SET) Numero`), abbinamento al DB per set+numero con fallback per nome, export fedele ad Arena (riusa i codici set originali del file). Comandi `import_deck`/`export_deck`. UI: pannello "Decks" (incolla o carica .txt, vista carte abbinate con immagini, "Copy to Arena"). 4 test backend OK + test manuale utente OK (Brawl/Standard/Historic). NB: app convertita TUTTA in inglese (UI compresa) in questa fase.
4. **Analisi mazzo**: tutti i grafici e la validazione legalità.
5. **Editor mazzi**: ricerca carte, creazione, salvataggio.
6. **Tracking partite**: watcher dei log, cronologia, win rate, matchup.
7. **Collezione/wildcard**: lettura collezione e calcolo wildcard.
8. **IA**: integrazione provider configurabile e pannelli di consigli.
9. **Rifinitura**: impostazioni, packaging portable finale, test guidati.

## 6. Decisioni confermate dall'utente (2026-06-21)
- Dati carte: **ibrido** (locale + aggiornamento online).
- Tracking partite: **automatico** dai log di Arena.
- IA: **configurabile** (cloud + locale).
- Scope: **tutte le funzioni progettate dall'inizio**.
- Collezione/wildcard: **inclusa**.
- Nomi carte nell'interfaccia: **inglese** (UI in italiano).

## 7. Da confermare prima di iniziare a costruire
- VIA LIBERA sullo stack **Tauri 2** (in attesa di conferma utente).
- Provider IA concreti da supportare per primi (es. Claude API + Ollama).
