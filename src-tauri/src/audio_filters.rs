use super::{AssetKind, CatalogQuery, Result};

#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct AudioCriteria {
    pub min_duration_seconds: Option<f64>,
    pub max_duration_seconds: Option<f64>,
    pub channels: Option<String>,
    pub analysis: Option<String>,
}

impl AudioCriteria {
    pub(super) fn validate(&self, query: &CatalogQuery) -> Result<bool> {
        if [self.min_duration_seconds, self.max_duration_seconds]
            .into_iter()
            .flatten()
            .any(|value| !value.is_finite() || value < 0.0)
            || matches!((self.min_duration_seconds, self.max_duration_seconds), (Some(min), Some(max)) if min > max)
        {
            return Err("Dauer muss endlich, nicht negativ und von ≤ bis sein".into());
        }
        if !matches!(
            self.channels.as_deref().unwrap_or("any"),
            "any" | "mono" | "stereo" | "multi"
        ) || !matches!(
            self.analysis.as_deref().unwrap_or("any"),
            "any" | "notAnalysed" | "ready" | "error"
        ) {
            return Err("Ungültiger Audiofilter".into());
        }
        let active = self.min_duration_seconds.is_some()
            || self.max_duration_seconds.is_some()
            || self.channels.as_deref().is_some_and(|value| value != "any")
            || self.analysis.as_deref().is_some_and(|value| value != "any");
        if active
            && (query.kind.is_some_and(|kind| kind != AssetKind::Sound) || query.workflow.is_some())
        {
            return Err("Audiofilter benötigen Sounds ohne Bild-Workflow".into());
        }
        Ok(active)
    }
}

// Every technical predicate refers to the same file as the existing catalog predicates.
pub(super) const FILTER: &str = "
    AND ((?10 IS NULL AND ?11 IS NULL AND ?12 = 'any') OR EXISTS (
        SELECT 1 FROM audio_metadata m WHERE m.file_id = f.id AND m.error IS NULL
        AND (?10 IS NULL OR (m.duration_seconds >= ?10 AND m.duration_seconds <= 1.7976931348623157e308))
        AND (?11 IS NULL OR (m.duration_seconds >= 0 AND m.duration_seconds <= ?11))
        AND (?12 = 'any' OR (?12 = 'mono' AND m.channels = 1)
            OR (?12 = 'stereo' AND m.channels = 2) OR (?12 = 'multi' AND m.channels > 2))))
    AND (?13 = 'any'
        OR (?13 = 'notAnalysed' AND NOT EXISTS (SELECT 1 FROM audio_metadata m WHERE m.file_id = f.id))
        OR EXISTS (SELECT 1 FROM audio_metadata m WHERE m.file_id = f.id
            AND ((?13 = 'ready' AND m.error IS NULL) OR (?13 = 'error' AND m.error IS NOT NULL))))";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::Library;
    use std::fs;

    fn wav(channels: u16, seconds: u32) -> Vec<u8> {
        let size = 8_000 * u32::from(channels) * 2 * seconds;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + size).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&channels.to_le_bytes());
        bytes.extend_from_slice(&8_000_u32.to_le_bytes());
        bytes.extend_from_slice(&(8_000 * u32::from(channels) * 2).to_le_bytes());
        bytes.extend_from_slice(&(channels * 2).to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&size.to_le_bytes());
        bytes.resize(44 + size as usize, 0);
        bytes
    }

    #[test]
    fn saved_audio_filters_distinguish_missing_error_partial_and_offline_values_and_hide_trash() {
        let root = std::env::temp_dir().join(format!("audio-filter-states-{}", std::process::id()));
        fs::create_dir_all(root.join("source")).unwrap();
        fs::create_dir_all(root.join("external")).unwrap();
        let mp3 = include_bytes!("../tests/fixtures/audio/stereo44100.mp3");
        for (name, bytes) in [
            ("Missing.wav", wav(1, 1)),
            ("Error.wav", b"corrupt sound".to_vec()),
            ("Partial.mp3", mp3.to_vec()),
            ("Multi.wav", wav(3, 1)),
            ("Ready.wav", wav(1, 2)),
        ] {
            fs::write(root.join("source").join(name), bytes).unwrap();
        }
        fs::write(root.join("external/Offline.mp3"), mp3).unwrap();
        let mut library = Library::create(&root.join("library")).unwrap();
        library.import_folder(&root.join("source")).unwrap();
        let external = library.reference_folder(&root.join("external")).unwrap();
        let originals = library.query(CatalogQuery::default()).unwrap();
        for asset in &originals.items {
            if asset.name != "Missing" {
                library.analyze_audio(asset.files[0].id).unwrap();
            }
        }
        let partial = originals
            .items
            .iter()
            .find(|asset| asset.name == "Partial")
            .unwrap();
        // Existing catalogs can contain ready rows with individual properties unavailable.
        library.connection.execute("UPDATE audio_metadata SET duration_seconds = NULL, channels = NULL WHERE file_id = ?1", [partial.files[0].id]).unwrap();
        fs::rename(root.join("external"), root.join("disconnected")).unwrap();
        for (analysis, expected) in [("any", 6), ("notAnalysed", 1), ("ready", 4), ("error", 1)] {
            let page = library
                .query_with_audio(
                    CatalogQuery::default(),
                    Some(AudioCriteria {
                        analysis: Some(analysis.into()),
                        ..Default::default()
                    }),
                )
                .unwrap();
            assert_eq!(page.total, expected, "{analysis}");
        }
        let bounded = || {
            Some(AudioCriteria {
                min_duration_seconds: Some(0.0),
                ..Default::default()
            })
        };
        let page = library
            .query_with_audio(CatalogQuery::default(), bounded())
            .unwrap();
        assert_eq!(
            page.items
                .iter()
                .map(|asset| asset.name.as_str())
                .collect::<Vec<_>>(),
            ["Multi", "Offline", "Ready"]
        );
        assert!(
            !page
                .items
                .iter()
                .find(|asset| asset.name == "Offline")
                .unwrap()
                .files[0]
                .available
        );
        let partial_page = library
            .query_with_audio(
                CatalogQuery {
                    asset_id: Some(partial.id),
                    ..Default::default()
                },
                Some(AudioCriteria {
                    analysis: Some("ready".into()),
                    ..Default::default()
                }),
            )
            .unwrap();
        assert_eq!(partial_page.total, 1);
        assert!(partial_page.items[0].files[0]
            .audio
            .as_ref()
            .unwrap()
            .duration_seconds
            .is_none());
        let multi = library
            .query_with_audio(
                CatalogQuery::default(),
                Some(AudioCriteria {
                    channels: Some("multi".into()),
                    ..Default::default()
                }),
            )
            .unwrap();
        assert_eq!(
            multi
                .items
                .iter()
                .map(|asset| asset.name.as_str())
                .collect::<Vec<_>>(),
            ["Multi"]
        );
        let stereo = library
            .query_with_audio(
                CatalogQuery::default(),
                Some(AudioCriteria {
                    channels: Some("stereo".into()),
                    ..Default::default()
                }),
            )
            .unwrap();
        assert_eq!(
            stereo
                .items
                .iter()
                .map(|asset| asset.name.as_str())
                .collect::<Vec<_>>(),
            ["Offline"]
        );
        library.trash_file(multi.items[0].files[0].id).unwrap();
        library.trash_package(external.id).unwrap();
        assert_eq!(
            library
                .query_with_audio(CatalogQuery::default(), bounded())
                .unwrap()
                .items
                .iter()
                .map(|asset| asset.name.as_str())
                .collect::<Vec<_>>(),
            ["Ready"]
        );
        drop(library);
        let library = Library::open_read_only(&root.join("library")).unwrap();
        assert_eq!(
            library
                .query_with_audio(
                    CatalogQuery::default(),
                    Some(AudioCriteria {
                        analysis: Some("notAnalysed".into()),
                        ..Default::default()
                    })
                )
                .unwrap()
                .total,
            1
        );
        assert_eq!(
            fs::read(root.join("disconnected/Offline.mp3")).unwrap(),
            mp3
        );
        drop(library);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn saved_audio_filters_reject_invalid_ranges_modes_and_contradictory_workflows() {
        let root =
            std::env::temp_dir().join(format!("audio-filter-invalid-{}", std::process::id()));
        let library = Library::create(&root).unwrap();
        for criteria in [
            AudioCriteria {
                min_duration_seconds: Some(-1.0),
                ..Default::default()
            },
            AudioCriteria {
                max_duration_seconds: Some(f64::INFINITY),
                ..Default::default()
            },
            AudioCriteria {
                min_duration_seconds: Some(f64::NAN),
                ..Default::default()
            },
            AudioCriteria {
                min_duration_seconds: Some(3.0),
                max_duration_seconds: Some(2.0),
                ..Default::default()
            },
            AudioCriteria {
                channels: Some("Mono".into()),
                ..Default::default()
            },
            AudioCriteria {
                analysis: Some("pending".into()),
                ..Default::default()
            },
            AudioCriteria {
                channels: Some("stereo' OR 1=1 --".into()),
                ..Default::default()
            },
        ] {
            assert!(library
                .query_with_audio(CatalogQuery::default(), Some(criteria))
                .is_err());
        }
        for query in [
            CatalogQuery {
                kind: Some(AssetKind::Image),
                ..Default::default()
            },
            CatalogQuery {
                kind: Some(AssetKind::Model),
                ..Default::default()
            },
            CatalogQuery {
                workflow: Some(crate::library::ImageWorkflow::Material),
                ..Default::default()
            },
            CatalogQuery {
                workflow: Some(crate::library::ImageWorkflow::SpriteSequence),
                ..Default::default()
            },
        ] {
            assert!(library
                .query_with_audio(
                    query,
                    Some(AudioCriteria {
                        min_duration_seconds: Some(0.0),
                        ..Default::default()
                    })
                )
                .is_err());
        }
        assert!(serde_json::from_str::<AudioCriteria>(r#"{"sampleRate":44100}"#).is_err());
        drop(library);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn saved_audio_filters_require_one_qualifying_variant_with_composed_provenance_and_annotations()
    {
        let root =
            std::env::temp_dir().join(format!("audio-filter-variants-{}", std::process::id()));
        fs::create_dir_all(root.join("source/long")).unwrap();
        fs::create_dir_all(root.join("source/short")).unwrap();
        let mono = wav(1, 2);
        let stereo = include_bytes!("../tests/fixtures/audio/stereo44100.mp3");
        fs::write(root.join("source/long/Long.wav"), &mono).unwrap();
        fs::write(root.join("source/short/Short.mp3"), stereo).unwrap();
        let mut library = Library::create(&root.join("library")).unwrap();
        let package = library.import_folder(&root.join("source")).unwrap();
        let page = library.query(CatalogQuery::default()).unwrap();
        let long = page
            .items
            .iter()
            .find(|asset| asset.name == "Long")
            .unwrap();
        let short = page
            .items
            .iter()
            .find(|asset| asset.name == "Short")
            .unwrap();
        let wav_id = long.files[0].id;
        let mp3_id = short.files[0].id;
        library.analyze_audio(wav_id).unwrap();
        library.analyze_audio(mp3_id).unwrap();
        library.assign_variant(wav_id, short.id).unwrap();
        library
            .annotate_asset(short.id, "Effect", &["asset-tag".into()])
            .unwrap();
        library
            .annotate_package(package.id, &["package-tag".into()])
            .unwrap();
        let parent = library.create_folder("Game", None).unwrap();
        let child = library.create_folder("Sounds", Some(parent.id)).unwrap();
        library.add_to_folder(child.id, short.id).unwrap();
        let public = serde_json::to_value(library.query(CatalogQuery::default()).unwrap()).unwrap();
        assert_eq!(public["items"][0]["files"][0]["id"], mp3_id);
        let incompatible = library
            .query_with_audio(
                CatalogQuery::default(),
                Some(AudioCriteria {
                    min_duration_seconds: Some(1.0),
                    channels: Some("stereo".into()),
                    ..Default::default()
                }),
            )
            .unwrap();
        assert_eq!(
            incompatible.total, 0,
            "duration and channels cannot be borrowed from different variants"
        );
        let mono_filter = || {
            Some(AudioCriteria {
                min_duration_seconds: Some(1.0),
                max_duration_seconds: Some(2.0),
                channels: Some("mono".into()),
                analysis: Some("ready".into()),
            })
        };
        for query in [
            CatalogQuery {
                format: Some("MP3".into()),
                ..Default::default()
            },
            CatalogQuery {
                original_folder: Some("short".into()),
                ..Default::default()
            },
            CatalogQuery {
                text: "Short.mp3".into(),
                ..Default::default()
            },
        ] {
            assert_eq!(
                library
                    .query_with_audio(query, mono_filter())
                    .unwrap()
                    .total,
                0
            );
        }
        for tag in ["asset-tag", "PACKAGE-TAG"] {
            let matched = library
                .query_with_audio(
                    CatalogQuery {
                        package_id: Some(package.id),
                        folder_id: Some(parent.id),
                        original_folder: Some("long".into()),
                        format: Some("wav".into()),
                        text: "Long.wav".into(),
                        tag: Some(tag.into()),
                        ..Default::default()
                    },
                    mono_filter(),
                )
                .unwrap();
            assert_eq!(matched.total, 1);
            assert_eq!(
                matched.items[0]
                    .files
                    .iter()
                    .map(|file| file.id)
                    .collect::<Vec<_>>(),
                [wav_id, mp3_id]
            );
            let export = root.join(format!("{tag}.wav"));
            library
                .export(matched.items[0].files[0].id, Some(&export), false)
                .unwrap();
            assert_eq!(fs::read(export).unwrap(), mono);
        }
        assert_eq!(
            serde_json::to_value(library.query(CatalogQuery::default()).unwrap()).unwrap(),
            public,
            "private filtering cannot alter public JSON or variant order"
        );
        assert_eq!(fs::read(root.join("source/long/Long.wav")).unwrap(), mono);
        assert_eq!(
            fs::read(root.join("source/short/Short.mp3")).unwrap(),
            stereo
        );
        drop(library);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn saved_audio_filters_count_and_page_the_whole_catalog_without_analyzing_imports() {
        let root = std::env::temp_dir().join(format!("audio-filter-page-{}", std::process::id()));
        fs::create_dir_all(root.join("source")).unwrap();
        let bytes = include_bytes!("../tests/fixtures/audio/stereo44100.mp3");
        for index in 0..85 {
            fs::write(root.join(format!("source/Sound{index:03}.mp3")), bytes).unwrap();
        }
        let mut library = Library::create(&root.join("library")).unwrap();
        library.import_folder(&root.join("source")).unwrap();
        let before = library
            .query(CatalogQuery {
                limit: 100,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(before.total, 85);
        assert!(before
            .items
            .iter()
            .all(|asset| asset.files[0].audio.is_none()));
        for asset in &before.items[40..] {
            library.analyze_audio(asset.files[0].id).unwrap();
        }
        let criteria = || {
            Some(AudioCriteria {
                max_duration_seconds: Some(0.3),
                channels: Some("stereo".into()),
                ..Default::default()
            })
        };
        let first = library
            .query_with_audio(CatalogQuery::default(), criteria())
            .unwrap();
        assert_eq!(first.total, 45);
        assert_eq!(first.items.len(), 40);
        assert_eq!(first.items[0].name, "Sound040");
        let second = library
            .query_with_audio(
                CatalogQuery {
                    offset: 40,
                    ..Default::default()
                },
                criteria(),
            )
            .unwrap();
        assert_eq!(second.total, 45);
        assert_eq!(second.offset, 40);
        assert_eq!(
            second
                .items
                .iter()
                .map(|asset| asset.name.as_str())
                .collect::<Vec<_>>(),
            ["Sound080", "Sound081", "Sound082", "Sound083", "Sound084"]
        );
        let reset = library
            .query_with_audio(
                CatalogQuery {
                    offset: 100,
                    ..Default::default()
                },
                criteria(),
            )
            .unwrap();
        assert_eq!(reset.offset, 0);
        assert_eq!(reset.items[0].id, first.items[0].id);
        drop(library);
        let library = Library::open_read_only(&root.join("library")).unwrap();
        assert_eq!(
            library
                .query_with_audio(CatalogQuery::default(), criteria())
                .unwrap()
                .total,
            45
        );
        assert!(library
            .query(CatalogQuery {
                limit: 100,
                ..Default::default()
            })
            .unwrap()
            .items[..40]
            .iter()
            .all(|asset| asset.files[0].audio.is_none()));
        assert_eq!(fs::read(root.join("source/Sound084.mp3")).unwrap(), bytes);
        drop(library);
        fs::remove_dir_all(root).unwrap();
    }
}
