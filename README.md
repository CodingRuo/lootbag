# Lootbag

Lootbag is a local game development asset library for sounds, images and
models on macOS. It is free software under the GPL-3.0-or-later.
The current editor increment is implemented on `codex/asset-editor`.
Source checks, native acceptance, and final review gates are recorded below.

## Development

The application uses Tauri 2, React, TypeScript, and a local SQLite catalog.
Install Node.js, pnpm, Rust, and the macOS Command Line Tools before building.
The app targets macOS 15.4 or later for the accepted OGG/Vorbis preview path.
Native acceptance runs on Apple Silicon; see [Windows](#windows) for its status.

```sh
pnpm install --frozen-lockfile
pnpm tauri dev
```

Create a production app bundle with:

```sh
pnpm tauri build --bundles app
```

Signed and notarized macOS release (runs locally, needs a Developer ID certificate
in the keychain and a one-time `xcrun notarytool store-credentials lootbag`):

```sh
scripts/release-macos.sh
```

## Windows

Windows 10/11 x64 builds on a Windows machine with `pnpm tauri build --bundles nsis`
(unsigned NSIS installer).
It has not been tested on real Windows hardware yet. Differences from macOS:

- Import uses two buttons, `Ordner importieren` and `ZIP importieren`, with the
  standard dialogs; macOS keeps its single native folder-or-ZIP picker.
- External folders and ZIPs are identified by volume serial number plus NTFS file
  index (`win-volume:…`) instead of the macOS volume UUID and inode. On ReFS only
  the legacy 64-bit part of the file ID is used.
- Free space comes from `GetDiskFreeSpaceExW` instead of `df`.
- OGG/Vorbis preview uses WebView2, which plays it natively; the macOS 15.4
  minimum does not apply.
- Catalog paths always use `/`; ZIP entries and project folder names containing
  `\`, `:` (drive letters, UNC paths, NTFS streams) are rejected on every system.

## Library storage

Choose a library folder or let the app create its default `~/Lootbag Library`.
Libraries created before the rename (`~/Game Asset Library`) open via
`Bibliothek öffnen`. The app reopens the last library on the next launch while its
folder and catalog still exist. Managed imports copy package contents into library storage,
preserving original folders and companion files. Referenced sources retain
their files at the external location. The catalog stores names, tags, and
provenance without renaming original files.

Use `Paket importieren` to select either a folder or a ZIP in the same native
dialog. The whole package is copied through the importer; ZIP imports first show
an extraction-space estimate and confirmation.
Native drops of folders or ZIP files use the same progress, cancellation
and ZIP confirmation; up to 32 sources are processed sequentially. A failure
stops the queue and reports any earlier successful imports.
macOS companion files (`._*` and files inside `__MACOSX`) remain in the
package storage but do not appear as assets. Existing catalogs are corrected
once on open, after creating a `catalog-before-macos-metadata-*.sqlite3` backup;
the corrected catalog is marked as format version 2, which older app builds
cannot open. Opening a library also removes staging folders left by an
interrupted import. Text search covers names, package, folder, filename,
tags and auto-tags, not the absolute path the package was imported from.

Close the app before manually copying or moving the entire library.
Managed locations are relative to the library root. External sources can
require relinking after moving to another computer.
Relinking validates the original source. Add a copied folder with different
file identities as a new external source.

## ZIP-Pakete ohne Entpacken

`ZIP einbinden` (sidebar, next to `Externen Ordner einbinden`) registers a ZIP
archive in place instead of extracting it. Only the archive directory is read:
entry names, sizes and CRCs are catalogued with the same asset, variant and
group rules as a folder import, and nothing is written to the library besides
catalog rows. Absolute or `..` paths, backslashes, symlink/special entries and
encrypted entries reject the whole archive. The package row shows a `ZIP`
badge; `Herkunft` shows `Aus ZIP <archive>/<entry>`.

The archive identity is its file identity plus size and modification time. A
moved archive shows as offline and relinks like an external folder (choose the
moved original ZIP); a rewritten archive or a copy is refused and must be
added as a new ZIP.

Previews, exports, audio analysis and model previews extract the needed entry
on demand into `<library>/.cache/zip/<key>/<entry path>`, checking size and CRC
and the free space first; later uses reuse that copy. A model preview also
extracts the archive's images, `.bin` and `.mtl` files once so companions
resolve as in a folder; model exports and project exports copy the same bytes.
Duplicate hashing streams entries from the archive, and the model-trait pass
reads each model through a temporary file that is deleted at once, so
background jobs do not fill the cache. Unavailable archives are retried on the
next run, unreadable entries store an error.

The cache is disposable and never holds originals: it is dropped for a package
whose archive changed, removed when the package is deleted from the trash (the
archive itself is never touched), and deleted completely on library open when
it exceeds 4 GiB. It can be deleted by hand while the app is closed and is not
needed in backups. To turn a ZIP package into a managed one, import the same
ZIP with `Paket importieren`.

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

After a library is opened, imported or relinked, a background pass reads only
the headers of model files (binary and ASCII FBX 7.x, GLB/glTF; OBJ counts as
mesh) and records whether each has mesh geometry and keyed animation. An asset
whose every variant has keyed curves and no mesh is listed under "Animationen"
instead of "3D-Modelle"; mixed, unreadable and Blender files stay models.
Mesh files with an empty "Take 001" stack are not animations.
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
Image/model technical filters remain pending, alongside
native macOS/Windows acceptance.

## Projekte exportieren

A project folder is a game-project selection: each membership can carry a
free-text role (for example `Wald-Ambience`, `Held`, `Inventar-Icon`, at most
80 characters) and, for assets with several format variants, the variant to
export. Edit both in the inspector under `Eigenschaften › Projektordner`;
while a project folder is the catalog filter, roles appear as chips on cards
and as a column in the list. A chosen variant that is later trashed or moved
to another asset falls back to the default (first) variant. Opening a library
upgrades the catalog to format version 3.

`Projekt exportieren …` in the catalog heading (shown while a project folder
is selected) asks for a destination folder, then lists unavailable members,
the number of texture substitutions and of packages without a licence entry
before anything is copied; continue or cancel. The export creates
a new `<destination>/<folder name>` (an existing folder, the library and
external sources are refused), mirrors subfolders as directories and never
changes library originals. glTF/GLB models take their declared buffers and
images; FBX/OBJ take package textures and `.mtl` files whose file names appear
in the model or its material file (a same-named PNG may stand in for a PSD
reference; the model still names the original, and the asset's manifest
`notes` say so). Material sets and sprite sequences are
exported as the member asset's own file, not the whole group. Duplicate names
get `-2`, `-3`, … `manifest.json` (`manifest-2.json` if a subfolder already
has that name) records project, `exportedAt`, every asset
(id, name, role, kind, package, original package path, variant, exported files,
`license`, `author`, `store`, `url` from the package info, `notes`) and the skipped members. A failed copy keeps the
partial folder and names it in the error ("Export unvollständig: …"); nothing
in it is deleted. The copy runs in one background
step without progress or cancellation. Game-project profiles are not built yet.

## Paketinfos und Credits

`Paketinfo bearbeiten…` in a package's `⋯` menu (or in the inspector's
`Herkunft` tab) records where a package came from: Shop, Link, Lizenz,
Urheber, Gekauft am, Version and Notizen. All fields are optional free text
(at most 200 characters, notes 2000); a link must start with `http://` or
`https://`, the date is `YYYY-MM-DD`. The info is stored per package in the
catalog and deleted with the package when the trash is emptied. `Herkunft`
shows shop, link (selectable, with `Link kopieren`), licence and author, or
"Keine Lizenz hinterlegt". For imported packages the dialog lists licence and
readme files found in the package root and its direct subfolders
(`LICENSE*`, `*license*.txt/md/pdf`, `README*`); nothing is read or filled in
automatically, and external sources are not scanned.

Project exports also write `CREDITS.md` next to `manifest.json` (`CREDITS-2.md`
if a subfolder has that name): one section per package with Urheber, Lizenz
(or "nicht hinterlegt – bitte prüfen"), Shop/Link and the exported asset names.

## Suche, Auto-Tags und gespeicherte Suchen

The catalog keeps an SQLite FTS5 index of each asset's name, filenames,
folders, package name, user, package and auto-tags. Triggers update it in the
same transaction as every import, rename, tag edit or regrouping; trashed
assets stay indexed but are filtered out by the catalog query. Each typed word
is a prefix, and all words must match: `barr nuk` finds
`SM_Prop_Barrel_Nuke_01`. A query containing other characters than letters and
digits (`SM_Prop`, `A001.wav`) uses the previous substring search. Prefix
search does not find word middles; camelCase parts come in via auto-tags.
Type/package/folder/format/tag/duplicate/audio filters combine with both.
`/` or Command/Control-F focuses the search box. On the 6,968-asset test library
`barrel`, `crouch fem` and `bite` take under 1 ms instead of about 8 ms with the
substring search; building the index on first open takes about 25 ms and the
auto-tag pass about 1.5 s in the background.

Auto-tags are derived without reading files, after opening and after each
import: filename stems and the original folder path are split on `_ - . /`
spaces and letter/digit changes; whole segments come first (filename, then
folders deepest first), then camelCase parts (`RootMotion` → `rootmotion`,
`root`, `motion`). Numbers, tokens under three letters and a short stopword list
(`tex`, `mod`, `lod`, `the`, `fbx`, `png` …) are dropped; at most 8 per asset. They live
in their own table, so user tags stay authoritative. The inspector shows them
muted below the tags; clicking one adds it to the user tags. Cards do not show them.

`Suche speichern` next to the search box stores the current text and filters
under a name (same name replaces) in the catalog, so saved searches travel
with the library. `Gespeicherte Suchen` in the sidebar applies or deletes them.

## Duplikate

Bundles often ship the same model, texture or sound in several packs. After a
library is opened and after imports, references and relinks, a background job
hashes (SHA-256, streamed) every active file whose size matches another active
file; unique sizes and empty files are skipped, offline files are retried on
the next run, read errors are stored. Files with equal size and hash in
different assets count as duplicates. Cards show a `2×` chip, `Filter` →
`Duplikate (n)` lists only assets with a copy, and the inspector's `Herkunft`
tab lists `Gleicher Inhalt (Stand der letzten Prüfung):` with package and path;
clicking a copy opens it. A hash belongs to the file's size and modification
time (for ZIP entries: the archive's identity). Files are checked against that
when the library is opened (every hashed file, at the start of each hashing
run), when `Duplikate` is turned on (every duplicate group member: about 10 ms
for the 1,029 members of the test library below, about 0.2 s for 20,000 members
among 100,000 files) and when the copy list opens (the asset's files and their
copies). Changed files leave their group, counts and results refresh, and a
hashing run starts right away to hash them again; offline files keep their
hash. An edit that keeps both size and modification time is not noticed.
Nothing is deleted automatically – use the trash to remove copies you do not
need. On a 6,968-file test library (2.5 GB) 3,439 candidates (370 MB) hash in
about 2 s and form 513 groups.

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

Native Windows acceptance, shared preview links, source-store integrations,
engine importers, and cross-computer synchronization are later additions.
Project-oriented AI selection and export direction is recorded in
[Product direction](docs/product-direction.md).

The competitive expansion is tracked in the
[competitive specification](.scratch/competitive-library/spec.md) and
[acceptance record](.scratch/competitive-library/acceptance.md). Its remaining
formats, project workflows, engines, synchronization and release gates stay
open; individual source checks do not establish complete competitive parity.

## Verification

```sh
pnpm test
pnpm build
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo test --manifest-path src-tauri/Cargo.toml --no-default-features
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
- [Library organization tickets](.scratch/library-organization/issues)
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

## License

Lootbag is licensed under the GNU General Public License v3.0 or later
(`GPL-3.0-or-later`). See [LICENSE](LICENSE). Contributions are welcome; see
[CONTRIBUTING.md](CONTRIBUTING.md).
