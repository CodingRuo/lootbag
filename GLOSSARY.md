# Lootbag

A personal library of resources used to develop games.

## Language

**Asset**:
A game development resource, such as a sound or a 3D model. An asset may include multiple format variants and associated files needed to use it.
_Avoid_: File (when referring to the complete resource)

**Asset kind**:
The resource category of an asset: sound, image, model, or font. Format variants of
one asset belong to the same kind.
_Avoid_: Format (when referring to the resource category)

**Asset file**:
An individual file belonging to an asset, such as an audio file, a model file, or a required texture.
_Avoid_: Asset (when referring to one file of a multi-file asset)

**Format variant**:
An alternative representation of the same asset in another file format, such as WAV and OGG versions of one sound.
_Avoid_: Duplicate (when referring to an alternative format of the same asset)

**Tag**:
A user-assigned term describing an asset's content or intended use in a game, such as river, ambience, or footsteps. Tags can also describe an asset package.
_Avoid_: File type (when describing the content or intended use of a resource)

**Material set**:
A related set of texture maps describing one surface, such as its base color,
normal, roughness or height. Its maps retain their individual asset identities
and source provenance.
_Avoid_: Format variant (when referring to maps with different surface roles)

**Sprite sequence**:
Separate numbered images representing successive frames of one sprite animation.
_Avoid_: Spritesheet (when referring to separate frame files)

**Animation clip**:
A named motion authored for the model or skeleton supplied with a 3D asset.
_Avoid_: Retargeted animation (when compatibility with another skeleton is unknown)

**Library**:
A user's collection of game development assets, their descriptions, and their tags. A library can contain both managed assets and referenced assets.
_Avoid_: Folder (when referring to the collection as a whole)

**Catalog**:
The searchable records describing a library's assets and packages, including names, tags, provenance, technical properties, and file locations.
_Avoid_: Library (when referring only to its descriptive records)

**Managed asset**:
An asset whose files have been imported into the library's own storage.
_Avoid_: Uploaded asset (when implying storage on a remote service)

**Referenced asset**:
An asset whose files remain at an external source and are included in the library by reference.
_Avoid_: Managed asset

**External source**:
A location outside the library's own storage containing game development assets, such as an existing folder, a storage drive, or a synchronized cloud folder.
_Avoid_: Asset shop (when referring to an existing storage location)

**Asset package**:
A named collection of assets brought into the library together, retaining their source folder hierarchy and accompanying files.
_Avoid_: Asset (when referring to a collection of resources)

**Asset name**:
A human-readable name identifying an asset. It may differ from the names of that asset's files.
_Avoid_: Original filename (when referring to a manually assigned asset name)

**Provenance**:
Information about an asset's origin and its place within its source package, including original filenames and folder hierarchy.
_Avoid_: Asset name (when referring to the asset's origin)

**Project folder**:
A user-created folder grouping existing assets for a game project. It can have
subfolders and an asset can belong to multiple project folders. Membership does
not copy files or change their original package paths.
_Avoid_: Original folder (when referring to a user-created project grouping)

**Original folder**:
The immutable location of an asset file within its source package.
_Avoid_: Project folder (when describing file provenance)

**Trash**:
Recoverable catalog entries for deleted packages or asset files. Trashed items
are hidden from normal search and use; their files and annotations remain
available for restoration.
_Avoid_: Permanent deletion (when moving an entry to the trash)
