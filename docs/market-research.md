# Market research (2026-11-12)

Goal: a free, local-first asset library for indie game devs. Not a business.
Prices from vendor pages on the date above; re-check before quoting.

## Closest competitors

| Tool | Price | Platforms | Notes |
|---|---|---|---|
| [AssetHoard](https://assethoard.com) | Free beta, paid licence planned | Win/Mac/Linux | Closest match: 80+ formats, zip browsing, engine export. |
| [Sharp Stock](https://sharpknifegames.itch.io/sharp-stock-lite) | $34.99; Lite capped at 100 assets | Win/Mac | Images, audio, Spine, 3D; atlas tools. |
| [Asset Inventory 4](https://assetstore.unity.com/packages/tools/utilities/asset-inventory-4-349582) | ~€54 | Unity Editor only | Indexes all owned Unity packages. |
| [Eagle](https://en.eagle.cool/store) | $34.95 | Win/Mac | Visual refs; copies files into its library; 3D via plugin. |
| [Soundly](https://getsoundly.com) | Free up to 10k local sounds, then subscription | Win/Mac | Audio only. |
| [Sononym](https://www.sononym.net) | €89 | Win/Mac/Linux | Audio only; similarity + duplicate search. |
| [Allusion](https://github.com/allusion-app/Allusion) | Free, GPL | Win/Mac/Linux | Images only; dormant since 2024. |
| [Anchorpoint](https://www.anchorpoint.app/pricing) | Team subscription | Win/Mac | Version control for teams, not a pack library. |

Vendor-locked: Fab/Quixel Bridge (Epic), KitBash3D Cargo, SyntyDex. Engine-locked: Blender Asset Browser,
Godot "Global Asset Manager". Small/abandoned OSS: BtAssetManager, GLAM, ARCANA Vault (cancelled).

## Gaps worth filling (cheap to build)

1. Pack as a record: store, URL, licence, purchase date, version.
2. Credits/licence export (CREDITS.md) from a project selection.
3. Hash-based duplicates across bundles (Synty/Humble overlap).
4. Browse inside ZIP/unitypackage without extracting.
5. Auto-tags from folder and filename tokens (`SM_Prop_Barrel_01` → prop, barrel).
6. Fast keyboard grid + saved searches (SQLite FTS5).
7. Windows build.

Already built and rare among free tools: multi-type preview, sprite sequences, animation-only FBX
playback, project export with manifest.

## Do not build

Cloud sync, accounts, team locking, store/purchase integrations, AI generation, heavy AI tagging,
editing tools (atlas/audio editors), engine plugins as a dependency, version control.

## Name

"Lootbag": low conflict (Minecraft mods, a Unity addon; no asset-manager tool found; lootbag.com and
lootbag.dev showed no DNS). Alternatives: Packwright (no conflict found), Packmule, Stowage.
Avoid: Hoard (AssetHoard), Stash, Trove, Quiver, Loadout, Packrat. Trademark registers not searched yet.

## Round 2 (2026-11-13): what others do that Lootbag does not

Gaps 1–7 above are built (except `.unitypackage` browsing). Sources:
[AssetHoard](https://assethoard.com) and its [roadmap](https://assethoard.com/roadmap),
[Sharp Stock](https://sharpknifegames.itch.io/sharp-stock-lite),
[Asset Inventory](https://www.wetzold.com/tool/asset-inventory/),
[Sononym](https://www.sononym.net), [mipGAM](https://bytefoundry.itch.io/mipgam),
[electron-dam](https://github.com/simeonradivoev/electron-dam).

| Gap | Who has it | Cost | Verdict |
|---|---|---|---|
| Drag assets out into an engine, DAW or Finder | AssetHoard, Sharp Stock, Asset Inventory | S–M | Build |
| Reveal in Finder / open in the default app (Aseprite, Blender, Audacity) | mipGAM, most DAMs | S | Build |
| Rescan packs and watched folders for new or changed files | AssetHoard | M | Build |
| Browse `.unitypackage` in place (Synty, Asset Store) | AssetHoard, Asset Inventory | M | Build |
| Font preview (TTF/OTF/WOFF) | AssetHoard, Sharp Stock | S | Build |
| Waveform thumbnails on sound cards | AssetHoard | S–M | Build |
| Detect common licences (CC0, CC-BY, MIT, Kenney) in licence files | AssetHoard | S | Build |
| Filter images by dominant colour | Sharp Stock, AssetHoard (planned) | M | Later |
| Auto-slice spritesheets, export GIF | AssetHoard | M | Later |
| Similar-sound search (local audio features, no AI service) | Sononym, Asset Inventory | L | Later |
| Local MCP server so coding agents can search and copy assets | AssetHoard, Asset Inventory | M | Later |
| Semantic/AI tagging, background removal, image compression | AssetHoard, Sharp Stock, Asset Inventory | L | Skip (AI/editing) |
| RAR/TGZ, `.uasset`, GameMaker packages | AssetHoard | L | Skip for now |
| Store order sync, update checks, team databases | Asset Inventory | L | Skip (accounts/teams) |
