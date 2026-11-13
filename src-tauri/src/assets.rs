use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AssetKind {
    Sound,
    Image,
    Model,
    Font,
    /// Catalog filter only: model assets whose every variant is confirmed animation-only.
    /// Such assets keep `Model` as their kind.
    Animation,
}

impl AssetKind {
    pub(super) fn formats(self) -> &'static [&'static str] {
        match self {
            Self::Sound => &["WAV", "MP3", "OGG", "FLAC"],
            Self::Image => &["PNG", "JPEG", "WEBP", "PSD", "TGA", "GIF", "ASEPRITE"],
            Self::Model | Self::Animation => &["GLB", "GLTF", "FBX", "OBJ", "BLEND"],
            Self::Font => &["TTF", "OTF", "WOFF", "WOFF2"],
        }
    }

    pub fn for_format(format: &str) -> Option<Self> {
        [Self::Sound, Self::Image, Self::Model, Self::Font]
            .into_iter()
            .find(|kind| {
                kind.formats()
                    .iter()
                    .any(|candidate| candidate.eq_ignore_ascii_case(format))
            })
    }
}

pub(super) fn candidate_format(path: &Path) -> Option<&'static str> {
    if is_macos_metadata(path) {
        return None;
    }
    let extension = path.extension()?.to_str()?;
    let extension = if extension.eq_ignore_ascii_case("ase") {
        "ASEPRITE"
    } else if extension.eq_ignore_ascii_case("jpg") {
        "JPEG"
    } else {
        extension
    };
    [
        AssetKind::Sound,
        AssetKind::Image,
        AssetKind::Model,
        AssetKind::Font,
    ]
    .into_iter()
    .flat_map(AssetKind::formats)
    .copied()
    .find(|format| format.eq_ignore_ascii_case(extension))
}

pub(super) fn is_macos_metadata(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("._"))
        || path.components().any(|part| part.as_os_str() == "__MACOSX")
}
