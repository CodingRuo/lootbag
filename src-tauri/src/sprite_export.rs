use super::{path_text, ExportOutcome, Library, ModelResources, Result};
use std::{
    fs,
    path::{Component, PathBuf},
};

const MAX_BYTES: usize = 32 * 1024 * 1024;
#[cfg(test)]
thread_local! { static TEST_FAILURE: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) }; }
#[cfg(test)]
fn fail_at(phase: &str) -> Result<()> {
    if TEST_FAILURE.with(|point| point.get()) == Some(phase) {
        Err(format!("Injected {phase} failure"))
    } else {
        Ok(())
    }
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Layout {
    columns: u32,
    rows: u32,
    left: u32,
    top: u32,
    right: u32,
    bottom: u32,
    gap_x: u32,
    gap_y: u32,
}
#[derive(serde::Deserialize, serde::Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
enum Mode {
    Frames,
    Atlas,
    Packed,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PackedPlan {
    width: u32,
    height: u32,
    frames: Vec<[u32; 6]>,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SpritePlan {
    pub(crate) snapshot_id: String,
    pub(crate) file_id: i64,
    pub(crate) destination: PathBuf,
    width: u32,
    height: u32,
    layout: Layout,
    fps: u32,
    mode: Mode,
    lengths: Vec<usize>,
    packed: Option<PackedPlan>,
}
pub(crate) struct SpritePayload<'a> {
    pub(crate) plan: SpritePlan,
    images: Vec<&'a [u8]>,
    frames: Vec<serde_json::Value>,
    width: u32,
    height: u32,
}

impl<'a> SpritePayload<'a> {
    pub(crate) fn request_plan(bytes: &[u8]) -> Result<SpritePlan> {
        Self::header(bytes).map(|(plan, _)| plan)
    }
    fn header(bytes: &[u8]) -> Result<(SpritePlan, usize)> {
        if bytes.len() < 4 || bytes.len() > MAX_BYTES + 16 * 1024 + 4 {
            return Err("Sprite-Export: Ungültige Ausgabengröße".into());
        }
        let length = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
        if length == 0 || length > 16 * 1024 || 4 + length > bytes.len() {
            return Err("Sprite-Export: Ungültiger Exportplan".into());
        }
        let plan = serde_json::from_slice(&bytes[4..4 + length])
            .map_err(|_| "Sprite-Export: Exportplan nicht lesbar")?;
        Ok((plan, 4 + length))
    }
    pub(crate) fn parse(bytes: &'a [u8]) -> Result<Self> {
        let (plan, start) = Self::header(bytes)?;
        let (frames, width, height) = geometry(&plan)?;
        let expected = if plan.mode == Mode::Frames {
            frames.len()
        } else {
            1
        };
        if plan.snapshot_id.is_empty()
            || plan.snapshot_id.len() > 128
            || plan.file_id < 1
            || plan.lengths.len() != expected
            || !plan.destination.is_absolute()
            || plan
                .destination
                .components()
                .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
        {
            return Err("Sprite-Export: Ungültige Quelle oder Ausgabe".into());
        }
        let mut offset = start;
        let mut images = Vec::with_capacity(expected);
        for &length in &plan.lengths {
            let end = offset
                .checked_add(length)
                .ok_or("Sprite-Export: Ausgabe zu groß")?;
            if length == 0 || end > bytes.len() {
                return Err("Sprite-Export: PNG-Ausgabe unvollständig".into());
            }
            let image = &bytes[offset..end];
            validate_png(image, width, height)?;
            images.push(image);
            offset = end;
        }
        if offset != bytes.len() || offset - start > MAX_BYTES {
            return Err("Sprite-Export: Zusätzliche oder zu große Bilddaten".into());
        }
        Ok(Self {
            plan,
            images,
            frames,
            width,
            height,
        })
    }
}

fn geometry(plan: &SpritePlan) -> Result<(Vec<serde_json::Value>, u32, u32)> {
    let l = &plan.layout;
    let count = u64::from(l.columns) * u64::from(l.rows);
    if !(1..=256).contains(&count) || !(1..=60).contains(&plan.fps) {
        return Err("Sprite-Export: Maximal 256 Bilder und 1–60 FPS".into());
    }
    let removed_x =
        u64::from(l.left) + u64::from(l.right) + u64::from(l.columns - 1) * u64::from(l.gap_x);
    let removed_y =
        u64::from(l.top) + u64::from(l.bottom) + u64::from(l.rows - 1) * u64::from(l.gap_y);
    if removed_x >= u64::from(plan.width) || removed_y >= u64::from(plan.height) {
        return Err("Sprite-Export: Ränder verbrauchen das Bild".into());
    }
    let available_x = u64::from(plan.width) - removed_x;
    let available_y = u64::from(plan.height) - removed_y;
    if available_x % u64::from(l.columns) != 0 || available_y % u64::from(l.rows) != 0 {
        return Err("Sprite-Export: Raster ergibt keine ganzen Pixel".into());
    }
    let cell_x = available_x / u64::from(l.columns);
    let cell_y = available_y / u64::from(l.rows);
    if cell_x
        .checked_mul(cell_y)
        .and_then(|v| v.checked_mul(count))
        .filter(|v| *v <= 8 * 1024 * 1024)
        .is_none()
    {
        return Err("Sprite-Export: Ausgabe überschreitet 8 MiPixel".into());
    }
    let width = if plan.mode == Mode::Atlas {
        available_x
    } else {
        cell_x
    };
    let height = if plan.mode == Mode::Atlas {
        available_y
    } else {
        cell_y
    };
    if width > 8192 || height > 8192 {
        return Err("Sprite-Export: Ausgabekante überschreitet 8192 Pixel".into());
    }
    let frames: Vec<serde_json::Value> = (0..count).map(|i| {
        let column = i % u64::from(l.columns); let row = i / u64::from(l.columns);
        serde_json::json!({"filename": if plan.mode == Mode::Frames { format!("frame-{:04}.png", i + 1) } else { "atlas.png".into() },
            "source": {"x":u64::from(l.left) + column * (cell_x + u64::from(l.gap_x)), "y":u64::from(l.top) + row * (cell_y + u64::from(l.gap_y)), "width":cell_x,"height":cell_y},
            "output":{"x":if plan.mode == Mode::Atlas {column * cell_x} else {0},"y":if plan.mode == Mode::Atlas {row * cell_y} else {0},"width":cell_x,"height":cell_y}})
    }).collect();
    if plan.mode == Mode::Packed {
        return packed_geometry(plan, frames, cell_x, cell_y);
    }
    if plan.packed.is_some() {
        return Err("Sprite-Export: Gepackter Plan gehört nicht zu dieser Ausgabe".into());
    }
    Ok((frames, width as u32, height as u32))
}

fn packed_geometry(
    plan: &SpritePlan,
    mut frames: Vec<serde_json::Value>,
    cell_width: u64,
    cell_height: u64,
) -> Result<(Vec<serde_json::Value>, u32, u32)> {
    let invalid = || "Sprite-Export: Ungültige gepackte Frame-Koordinaten".to_string();
    let packed = plan.packed.as_ref().ok_or_else(invalid)?;
    if packed.frames.len() != frames.len()
        || packed.width == 0
        || packed.height == 0
        || packed.width > 8192
        || packed.height > 8192
        || u64::from(packed.width) * u64::from(packed.height) > 8 * 1024 * 1024
    {
        return Err(invalid());
    }
    let mut rectangles = Vec::<(u64, u64, u64, u64)>::with_capacity(frames.len());
    for (index, (frame, coordinates)) in frames.iter_mut().zip(&packed.frames).enumerate() {
        let [tx, ty, tw, th, x, y] = coordinates.map(u64::from);
        let empty = tw == 0 && th == 0;
        if (tw == 0 || th == 0) && !(empty && tx == 0 && ty == 0)
            || tx + tw > cell_width
            || ty + th > cell_height
        {
            return Err(invalid());
        }
        let w = tw.max(1);
        let h = th.max(1);
        if x < 1
            || y < 1
            || x + w + 1 > u64::from(packed.width)
            || y + h + 1 > u64::from(packed.height)
        {
            return Err(invalid());
        }
        let rect = (x - 1, y - 1, x + w + 1, y + h + 1);
        if rectangles.iter().any(|&(left, top, right, bottom)| {
            rect.0 < right && left < rect.2 && rect.1 < bottom && top < rect.3
        }) {
            return Err(invalid());
        }
        rectangles.push(rect);
        frame["index"] = serde_json::json!(index);
        frame["durationMs"] = serde_json::json!(1000.0 / f64::from(plan.fps));
        frame["sourceSize"] = serde_json::json!({"width":cell_width,"height":cell_height});
        frame["trim"] = serde_json::json!({"x":tx,"y":ty,"width":tw,"height":th});
        frame["empty"] = serde_json::json!(empty);
        frame["output"] = serde_json::json!({"x":x,"y":y,"width":w,"height":h});
    }
    Ok((frames, packed.width, packed.height))
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut value = !0_u32;
    for &byte in bytes {
        value ^= u32::from(byte);
        for _ in 0..8 {
            value = (value >> 1) ^ (0xedb8_8320 & 0_u32.wrapping_sub(value & 1));
        }
    }
    !value
}

fn validate_png(bytes: &[u8], width: u32, height: u32) -> Result<()> {
    let invalid = || "Sprite-Export: Beschädigte oder unerwartete PNG-Ausgabe".to_string();
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err(invalid());
    }
    let mut offset = 8;
    let mut header = false;
    let mut data = false;
    let mut data_ended = false;
    let mut chunks = 0;
    while offset < bytes.len() {
        chunks += 1;
        if chunks > 100_000 || bytes.len() - offset < 12 {
            return Err(invalid());
        }
        let length = u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        let end = offset
            .checked_add(12)
            .and_then(|v| v.checked_add(length))
            .filter(|v| *v <= bytes.len())
            .ok_or_else(invalid)?;
        let kind = &bytes[offset + 4..offset + 8];
        let body = &bytes[offset + 8..end - 4];
        if !kind.iter().all(u8::is_ascii_alphabetic) || kind[2] & 32 != 0 {
            return Err(invalid());
        }
        if crc32(&bytes[offset + 4..end - 4])
            != u32::from_be_bytes(bytes[end - 4..end].try_into().unwrap())
        {
            return Err(invalid());
        }
        match kind {
            b"IHDR" if !header && offset == 8 && length == 13 => {
                if u32::from_be_bytes(body[..4].try_into().unwrap()) != width
                    || u32::from_be_bytes(body[4..8].try_into().unwrap()) != height
                    || body[8] != 8
                    || !matches!(body[9], 2 | 6)
                    || body[10..] != [0, 0, 0]
                {
                    return Err(invalid());
                }
                header = true;
            }
            b"IDAT" if header && !data_ended => {
                if !body.is_empty() {
                    data = true;
                }
            }
            b"IEND" if header && data && length == 0 && end == bytes.len() => return Ok(()),
            b"IHDR" | b"IDAT" | b"IEND" => return Err(invalid()),
            _ if header && kind[0] & 32 != 0 => {
                if data {
                    data_ended = true;
                }
            }
            _ => return Err(invalid()),
        }
        offset = end;
    }
    Err(invalid())
}

impl Library {
    pub(crate) fn export_sprite(
        &self,
        snapshot: &ModelResources,
        payload: &SpritePayload<'_>,
        before_commit: impl FnOnce() -> Result<()>,
    ) -> Result<ExportOutcome> {
        self.validate_sheet_source(snapshot)?;
        if payload.plan.file_id != snapshot.file_id
            || payload.plan.snapshot_id != snapshot.snapshot_id
        {
            return Err("Sprite-Vorschau wurde gewechselt".into());
        }
        let destination = &payload.plan.destination;
        let parent = destination
            .parent()
            .ok_or("Exportziel hat keinen Ordner")?
            .canonicalize()
            .map_err(|e| format!("Exportordner nicht verfügbar: {e}"))?;
        self.validate_export_parent(&parent)?;
        let name = destination
            .file_name()
            .ok_or("Exportziel hat keinen Ordnernamen")?;
        if name.to_string_lossy().starts_with('.') {
            return Err("Bitte einen sichtbaren Exportordnernamen wählen".into());
        }
        let source: serde_json::Value = self.connection.query_row(&format!("SELECT f.asset_id,a.package_id,p.name,f.original_filename,{},f.format,f.size_bytes FROM active_asset_files f JOIN assets a ON a.id=f.asset_id JOIN packages p ON p.id=a.package_id WHERE f.id=?1", super::FILE_FOLDER), [snapshot.file_id], |r| Ok(serde_json::json!({"fileId":snapshot.file_id,"assetId":r.get::<_, i64>(0)?,"packageId":r.get::<_, i64>(1)?,"package":r.get::<_, String>(2)?,"filename":r.get::<_, String>(3)?,"originalFolder":r.get::<_, String>(4)?,"format":r.get::<_, String>(5)?,"sizeBytes":r.get::<_, i64>(6)?}))).map_err(|e| format!("Sprite-Herkunft nicht verfügbar: {e}"))?;
        let mut manifest = serde_json::json!({"version":1,"mode":payload.plan.mode,"fps":payload.plan.fps,"source":source,"canvas":{"width":payload.plan.width,"height":payload.plan.height},"layout":payload.plan.layout,"frames":payload.frames,"outputCanvas":{"width":payload.width,"height":payload.height},"trimmed":false,"rotated":false});
        if payload.plan.mode == Mode::Packed {
            manifest["version"] = serde_json::json!(2);
            manifest["trimmed"] = serde_json::json!(true);
            manifest["padding"] = serde_json::json!(1);
            manifest["extrusion"] = serde_json::json!(1);
            manifest["coordinateOrigin"] = serde_json::json!("topLeft");
        }
        let manifest = serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?;
        let destination = parent.join(name);
        // create_dir fails on any existing file, folder or symlink, so existing destinations are never touched.
        fs::create_dir(&destination).map_err(|e| {
            format!("Sprite-Ordner konnte nicht angelegt werden (bestehende Ziele bleiben erhalten): {e}")
        })?;
        let mut total = 0_u64;
        let written = (|| {
            for (i, image) in payload.images.iter().enumerate() {
                let filename = if payload.plan.mode == Mode::Frames {
                    format!("frame-{:04}.png", i + 1)
                } else {
                    "atlas.png".to_string()
                };
                #[cfg(test)]
                fail_at("write")?;
                fs::write(destination.join(filename), image)
                    .map_err(|e| format!("Sprite-Datei konnte nicht geschrieben werden: {e}"))?;
                total += image.len() as u64;
            }
            fs::write(destination.join("sprite.json"), &manifest)
                .map_err(|e| format!("Sprite-Datei konnte nicht geschrieben werden: {e}"))?;
            total += manifest.len() as u64;
            before_commit()?;
            self.validate_sheet_source(snapshot)
        })();
        if let Err(e) = written {
            let _ = fs::remove_dir_all(&destination);
            return Err(e);
        }
        Ok(ExportOutcome::Exported {
            path: path_text(&destination)?,
            size_bytes: total,
        })
    }
}

#[cfg(all(test, target_os = "macos"))]
#[path = "sprite_export_tests.rs"]
mod tests;
