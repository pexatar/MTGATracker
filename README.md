# MTG Arena Tracker

A local, portable desktop app for **Magic: The Gathering Arena** (Windows 11).
Import and analyze your decks, build new ones, track your games, and get
AI-powered insights — all offline-first, with no installation required.

> Status: early development. Foundation and local card database are in place;
> deck import/export, analytics, match tracking, collection/wildcards and AI
> are on the roadmap.

## Features

- **Import** decks exported from MTG Arena (`Commander` / `Deck` / `Sideboard`).
- **Export** decks back to the Arena-compatible format.
- **Analyze** decks with charts: mana curve, colors, types, rarity, legality.
- **Build** decks in-app with card search and images.
- **Track** matches automatically by reading the Arena logs (win rate, matchups).
- **Collection & wildcards**: see which cards you own and what you need to craft.
- **AI insights** over your own data (configurable: cloud or local model).

## Card data

Card information comes from [Scryfall](https://scryfall.com). On first run the
app downloads the bulk data once and stores only the Arena-legal cards in a
small local SQLite database (~20 MB), then keeps it up to date automatically.

## Tech stack

- [Tauri 2](https://tauri.app) — lightweight, portable desktop shell.
- [Svelte 5](https://svelte.dev) + TypeScript — UI.
- Rust — backend (database, networking, log parsing).
- SQLite — local card database.

## Development

Prerequisites: [Rust](https://www.rust-lang.org/tools/install) and
[Node.js](https://nodejs.org).

```bash
cd app
npm install
npm run tauri dev      # run the app in development
npm run tauri build    # build a portable release
```

## License

See [LICENSE](LICENSE).
