# Lootbag features in detail

Behavior, limits and edge cases of each feature. The [README](../README.md) has the overview.

## Windows

Windows 10/11 x64 builds on a Windows machine with `pnpm tauri build --bundles nsis`
(unsigned NSIS installer).
It has not been tested on real Windows hardware yet. Differences from macOS:

- Import uses two buttons, `Ordner importieren` and `ZIP/Unity importieren`, with the
  standard dialogs; macOS keeps its single native folder-or-archive picker.
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

Use `Paket importieren` to select a folder, a ZIP or a `.unitypackage` in the
same native dialog. The whole package is copied through the importer; ZIP and
Unity imports first show an extraction-space estimate and confirmation.
Native drops of folders, ZIP files or Unity packages use the same progress,
cancellation and confirmation; up to 32 sources are processed sequentially. A failure
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

## Öffnen und im Finder zeigen

A card's `⋯` menu (or right-click) and the inspector's file menu offer
`Öffnen` (system default app, e.g. Aseprite, Blender or Audacity) and
`Im Finder zeigen` (`Im Explorer zeigen` on Windows). The card uses its
default variant, the inspector the selected variant. Only the catalog file ID
crosses to Rust, which resolves it like a preview: trashed files, files of a
library that was switched meanwhile, offline or changed files and files
without an associated app give a German error. A ZIP entry opens from the ZIP
cache; the app says so, because edits to that copy do not go back into the
ZIP.

## Asset herausziehen

Dragging a card out of the window into Finder (Explorer on Windows), an
engine or a DAW drops a copy of the card's default variant there. Copy is the
only offered operation, so the original in the library or source folder is
never moved. Only the catalog file ID crosses to Rust, which resolves it like
`Öffnen`: ZIP entries are taken from the ZIP cache before the drag starts, and
offline, changed or trashed files show a German error instead of a drag. A
drop back onto Lootbag itself is ignored by the import drop zone.

Dragging a card that is part of the catalog selection drags the whole
selection with all variants; a material set or sprite sequence drags its
member files. Every item is prepared on its own before the drag starts:

- Files whose names are unique are handed over as originals. A repeated name
  (compared case-insensitively) gets a renamed copy such as `Hit-2.wav`.
- A model whose export set (entry, declared resources and detected
  companions, as in the model export) has companions becomes a bundle folder
  named after it, so textures, `.bin` and `.mtl` keep their relative paths.
  A self-contained model, an animation-only FBX and `.blend` drag as the
  plain file. Bundle folders are announced as folders on the pasteboard.
- Incomplete models, offline or trashed files and sources that change during
  preparation are left out and listed in a German notice after the drag; if
  nothing is left, the drag does not start and the notice says why.

On macOS Lootbag owns the gesture natively, because AppKit only starts a drag
from the real mouse event. A Lootbag event monitor records each real mouse
press with an id, its window and the press point in the webview (page zoom 1 is
assumed). Nothing is copied on a mere click: once the pointer moves 6 points
with the button still held, Rust asks that window's page which card lies under
the press point. With the button still down the page shows what was pressed, so
it answers at once from `elementFromPoint` and the current state: the card's
file, or the whole selection if the card is selected, or nothing. It answers
with files only if the card under the point is the card the press landed on
and the results and selection are unchanged since the press; scrolling or a
results refresh during the hold answers nothing. Only the
first answer for the still held press of that window is prepared in the
background; answers for another press or window, late answers after release
and repeated answers are dropped. The next move of that press after the payload
is ready starts the drag from that real drag event.
Releasing the button, any key (Escape, `⌘A`, arrows) and opening another
library disarm the press, so a drag never starts after release or with a stale
selection. WebKit's own image and text dragging is switched off inside cards.
On Windows the drag starts after the pointer moved past the threshold, as
before.

A self-contained model is handed over only if it is still the file whose
companions were checked; a model that changed in between is left out.

Copies and bundles are staged in Lootbag's cache folder (`drag-out`). Copies
handed to a drag stay until the next launch of Lootbag, because a recipient
may still read them long after the drop; switching libraries keeps them.
Copies no drag used (refused, ready too late, never dragged, failed) are
removed right away. All removal runs on a background thread, never inside the
mouse or drag callbacks. The folder holds at most 2 GiB: a drag whose copies
would exceed that is refused with a notice that the cache is full and is
cleared at the next start. A copy or bundle that fails gives its share of the
budget back. Drags of originals need no copies and are not affected.

Lootbag runs as a single instance: a second launch focuses the open window and
quits before it touches the drag-out cache or the catalog. Because the
single-instance check is not atomic on macOS, each launch also takes an
exclusive OS lock on `instance.lock` in the cache folder first and holds it
until it quits; a launch that cannot get it exits at once.

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

## Unity-Pakete ohne Entpacken

`Unity-Paket einbinden` (sidebar, next to `ZIP einbinden`) registers a
`.unitypackage` in place for browsing its supported assets (sounds, images,
models, fonts) as raw files. It is not a Unity project import: prefabs,
materials, scenes and scripts are neither catalogued nor rebuilt, and Unity's
import settings are ignored. The package row shows a `Unity` badge;
`Herkunft` shows `Aus Unity-Paket <package>/<pathname>`.

A Unity package is a gzip tar of `<guid>/asset` payloads with their
`<guid>/pathname`, in any order, so adding one reads the whole package once.
That scan is bounded (64 GiB decompressed, 1,000,000 tar entries, 4 KiB
`pathname` files, 1024-byte paths with at most 255 bytes per name, 30 minutes)
and rejects the whole package for links or special entries, a GUID with two
payloads or pathnames, a payload without pathname, a supported file without
payload, absolute, `.`/`..`, backslash, drive, UNC or stream (`:`) paths,
Windows device names (`CON`, `nul.wav`, `COM1`, …), names ending in a dot or
space, reserved or control characters, and paths that collide by case or as
file and folder. A GUID with a pathname but no payload is a folder.

Identity, offline state, relinking (choose the moved original), the extraction
cache (`<library>/.cache/unity/<key>/<pathname>`, size and CRC checked), trash
cleanup, duplicate hashing, model traits, previews, exports, model companions
and project exports work as for ZIP packages. The GUIDs found by the scan are
stored with the files, so a cache miss reads the package front to back once and
extracts every requested model companion in that same pass; such extractions run
one at a time, and a second request for the same file finds the first one's
copy. Background passes read the package forward in catalog order without
filling the cache. License and README hints are not read from Unity packages.

Cold reads are proportional to the payload's position in the package, because
gzip cannot seek. Measured with the ignored `cold_read_latency` test (release
build, Apple M5 Max, 4096 payloads of 256 KiB, half incompressible and half
low-entropy, 1 GiB decompressed / 795 MiB on disk): adding the package took
1.8 s, the cold preview of the first payload 2 ms, of the last one 1.7 s, and a
cached preview 0.1 ms.

### Unity-Pakete importieren

`Paket importieren` (or a drop) with a `.unitypackage` is a raw asset
extraction into library storage, labelled so in the confirmation and the
import notice. The package gets the same full scan, bounds and path checks as
above; every payload is then written at its Unity `pathname` (folder structure
kept, model companions like `.bin` included) in one forward pass, each checked
against the scanned size and CRC. `.meta` files, scripts (`.cs`, `.js`, `.boo`,
`.dll`, `.asmdef`, `.asmref`) and Unity YAML (payloads starting with `%YAML`:
prefabs, materials, scenes, …) are skipped; the confirmation and the import
notice give their count. Space estimate, progress and cancellation work as for
ZIP imports; the scan from the estimate is reused by the import.

## Images, sprites and models

New imports and external references discover PNG/JPEG/WebP/TGA/GIF/Aseprite images and
GLB/glTF 2.0, FBX, OBJ and Blender source models alongside sounds. Existing packages keep their
catalog entries until rescanned (`Neu einlesen`) or reimported. Search and category filters work
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
Managed packages discover previously uncatalogued TGA images with `Neu
einlesen`; external and ZIP sources require reimport.

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
Managed packages and external folders discover GIF files with `Neu einlesen`;
ZIP sources require reimport.

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
Managed packages and external folders discover these sources with `Neu einlesen`;
ZIP sources require reimport.

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
preview lifecycle; no Blender process is run by the app. Managed packages
and external folders discover previously ignored `.blend` files with `Neu einlesen`;
ZIP sources require reimport.

GLB/glTF, FBX and OBJ cards generate actual
static thumbnails serially with one renderer and a bounded session cache;
background thumbnails do not change the inspector's export selection.
PSD files are recognized as image assets by new folder, ZIP and external
imports, and retain their original bytes on export. PSD image decoding is
pending approval of the decoder dependency. Existing packages are deliberately
not rescanned on reopen; `Neu einlesen` discovers previously ignored PSD files in
managed packages and external folders, ZIP sources require reimport.
The tested Synty Town prop now renders with its supplied PNG; the City Biker
retains its regular preview and export. Geometry and source files are preserved.

## Schriften

TTF, OTF, WOFF and WOFF2 files are the asset kind **Schriften**, found by new
folder, external-folder and ZIP imports. Font format variants with the same name
group into one asset; a sound with the same name stays a separate asset. Cards
render "Aa Bb 123" in the font, loaded only while the card is visible and
unloaded when it scrolls away; the inspector shows an editable sample text at
12–72 px. Fonts over 32 MiB or unreadable fonts show "Keine Vorschau" on their
own card. Metadata is the filename only (no family/style parsing). Export and
project export copy the original bytes. Existing packages are not rescanned;
previously ignored fonts need a reimport.

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

## Waveforms on sound cards

In the card view, sound cards show a small waveform of their first file variant,
so one-shots and ambiences are recognizable without playing them. Only cards
scrolled into view are analysed, one at a time, with the same limits as the
inspector waveform (WAV/PCM, MP3, OGG/Vorbis, native FLAC, at most 512 MiB and
30 seconds). Cards scrolled away before their turn are skipped; switching the
library cancels the queue. The inspector's own waveform runs separately and is
never cancelled by cards.

Results, including failures, are kept in the catalog per source state, so an
unchanged sound is not decoded again; a changed file or ZIP archive is analysed
anew, and a result whose source changed during analysis is discarded. A ZIP entry
is read through a temporary copy that is deleted right after analysis. Emptying
the trash removes the stored waveform with its file. A failed card shows
`Keine Vorschau` with the reason as tooltip. This adds catalog format 4; older
catalogs are upgraded when opened, and older app versions cannot open it again.

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

## Neu einlesen

`Neu einlesen` in a package's `⋯` menu scans the package's files again with the
current discovery rules: a managed package's folder in the library storage (never
the original import location), an external folder at its registered location. It reports "n neu, n geändert, n fehlen". A file at the same
package path keeps its asset and file identity, so names, tags, package info,
project memberships, roles and chosen variants stay, also when its content
changed. Changes are detected by size and a scan stamp stored per file in the
catalog, independent of hashes: modification time (plus the file's inode where
the platform has one) for managed files, the recorded modification time for
external folders. Analysed audio properties, model traits, hashes and card
waveforms of changed files are dropped and recomputed, auto-tags and duplicates
follow in the background. Catalogs from before format 6 have no stamp for
managed files: their first rescan records one and, to be safe, drops and
recomputes those files' derived data, but does not count them as "geändert",
because an earlier change cannot be told from no change. Background passes
(model traits, hashes) that read a file before a rescan changed it discard
their result instead of storing it, and a cancelled pass stores nothing; the
auto-tag pass reads names and writes tags in one catalog transaction. After a
manual or watched rescan, catalog thumbnails, card waveforms and the
inspector preview are rendered again, also for files whose size stayed the same. A renamed or moved file counts as missing plus new. Missing files keep
their catalog entry and annotations and show as unavailable until they return or
are moved to the trash; trashed files stay in the trash. New files join an
existing asset only when exactly one asset of the same folder, name and kind is
still an unchanged automatic group, so split or reassigned variants stay as they
are. Material sets and sprite sequences follow automatically; groups that become
ambiguous are named in the result. A link, special file or unreadable folder
aborts the scan and leaves the package unchanged. An external folder is rescanned
only while it is reachable and still the registered folder: an offline source or
a different folder at the same path is refused and changes nothing. The rescan
accepts updated files at that location, so they open again; moving the source
still needs `Quelle neu verknüpfen` with the original folder. A ZIP source is
rescanned from the archive now at its registered location, for example after the
pack was updated there: the whole archive is checked with the same rules as when
it was added (unsafe, encrypted or colliding paths, unreadable archives), and an
invalid, missing or meanwhile changed archive is refused and changes nothing.
Entries are matched by path; changes are detected by size and CRC. The package's
extracted cache copies are removed, so previews extract the new entries again.
Unity packages cannot be rescanned yet: `Neu einlesen` refuses them with a
message and changes nothing; add the updated package anew instead.

### Ordner beobachten

`Ordner beobachten` in an external folder's `⋯` menu (off by default, stored
per source in the catalog) checks the folder when the library is opened and
when the app regains focus (at most once a minute): a recursive listing of
names, sizes and modification times, without hashing. Only when that listing
differs from the last one seen does `Neu einlesen` run in the background, one
check at a time, and a notice reports "Neue Dateien in <Paket>: n neu, n
geändert, n fehlen". The same state is never reported twice. An offline
folder is skipped silently; a different folder at the registered path or an
incomplete listing (for example missing permissions) is reported once and
changes nothing. Switching the library cancels a running check. ZIP sources
and Unity packages are not watched yet.

## Paketinfos und Credits

`Paketinfo bearbeiten…` in a package's `⋯` menu (or in the inspector's
`Herkunft` tab) records where a package came from: Shop, Link, Lizenz,
Urheber, Gekauft am, Version and Notizen. All fields are optional free text
(at most 200 characters, notes 2000); a link must start with `http://` or
`https://`, the date is `YYYY-MM-DD`. The info is stored per package in the
catalog and deleted with the package when the trash is emptied. `Herkunft`
shows shop, link (selectable, with `Link kopieren`), licence and author, or
"Keine Lizenz hinterlegt". The dialog lists licence and readme files found in
the package root and its direct subfolders (`LICENSE*`, `*license*.txt/md/pdf`,
`README*`) for imported packages, external folders and ZIPs (only while the
source is still the catalogued one; links are not followed, and an imported
package's folder is read, or rescanned, only while it and every folder up to the
library root are real folders, not links). Plain-text ones
(`.txt`, `.md`, no extension; first 64 KB) are read for licence names: CC0,
CC BY 3.0/4.0, CC BY-SA 3.0/4.0 and MIT, by explicit name with version or the
canonical licence URL. Each match is shown as a suggestion with its file and
line, e.g. "CC0 1.0 – aus License.txt: »…«"; `Übernehmen` fills the Lizenz
field only while it is empty. Several licences are listed separately, never
merged. Generic wording ("royalty free"), a shop name or a versionless
"Creative Commons Attribution" is not recognized; PDFs stay hints; nothing is
rendered or fetched.

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
