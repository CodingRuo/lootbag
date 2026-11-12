# Product direction

Build games from the assets you already own.

René supplied this direction on 2026-10-07. It guides later specifications;
it does not expand the current sound-library implementation ticket.

## Core workflow

One library serves many game projects. While planning a game, its asset
selection contains references to existing assets, selected variants and their
roles, such as forest ambience, hero or inventory icon. Creating a selection
does not copy the files. Original package hierarchy and provenance remain
available, including for generically named files such as `A001.wav`.

Copy files only when the user explicitly prepares or exports a selection to
a chosen game-project location. Keep that export separate from the library;
engine-specific handling follows the generic workflow.

## Current foundation

- Managed folder/ZIP imports and referenced external sources.
- Sound discovery, annotations, format variants, playback and individual export.
- Recoverable package/file trash and project folders holding asset references.
- Project folders as selections: per-asset role and chosen variant, and an
  export of the folder (with subfolders) that reports unavailable files first
  and writes a `manifest.json` beside the copied variants and companion files.

Game-project profiles (setting, genre, style, engine) are still open, as are
content hashes for moved files and license evidence in the manifest.

## Next useful step

Specify one game-project profile and one reviewable asset selection before
building semantic search or an embedded AI system. A profile records the
game's setting, genre, style, mood, engine and camera where useful. A selection
records the asset ID, chosen variant and intended role. Persist selections
independently of their display paths and preserve provenance.
Define content hashes and source identity for detecting moved or updated
files before promising automatic relocation or cross-computer sync. The
current external-source checks and manual relinking remain the baseline.

Prepare the selection for a chosen destination with a manifest of assets,
variants and required companion files. Report unavailable files and unresolved
dependencies before copying. Keep the library originals intact. License
evidence must be retained; technical exportability does not establish usage
rights.

## Built-in AI direction

The future product should suggest a coherent asset set for a game, rather
than only return search matches. Use the project profile and optionally a
reference asset to find compatible items already owned by the user.

Suggestions identify candidate assets, their proposed roles and the reasons
for selection. The user reviews, accepts or rejects them before they become
part of a project or are exported. Surface missing categories and uncertainty;
do not invent assets, license permissions or unsupported compatibility scores.
Percentage scores require a defined and validated measurement.

Embedded AI is a separate user-facing workflow. Future capabilities can reuse
the same project and selection data, but no speculative AI provider integration
is added now.

## Later increments

1. Project profiles, variant/role selection and a generic selection export.
2. License records and dependency-aware packaging; useful 3D/image previews.
3. Reviewable matching and missing-asset suggestions against the owned library.
4. Engine-specific exporters and source/update/synchronization integrations
   after their concrete workflows are validated.

The pasted competitor comparison is context, not verified market research.
Competing on Git, locking or an ever-growing format count is not the current
product goal. A matching engine, graph store, vector database or background
AI service needs a demonstrated requirement before it is introduced.
