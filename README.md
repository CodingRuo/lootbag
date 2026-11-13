<p align="center"><img src="branding/lootbag-mark.svg" width="96" alt=""></p>

# Lootbag

A free, local asset library for game devs. Keep sounds, sprites, materials,
3D models and animations from all your packs in one searchable place, preview
them, and copy exactly what a game project needs into it, with credits.

No account, no cloud, no subscription. Your files stay where they are.

## Features

- **Import packs** from folders or ZIPs, or reference them in place (external folders and ZIPs are never copied or changed).
- **Preview everything**: sounds with waveform and loop check, images and spritesheets, GIF and Aseprite, materials on a sphere, GLB/glTF, FBX and OBJ models with their animations, Blender thumbnails.
- **Find things fast**: prefix search (`barr nuk` finds `SM_Prop_Barrel_Nuke_01`), automatic tags from file and folder names, filters, saved searches.
- **Spot duplicates** across packs and bundles.
- **Project folders**: collect assets for a game, give them roles, pick variants, and export them with `manifest.json` and a `CREDITS.md` built from each pack's licence info.
- **Sprite tools**: frame grids, atlas and packed-atlas export with `sprite.json`.

The full behavior, limits and edge cases are in [docs/features.md](docs/features.md).

## Download

macOS 15.4 or later: get the signed and notarized app from
[Releases](https://github.com/CodingRuo/lootbag/releases). Windows builds
compile but have not been tested on real hardware yet (see [Windows](docs/features.md#windows)).

## Build from source

Install Node.js 22.18+ (or 24+), pnpm, Rust and the macOS Command Line Tools.

```sh
pnpm install --frozen-lockfile
pnpm tauri dev                     # run in development
pnpm tauri build --bundles app     # unsigned app bundle
scripts/release-macos.sh           # signed + notarized release (needs a Developer ID)
```

Checks before a pull request:

```sh
pnpm test && pnpm build
cargo test --manifest-path src-tauri/Cargo.toml
```

Tauri 2 (Rust) with React, TypeScript, three.js and a local SQLite catalog.

## Contributing

Issues and pull requests are welcome, see [CONTRIBUTING.md](CONTRIBUTING.md)
and the domain terms in [GLOSSARY.md](GLOSSARY.md).

## License

Free for everyone, including commercial game projects, under
GPL-3.0-or-later (see [LICENSE](LICENSE)). Paying is voluntary: if Lootbag
saves you time, star the repo, report bugs or tell other devs about it.
