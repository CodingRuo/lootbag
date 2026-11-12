# Game Asset Library

A local game development asset library for sounds, images and models on macOS.
The current editor increment is implemented on `codex/asset-editor`.
Source checks, native acceptance, and final review gates are recorded below.

## Development

The application uses Tauri 2, React, TypeScript, and a local SQLite catalog.
Install Node.js, pnpm, Rust, and the macOS Command Line Tools before building.
The app targets macOS 15.4 or later for the accepted OGG/Vorbis preview path.
Native acceptance runs on Apple Silicon; Windows delivery follows separately.

```sh
pnpm install --frozen-lockfile
pnpm tauri dev
```

Create a production app bundle with:

```sh
pnpm tauri build --bundles app
```

## Library storage

Choose a library folder or let the app create its default in your home
directory. Managed imports copy package contents into library storage,
preserving original folders and companion files. Referenced sources retain
their files at the external location. The catalog stores names, tags, and
provenance without renaming original files.

Use `Paket importieren` to select either a folder or a ZIP in the same native
dialog. Review the package before copying; ZIP imports show an extraction-space
estimate and confirmation after explicit consent to an unverified whole-package import.
Native drops of folders or ZIP files use the same checks, progress, cancellation
and ZIP confirmation; up to 32 sources are processed sequentially. A failure
stops the queue and reports any earlier successful imports.
macOS companion files (`._*` and files inside `__MACOSX`) remain in the
package storage but do not appear as assets. Existing catalogs are corrected
on open, after creating a `catalog-before-macos-metadata-*.sqlite3` backup.

Close the app before manually copying or moving the entire library.
Managed locations are relative to the library root. External sources can
require relinking after moving to another computer.
Relinking validates the original source. Add a copied folder with different
file identities as a new external source.

### Exact file duplicate evidence

Choose `Datei-Duplikate prüfen` in the catalog, then the whole library or the
selected package. `Umfang vorbereiten` captures active indexed variants across
all pages; `Prüfung starten` starts cancellable work. Search, category, tag,
project-folder and audio filters do not narrow this scope. Trash and unindexed
companions are excluded.

Streamed SHA-256 nominates candidates; actual byte comparison confirms groups.
The paged report retains package, original path, file and asset IDs and labels
shared identities such as hardlinks. Missing, changed, blocked or over-limit
inputs cannot produce a complete clean result. Stop retains incomplete evidence.

Originals, annotations and the catalog remain unchanged. macOS platform hashing
uses 64 KiB buffers with limits of 5,000 variants, 4 GiB per file, 32 GiB total
reads including comparison, and 120 seconds of cooperative work. The report
describes an observation interval, not a frozen filesystem snapshot. Equal model
bytes do not establish equal textures, companions or licenses, and hardlinks do
not establish reclaimable space. Persistent fingerprints and native
macOS/Windows acceptance remain open.

### Review before import

Folder preparation compares incoming asset candidates with active indexed
variants across the library, including external references. The review shows
byte-identical matches with package and original-path evidence. Complete folder
inspection binds copying to the inspected whole package, including companions
and empty directories. Changes require fresh preparation; files are not
silently omitted, merged or deleted. Cancel skips the current package in a drop
queue; Stop or a failure ends the queue while retaining earlier committed imports.

Inspection is bounded to 20,000 entries, 48 directory levels, 4,096 UTF-8 bytes
per path, 256 MiB per incoming file, 1 GiB incoming contents, 2 GiB charged read
work, 5,000 existing variants and 30 seconds of cooperative work. Retained plan
data also has finite limits. Unavailable or over-limit evidence offers an
explicit unverified whole-package continuation through the existing importer;
these inspection limits do not impose an import-size ban.

Initial ZIP evidence and counts are explicitly unavailable: preparation does
not construct an archive or estimate its contents. Only confirmed unverified
continuation starts the existing ZIP estimate and space confirmation. Legacy
ZIP parsing/allocation and authorized full-folder traversal remain outside the
bounded inspection guarantee. Cancelled readers can briefly require retry;
possible temporary-folder remnants are reported. Source checks pass on macOS
arm64, without an atomic filesystem snapshot, durable fingerprints, complete
ZIP content binding or native macOS/Windows acceptance.

### Backups and restoration

Choose `Bibliothek sichern` below the storage location, select a destination
parent outside the current library, then `Vorbereiten`. Review the captured
contents and limits before `Starten`. A successful backup creates a fresh
`library-backup-<UUID>` folder containing `manifest.json` and `library/`.
It includes the catalog, managed originals, companions, licenses, empty folders,
annotations, project membership and retained trash. External sources contribute
catalog records only; their original files are not copied.

Choose `Bibliothek wiederherstellen` from the initial screen or storage location.
Select that backup folder and a destination parent outside the backup/current
library. Preparation checks the manifest, contents and supported catalog shape.
Start publishes a fresh `library-restored-<UUID>` library, without replacing or
merging an existing destination. Afterwards, explicitly choose
`Wiederhergestellte Bibliothek öffnen`; restoration does not switch libraries
automatically. External locations can still require relinking.

Progress and Stop cover copying and verification. Hashes, catalog integrity and
source checks precede publication. Changed sources, links, special files, WAL
catalogs and potentially pending journals are refused. Unknown cleanup remnants
and a published result with durability warnings remain visible.

This ordinary-folder v1 has finite planning, retained-data and cooperative work
limits, including 2 TiB of logical contents, 250,000 entries and a 24-hour run.
These are admission limits, not measured large-library throughput. It does not
preserve hardlink topology or filesystem attributes, resume interrupted work,
encrypt backups or synchronize computers. Source checks pass on macOS arm64.
Native macOS backup, null-root restoration and explicit Open pass on an owned
9.5-MB fixture, including sound/model/image previews from the restored root.
Long-running Stop, failure/retry, extended interaction and Windows remain pending.

### Storage overview

Open `Speicherübersicht` below the library storage location, then choose
`Aktualisieren` for an explicit, cancellable measurement. The report separates
managed package contents (including companion files), retained trash, catalog
and support files, import staging and other files. Hardlinks are counted once,
with a separate category when names span categories. It measures logical file
bytes, rather than allocated or reclaimable disk space; APFS clones and sparse
files can occupy a different amount on disk.

External sources show historical catalog values separately from the library
sum. They are not crawled and can be offline or outdated; uncataloged external
companions are excluded. Links, special files, changed or missing sources and
limit exhaustion make the report incomplete. Stop retains only a visibly
incomplete measurement. Runtime previews use memory; this report does not
measure RAM or operating-system/WebView caches outside the library.

The reporter leaves the catalog and original files unchanged. WAL catalogs and
potentially pending rollback journals are explicitly unavailable; inactive
retained journals remain part of storage. The report does not recover a catalog,
delete files or provide a verified full-library backup. Traversal is qualified
on macOS with finite catalog, entry, name/depth and cooperative work limits.
Native macOS and Windows acceptance remain pending.

## Images, sprites and models

New imports and external references discover PNG/JPEG/WebP/TGA/GIF/Aseprite images and
GLB/glTF 2.0, FBX, OBJ and Blender source models alongside sounds. Existing packages keep their
catalog entries until explicitly reimported. Search and category filters work
together; identically named assets of different kinds remain separate.

Images show dimensions, transparency, zoom and pixel mode. Open the regular
spritesheet controls to choose columns, rows, four outer margins, horizontal and
vertical cell gaps and FPS, then play or step frames. Left/top margins define the
grid origin. Crops must use whole pixels; invalid layouts show the full sheet
with animation disabled. These settings are session-only and preserve the original image.
On macOS, `PNG-Bilder exportieren` saves every current grid cell in row-major
order; `Atlas exportieren` removes margins/gaps into a regular atlas.
`Gepackten Atlas exportieren` trims transparent cell borders and packs the
unequal rectangles without rotation. Choose a project directory to create a
fresh `<source>-frames`, `<source>-atlas` or `<source>-packed` folder with PNGs
and `sprite.json`. Existing destinations are protected.

The packed v2 manifest retains row-major frame indices, original cell size,
trim offsets, top-left atlas core coordinates, uniform frame duration and
catalog provenance. Empty frames keep their position and timing with a
transparent 1×1 core. `padding: 1` and `extrusion: 1` describe the same one-pixel
band repeating edge/corner pixels. This bounded packing heuristic does not
promise optimal size or protection at arbitrary mip levels. Derived canvas
PNGs normalize invisible RGB and may round partially transparent colors;
library originals remain byte-identical. Frame and regular-atlas v1 manifests
retain their existing format.

Limits are 256 frames, 8 Mi source-crop and output pixels, 8192 pixels per output
edge and 32 MiB of PNG data. Export work has a cooperative 30-second deadline
after choosing the destination. Stop waits for native cancellation and the old
export to settle before retry; cancellation after publication retains the
complete export. GIF/Aseprite animations use their own inspectors. Saved named
clips, packing independent images and Windows publication remain pending.

The editor separates Preview, Properties and Origin. Searchable cards or a
compact list share pagination and filters; package navigation retains full
names in tooltips. Export remains visible while scrolling the inspector.
Checkboxes, Command/Control-click and Shift ranges select multiple entries on
the current page. Arrow keys, Home/End and Shift-arrow work from a focused asset;
Command/Control-A selects that page and Escape clears the selection. Changing a
filter or page clears the selection. `Sammelaktionen` adds tags without replacing
existing annotations, assigns project folders and recoverably trashes all
variants. Material and sprite group selections include all their maps/frames.
Each batch is atomic: an invalid entry or tag limit leaves the batch unchanged.

Choose `Materialien` to browse texture sets within their original package and
folder. Select maps explicitly when a role is ambiguous, or omit a map while
keeping its warning visible. Preview a sphere/plane with tiling and normal
convention controls, using pointer, keyboard or camera buttons. Export copies
the selected original maps into a protected new material folder.

Choose `Sprite-Sequenzen` for numbered frame files with the same prefix and
source folder. Play, step or seek frames in numeric order, adjust FPS, zoom,
pixel rendering and transparency, then export the complete frame bundle.
Duplicate frame indices and incompatible image sizes require correction.

GLB/glTF, FBX and OBJ models use local package resources with orbit, zoom, fit/reset and mesh counts.
Embedded GLB/glTF and FBX animation clips provide play/pause, clip selection,
loop, speed and seeking. Skeleton-only files show their bones; static models
report the absence of clips. Playback pauses when the inspector is hidden.
Export a ready model by choosing a project directory; the app creates a new
`<model-name>-model` folder containing its original file and needed resources,
preserving package-relative paths. Existing model folders are protected.
Missing materials/textures, unsafe references and unsupported encodings are
reported and prevent a complete export. For FBX PSD references, the preview can
use one package-local PNG/JPEG/WebP with the same filename stem. Ambiguous
matches fail explicitly. The viewer identifies each replacement and keeps
complete export disabled because the original FBX still references PSD.
If a known FBX image is genuinely missing, readable geometry remains visible
with a neutral texture and an explicit warning; complete export stays disabled.
TGA previews share a bounded worker across cards, images, material maps, sprite
frames and model textures. RAW/RLE truecolor (24/32-bit), grayscale (8-bit or
16-bit with alpha) and indexed images (8-bit indices, 24-bit zero-origin palette)
are supported, including all four raster origins and v2 footers without extended
metadata. Unsupported 16-bit truecolor, interleaving and extension/developer areas
report an error. Limits are 64 MiB encoded, 16 Mi pixels and 8192 pixels per side;
cards retain proxies up to 512 pixels. Groups/model TGA textures retain at most
32 Mi pixels (combined TGA/GIF/Aseprite for model textures), sprite sequences at most 256 frames, and queued decode jobs have a
30-second timeout. Original exports keep TGA bytes; proxies are temporary.
Existing packages require explicit reimport to add previously uncatalogued TGA
images.

GIF87a/89a previews validate the container before browser-native decoding. The
image inspector switches between natural animation and the first image; changing
modes can restart playback. It does not offer indexed seek, current-frame pause
or custom FPS. Cards, material maps and model textures use a labelled static first
composite. Numbered GIF files remain independent assets. Limits are 32 MiB encoded,
8192 pixels per edge, 16 Mi canvas pixels, 256 frames, 32 Mi canvas-pixels-times-
frames and 100,000 container blocks. Shared queued jobs have a 30-second deadline;
native image loads have 15 seconds. Unsupported rendering extensions, invalid
palettes/rectangles, truncated blocks and invalid trailers fail explicitly.
These limits bound application work; timeouts cannot forcibly interrupt arbitrary
native decoder work. Proxies stay temporary and exports retain original bytes.
External GIF model textures and explicit glTF GIF URI/data/bufferView previews
use the guarded route; GIF is outside glTF 2.0's core PNG/JPEG image formats and
this adds no portability conversion. Embedded FBX GIF images are unsupported.
Existing packages require explicit reimport to discover GIF files.

Aseprite `.ase` and `.aseprite` files use complete canvas composites from flat
normal 32-bit RGBA layers, including raw/compressed cels, backward linked images,
opacity and clipped positions. The inspector plays the whole timeline forward
with each original frame duration; pause, seek, next/first, fit, zoom and pixel
controls are available. Tags are metadata rather than separately selected clips.
Cards, material maps and supported external model textures show a labelled first
complete image. Original exports retain both filename aliases and source bytes.

Limits are 32 MiB encoded, 4096 pixels per edge, 4 Mi canvas pixels, 256 frames,
64 layers and 100,000 chunks; retained frame and unique cel pixels each have a
16 Mi budget, and compositing work has a 64 Mi-pixel budget. The bounded worker
shares the existing queue/deadline. Browser-standard zlib decompression is
required; these application limits are not an OS heap quota. Indexed/grayscale,
groups, tilemaps, non-normal/special layers, scaled cels and unsupported profiles
fail explicitly, including hidden content. No Aseprite process runs in the app.
Existing packages require explicit reimport to discover these sources.

Blender source files show their embedded saved thumbnail in cards and a static
inspector with fit, 100 percent zoom and pixel mode. That image can be older than
the scene; it does not provide interactive geometry, materials or animation.
The guarded entry-only snapshot does not resolve external textures or linked
libraries. Export copies only the unchanged `.blend` original, including when a
thumbnail is missing or unsupported; complete companion-bundle export is disabled.

The supported thumbnail path is uncompressed Blender format 1, version 502,
little-endian/64-bit, with a leading saved TEST image. Limits: 64 MiB input,
1 MiB leading block offset, 256 leading blocks and 2048 pixels per image side
(4 Mi pixels). Legacy, gzip/Zstd, other layouts and missing/malformed thumbnails
report explicit errors. A bounded same-origin worker shares the existing serial
preview lifecycle; no Blender process is run by the app. Existing packages need
explicit reimport to discover previously ignored `.blend` files.

GLB/glTF, FBX and OBJ cards generate actual
static thumbnails serially with one renderer and a bounded session cache;
background thumbnails do not change the inspector's export selection.
PSD files are recognized as image assets by new folder, ZIP and external
imports, and retain their original bytes on export. PSD image decoding is
pending approval of the decoder dependency. Existing packages are deliberately
not rescanned on reopen; previously ignored PSD files require reimport.
The tested Synty Town prop now renders with its supplied PNG; the City Biker
retains its regular preview and export. Geometry and source files are preserved.
See the [native acceptance](.scratch/visual-assets/native-acceptance.md).

## Embedded sound tags

The sound inspector reads title, artist, album and genre from guarded originals
when opened. Embedded text appears separately from filenames, package context,
user names and manual tags; it never replaces those annotations. Supported
missing fields, unsupported profiles and read errors have distinct results.
Reads run independently of playback and waveform work and cancel on source change.

Qualified profiles are little-endian WAV RIFF/INFO, plain MP3 ID3v2.3/v2.4,
single-stream OGG/Vorbis comments and FLAC comments. MP3 ID3v1 footers are
recognized and reported as unsupported; qualified v2 values remain visible.
WAV embedded ID3, RF64/RIFX, MP3 flags/chapters and other unqualified profiles
report their limitation. Text describes the source, not its sound or license.

Limits are 2 MiB physically read, 256 KiB tag input, 256 entries, 4 KiB per
source field and 1 KiB display text per field, with a three-second parsing guard.
This bounds application work, not OS allocation or total queued request time.
Opening a sound again rereads its source; no new tag cache or catalog schema is
introduced. Source and browser boundary checks passed; native macOS and Windows
acceptance for this increment remain pending.

## Sound discovery

Open `Filter` and use `Gespeicherte Audioanalyse` to restrict sounds by duration
in seconds, mono/stereo/more than two channels or analysis state. These criteria
compose with package, project folder, original folder, tags, text and format
before pagination. Adding an audio criterion selects Sounds and retains a
compatible existing sound-format filter.

The inspector initially selects a genuinely matching file variant; every other
variant remains available for deliberate preview/export selection. Those
alternates are labelled separately because they may not meet the active filters.

New imports have no analysis values. Choose Sounds and expand
`Sounds im Suchbereich analysieren` to prepare the complete current search,
review its frozen count of matching file variants, then start analysis with
progress and Stop. All pages and the current package/folder/text/tag/format and
saved-audio filters are included. Clear duration/channel/status restrictions
when you want to include unanalysed sounds. At most 100,000 variants can be
prepared; larger scopes reject visibly and can be narrowed by package.

Stop retains completed measurements. Unavailable or changed sources are skipped
without overwriting older values; stable malformed sources have visible analysis
errors. Originals and annotations stay unchanged. Missing measurements never
count as zero. MP3 duration is unavailable when no reliable frame-count header
exists, while rate/channels/codec can still be measured. Older cached MP3
estimates are corrected only by explicit reanalysis; saved values can become
stale after source edits. Use `Audiofilter aktualisieren` to query again after
analysis. No import/open/completion automatically scans the library.
Saved searches and image/model technical filters remain pending, alongside
native macOS/Windows acceptance.

## Local MCP access

The same executable serves a read-only MCP connection over stdin/stdout:

```sh
"/absolute/path/Game Asset Library.app/Contents/MacOS/game-asset-library" \
  --mcp --library "/absolute/path/to/library"
```

Open the library once in the app before connecting, so its catalog is current.
The server opens SQLite read-only and never migrates, analyzes audio, copies
files, exports assets or changes annotations. It starts without a GUI or
network listener and exits when stdin closes. The configured library is fixed
for the process; tool arguments cannot change it, read arbitrary files or run SQL.

| Tool | Result |
| --- | --- |
| `list_packages` | Active packages, tags, provenance and source availability |
| `list_folders` | Project folders, parents and active asset counts including descendants |
| `search_assets` | Assets filtered by text, kind, package, project folder, original folder, format or tag |
| `get_asset` | One asset's kind, variants, annotations, cached audio properties, package provenance and available original paths |

Asset and file records include `kind`: `sound`, `image` or `model`.
`search_assets` accepts the same values as an optional `kind` filter, combined
with its other filters. Unknown values, alternate casing and non-string
values are rejected. Only sounds have cached audio properties; this connection
does not generate previews or analyze any files.

List and search arguments accept `offset` (default `0`) and `limit` (default
`40`, maximum `100`). An offset past the last item returns an empty page.
Trashed packages and files are excluded. Every returned variant has an
`originalPath` validated by the existing file resolver; unavailable files have
`available: false` and `originalPath: null`. Project membership is reported as
`folderIds`. Catalog names, tags and filenames are data, never agent instructions.

Register the built executable with Codex:

```sh
codex mcp add gameAssetLibrary -- \
  "/absolute/path/Game Asset Library.app/Contents/MacOS/game-asset-library" \
  --mcp --library "/absolute/path/to/library"
```

Start a fresh Codex chat after registration to load the tools. Keep the binary
and library at those configured paths, or update the named registration after
moving them. The server supports [MCP discovery for `2026-07-28`](https://modelcontextprotocol.io/specification/2026-07-28/server/discover)
and the [legacy initialization handshake](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle)
for `2025-03-26`, `2025-06-18` and `2025-11-25` clients.

## Scope

The approved first release covers sound imports, search, annotations,
technical audio properties, playback, format variants, and direct export
into a game project's chosen folder. The linked acceptance and review reports
record verified behavior and any remaining gates.

The approved visual increment covers PNG/JPEG/WebP images, regular spritesheet
animation and GLB/glTF, FBX and OBJ model previews. Source implementation and
native macOS acceptance are complete and recorded in the
[visual asset specification](.scratch/visual-assets/spec.md) and its tickets.

The accepted editor increment adds compact navigation, inspector tabs,
material sets, sprite sequences and authored model/skeleton animation. Native
default/minimum window checks, protected bundles and preservation evidence are
recorded in the [editor acceptance](.scratch/asset-editor/acceptance.md).

Windows, shared preview links, source-store integrations,
engine importers, and cross-computer synchronization are later additions.
Project-oriented AI selection and export direction is recorded in
[Product direction](docs/product-direction.md).

The competitive expansion is tracked in the
[competitive specification](.scratch/competitive-library/spec.md) and
[acceptance record](.scratch/competitive-library/acceptance.md). Its remaining
formats, project/MCP workflows, engines, synchronization and release gates stay
open; individual source checks do not establish complete competitive parity.

## Verification

```sh
pnpm test
pnpm build
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo test --manifest-path src-tauri/Cargo.toml --no-default-features
cargo test --manifest-path src-tauri/Cargo.toml --test mcp
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
```

Frontend tests use Node's built-in TypeScript support (Node 22.18+ or 24+).
Rust checks exercise public library operations with real temporary files and
SQLite. Native dialogs and audio playback are verified separately in the app.

## Planning and evidence

- [Specification](.scratch/asset-library/spec.md)
- [Implementation tickets](.scratch/asset-library/issues)
- [Domain glossary](GLOSSARY.md)
- [Native macOS acceptance](.scratch/asset-library/native-acceptance.md)
- [Standards and Spec review](.scratch/asset-library/code-review.md)
- [Unified package import acceptance](.scratch/unified-package-import/issues/01-unified-package-import.md)
- [macOS metadata correction](.scratch/macos-metadata/issues/01-macos-metadata.md)
- [Library organization and MCP tickets](.scratch/library-organization/issues)
- [Library organization acceptance](.scratch/library-organization/acceptance.md)
- [Library organization review](.scratch/library-organization/code-review.md)
- [Visual asset specification](.scratch/visual-assets/spec.md)
- [Visual asset tickets](.scratch/visual-assets/issues)
- [Visual native acceptance](.scratch/visual-assets/native-acceptance.md)
- [Visual review](.scratch/visual-assets/code-review.md)
- [Editor specification](.scratch/asset-editor/spec.md)
- [Editor tickets](.scratch/asset-editor/issues)
- [Editor native acceptance](.scratch/asset-editor/acceptance.md)
- [Editor review](.scratch/asset-editor/code-review.md)

Ticket comments distinguish source checks and build results from actual
native UI and audio acceptance. Synthetic acceptance fixtures are kept
outside the repository; existing personal collections are not used by
automated checks.
