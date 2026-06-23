# IA — Documento di design (Fase 8)

> Documento di progettazione dell'IA del MTG Arena Tracker.
> Versione iniziale: 2026-06-24. Stato: **design completo, implementazione non iniziata.**
> Scope progettato dall'inizio (vedi preferenza progetto completo); l'implementazione sarà atomica e verificabile.

---

## 1. Visione: un "companion" per MTG Arena
L'IA è un **assistente** integrato nell'app con **due anime distinte**:
- **Data Analyst** (oggettivo): legge i **dati reali** dell'utente (mazzi, statistiche, partite, wildcard) ed estrae fatti, statistiche e pattern. Affidabile perché ancorato ai dati.
- **Coach** (consultivo): fornisce consigli strategici e costruttivi, spiegazioni e suggerimenti. Utile, ma da trattare come **opinione di un assistente**, con i limiti dichiarati al §8.

Tenere distinte le due anime è un requisito di prodotto: non spacciare mai un'opinione (coach) per un dato (analyst).

---

## 2. Principi guida
1. **Scope: solo MTG Arena.** Formati, pool carte e fonti dati sono ristretti ad Arena. "Arena-only" significa anche **scegliere fonti meta che misurano il ladder di Arena**, non i tornei cartacei/MTGO (che hanno meta diverso anche sullo stesso formato).
2. **Ancoraggio ai dati reali (anti-allucinazione).** L'IA **non inventa**: lavora sul **DB carte reale e aggiornato** (Scryfall, `game:arena`) e sui dati dell'app; le sue proposte sono **validate dal nostro codice** (esistenza carte, legalità di formato, conteggi).
3. **Offline-first / portable.** Il nucleo IA funziona **offline**, sulla chiavetta, senza installazioni. Tutto ciò che richiede internet (cloud, meta data) è **opzionale** e separato dal nucleo.
4. **No hardcoding.** Provider, formati, criteri e dati sono **data-driven**: niente liste/valori cablati.
5. **Configurabile.** Provider IA e lingua delle risposte si scelgono dalle Impostazioni.
6. **Limiti onesti dichiarati** (§8): l'IA non promette ciò che non può mantenere.

---

## 3. Architettura tecnica
- **Adattatore IA** (interfaccia comune): l'app parla con un'unica interfaccia; dietro, i provider sono **intercambiabili**.
- **Provider locale (default):** **llama.cpp incorporato** nell'app che carica un modello **GGUF**. Usa la **GPU (CUDA)** se presente, con **fallback automatico su CPU** per i PC senza scheda NVIDIA (requisito di portabilità). Modello = un file accanto all'app sulla chiavetta.
- **Provider cloud (opzionali):** un connettore **"OpenAI-compatible" generico** (copre ChatGPT/OpenAI, Gemini via endpoint OpenAI, OpenRouter, ecc.) **+ Anthropic nativo** (Claude). Chiavi API **salvate cifrate**.
- **Accesso ai dati:** l'IA riceve come **contesto** i dati reali dell'app (composizione mazzo, statistiche, partite, wildcard) e può attingere al **DB carte** per non allucinare nomi. Tecniche: dati in contesto, structured output, eventuale tool-use/retrieval.
- **Default:** locale offline; cloud **opt-in**.

Riferimento preferenze utente: niente LLM cinesi (solo modelli occidentali); lingua output configurabile (default italiano).

---

## 4. Modello locale
**Criteri di scelta:** origine occidentale (Llama/Gemma/Mistral/Phi — **no** Qwen/DeepSeek), formato **GGUF** compatibile con llama.cpp, taglia adatta a ragionamento testuale multilingue (italiano/inglese), buon equilibrio **qualità ↔ portabilità**.

**Candidati verificati (giugno 2026):** Gemma 4 (E2B / E4B / 12B), Mistral Nemo 12B, Phi-4, Llama 3.x — tutti disponibili in GGUF.

**Strategia "due modelli" sulla chiavetta:**
- **Qualità** (sul PC dell'utente, RTX 4070 12 GB): un ~**12B** in GPU (es. Gemma 4 12B / Mistral Nemo 12B).
- **Leggero** (PC altrui scarsi / portabilità estrema): un effective-2B/4B (es. Gemma 4 E2B/E4B, Phi-4-mini).
L'utente sceglie il modello attivo dalle Impostazioni e può puntare a qualunque `.gguf`.

> ⚠️ Nomi, taglie e dimensioni esatti dei file GGUF vanno **verificati sulla pagina Hugging Face reale al momento del download** (non fidarsi di riassunti). La build di llama.cpp incorporata deve supportare l'architettura del modello scelto (i modelli nuovissimi possono richiedere una versione recente).

---

## 5. Funzioni del companion (scope completo)
Per ciascuna: cosa fa, dati usati, fattibilità e accorgimenti.

**A. Analisi del mazzo** — pro/contro, curva di mana, ripartizione colori, tipi, rarità, legalità per formato, "come si pilota", best practice. *Dati:* composizione + statistiche (Fase 4). *Fattibilità:* alta.

**B. Statistiche personali** — win rate per mazzo, andamento nel tempo, record per formato, quale mazzo rende di più/meno. *Dati:* cronologia partite (Fase 6). *Fattibilità:* alta.

**C. Suggerimenti e migliorie su un mazzo esistente** — "rendilo più aggressivo", "abbassa la curva", "più rimozione"; **integrazione wildcard**: "suggerisci un mazzo che posso permettermi", "alternativa più economica a questa carta". *Dati:* DB carte + wildcard/craft cost (Fase 7). *Fattibilità:* alta. *Accorgimento:* proposte solo tra carte reali e legali, validate dal codice.

**D. Spiegazioni didattiche** — "perché questa carta?", "perché questo matchup è difficile?". Rafforza il ruolo coach. *Fattibilità:* alta.

**E. Creazione mazzi da richiesta naturale** — es. "crea un UB control per Brawl", "combo Historic BO1". *Approccio:* l'IA traduce la richiesta in criteri (colori, formato legale, ruoli) → l'app **filtra il DB reale** (carte esistenti e legali) → l'IA assembla tra carte vere → il codice **valida** legalità e conteggi (gestione BO1/BO3, sideboard). *Aspettativa onesta:* mazzo **coerente e giocabile**, **non** garantito "top-tier da torneo" (vedi §8).

**F. Coaching matchup** — "contro quale archetipo perdi e come giocarci". *Prerequisito:* oggi salviamo solo il **nome** dell'avversario, non le sue carte → serve **estendere il parser** per catturare le carte avversarie *viste in gioco* e inferirne l'archetipo (resta **parziale**: si vedono solo le carte giocate). *Fattibilità:* media, dopo l'estensione del parser.

**G. Meta data (opzionale)** — arricchisce il coach con il **meta aggiornato** (archetipi diffusi, play rate, win rate), che il modello non conosce. *Vincolo:* **solo via lecita** (import manuale di un export, o API con licenza compatibile) — **mai scraping** (ToS, fragilità, repo pubblico). *Fonti:* Arena-ladder (Untapped/AetherHub/MTGA Assistant), non tornei cartacei. *Complementare* ai dati personali, non sostitutiva.

---

## 6. Dati e fonti
- **Interni (reali):** DB carte (`cards`), mazzi (`decks`), partite (`matches`), inventario/wildcard (`inventory`).
- **Esterni (opzionali):** meta data Arena-ladder, via import lecito.
- **Dato avversario:** oggi solo il nome (`opponent`); l'archetipo richiede l'estensione del parser (funzione F).

---

## 7. Interazione e UX
- **Companion = report strutturati + chat conversazionale.** L'utente riceve un'analisi e può poi farci domande di follow-up.
- **Lingua output configurabile** (default italiano per l'utente; impostabile in inglese per altri).
- **Formato risposta:** strutturato dove utile (sezioni: punti forti / debolezze / carte da aggiungere-togliere), con distinzione visibile tra **fatto** (analyst) e **consiglio** (coach).
- **Confini:** l'IA consiglia e insegna, **non gioca** al posto dell'utente.

---

## 8. Limiti onesti
- **Meta datato:** nessun LLM conosce il meta MTGA *attuale* → mitigato con il meta data importato (funzione G).
- **Allucinazioni:** mitigate dall'ancoraggio al DB reale + validazione del codice.
- **Locale vs cloud:** il locale basta per analisi e data; per **creazione mazzi** e ragionamenti difficili il **cloud rende meglio**. L'architettura multi-provider serve a questo.
- **Creazione mazzi:** risultato "giocabile e coerente", **non** garantito competitivo top-tier (limite intrinseco di un modello locale amatoriale).
- **Matchup:** dipende dal dato avversario, da costruire (funzione F).

---

## 9. Privacy e sicurezza
- **Offline di default:** nessun dato lascia il PC senza scelta esplicita (cloud **opt-in**).
- **Chiavi API cifrate.**
- **Niente scraping** (ToS/legalità) e **niente memory-scanning** (renderebbe l'app indistinguibile da un malware) — scelte già prese nel progetto.

---

## 10. Ordine di implementazione (per dipendenze tecniche, NON per scope)
Lo scope è completo; questo è solo l'ordine in cui conviene costruire, perché ogni blocco si appoggia al precedente.
1. **Fondamenta:** motore IA (adattatore + llama.cpp/GGUF, GPU+fallback) **+ accesso ai dati reali** (DB carte, mazzi, statistiche, partite, wildcard).
2. **Funzioni su dati già disponibili:** A (analisi mazzo), B (statistiche), C (suggerimenti/migliorie + wildcard), D (spiegazioni), E (creazione mazzi guidata dal DB).
3. **Funzioni che richiedono nuovi dati:** F (coaching matchup → prima estendere il parser avversario), G (meta data → prima il modulo import lecito).
4. **Interazione completa** (report + chat) e **provider cloud** opzionali sopra il tutto.

---

## 11. Stato e prossimo passo
- **Design:** completo (questo documento).
- **Codice:** non iniziato.
- **Prossimo passo atomico:** punto 1 (fondamenta) — al **VIA LIBERA** dell'utente, dopo build → test manuale (gate inviolabile).

Collegato al piano generale: [PROGETTO.md](PROGETTO.md), Fase 8.
