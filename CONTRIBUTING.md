# Contributing to Lootbag

Thanks for helping out. Lootbag is a Tauri 2 desktop app (Rust in `src-tauri/`,
React/TypeScript in `src/`) and currently targets macOS 15.4 or later.

## Setup

Install Node.js 22.18+ (or 24+), pnpm, Rust, and the macOS Command Line Tools.

```sh
pnpm install
pnpm tauri dev
```

## Checks

Run these before opening a pull request:

```sh
pnpm typecheck
pnpm test
cd src-tauri && cargo test
```

Keep changes focused, add a test for new behavior, and use the domain terms
from `GLOSSARY.md`. By contributing you agree that your work is licensed under
GPL-3.0-or-later (see `LICENSE`).
