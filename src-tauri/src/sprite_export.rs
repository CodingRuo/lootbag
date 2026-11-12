use super::model_resources::SourceStamp;
use super::{path_text, unique_suffix, ExportOutcome, Library, ModelResources, Result};
use std::{
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
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
    #[cfg(target_os = "macos")]
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
        let mut stage = Stage::create(&parent)?;
        let mut total = 0_u64;
        for (i, image) in payload.images.iter().enumerate() {
            let filename = if payload.plan.mode == Mode::Frames {
                format!("frame-{:04}.png", i + 1)
            } else {
                "atlas.png".to_string()
            };
            stage.write(&filename, image)?;
            total += image.len() as u64;
        }
        stage.write("sprite.json", &manifest)?;
        total += manifest.len() as u64;
        stage.sync()?;
        before_commit()?;
        self.validate_sheet_source(snapshot)?;
        stage.publish(name)?;
        Ok(ExportOutcome::Exported {
            path: path_text(&parent.join(name))?,
            size_bytes: total,
        })
    }
    #[cfg(not(target_os = "macos"))]
    pub(crate) fn export_sprite(
        &self,
        _snapshot: &ModelResources,
        _payload: &SpritePayload<'_>,
        _before_commit: impl FnOnce() -> Result<()>,
    ) -> Result<ExportOutcome> {
        Err("Atomarer Sprite-Export ist derzeit nur auf macOS verfügbar".into())
    }
}

#[cfg(target_os = "macos")]
fn identity(metadata: &fs::Metadata) -> Result<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(format!("{}:{}", metadata.dev(), metadata.ino()))
    }
    #[cfg(not(unix))]
    {
        Ok(format!(
            "{:?}",
            metadata.created().map_err(|e| e.to_string())?
        ))
    }
}
#[cfg(target_os = "macos")]
fn directory_identity(path: &Path) -> Result<String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|e| format!("Exportordner nicht verfügbar: {e}"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("Exportordner wurde ersetzt".into());
    }
    identity(&metadata)
}

#[cfg(target_os = "macos")]
struct Stage {
    parent: PathBuf,
    name: String,
    parent_id: String,
    stage_id: String,
    files: Vec<(String, SourceStamp)>,
    published: bool,
    #[cfg(target_os = "macos")]
    parent_fd: fs::File,
    #[cfg(target_os = "macos")]
    stage_fd: fs::File,
}
#[cfg(target_os = "macos")]
impl Stage {
    fn create(parent: &Path) -> Result<Self> {
        let parent_id = directory_identity(parent)?;
        let name = format!(".game-assets-sprite-{}", unique_suffix());
        #[cfg(target_os = "macos")]
        let (parent_fd, stage_fd) = anchored::create(parent, &name)?;
        #[cfg(test)]
        if TEST_FAILURE.with(|point| point.get()) == Some("stage-replacement") {
            fs::rename(parent.join(&name), parent.join("moved-owned-stage"))
                .map_err(|e| e.to_string())?;
            fs::create_dir(parent.join(&name)).map_err(|e| e.to_string())?;
        }
        let stage_id = identity(&stage_fd.metadata().map_err(|e| e.to_string())?)?;
        let stage = Self {
            parent: parent.into(),
            name,
            parent_id,
            stage_id,
            files: Vec::new(),
            published: false,
            #[cfg(target_os = "macos")]
            parent_fd,
            #[cfg(target_os = "macos")]
            stage_fd,
        };
        stage.check()?;
        Ok(stage)
    }
    fn check(&self) -> Result<()> {
        if directory_identity(&self.parent)? != self.parent_id {
            return Err("Sprite-Exportordner wurde während der Vorbereitung ersetzt".into());
        }
        #[cfg(target_os = "macos")]
        if identity(&self.parent_fd.metadata().map_err(|e| e.to_string())?)? != self.parent_id
            || identity(&self.stage_fd.metadata().map_err(|e| e.to_string())?)? != self.stage_id
            || anchored::directory_identity(&self.parent_fd, &self.name)? != self.stage_id
        {
            return Err("Sprite-Exportordner wurde ersetzt".into());
        }
        Ok(())
    }
    fn write(&mut self, name: &str, bytes: &[u8]) -> Result<()> {
        self.check()?;
        #[cfg(target_os = "macos")]
        let mut file = anchored::create_file(&self.stage_fd, name)?;
        self.files.push((
            name.into(),
            SourceStamp::read(&file.metadata().map_err(|e| e.to_string())?)?,
        ));
        #[cfg(test)]
        if TEST_FAILURE.with(|point| point.get()) == Some("write") {
            file.write_all(&bytes[..bytes.len() / 2])
                .map_err(|e| e.to_string())?;
            return fail_at("write");
        }
        file.write_all(bytes)
            .map_err(|e| format!("Sprite-Datei konnte nicht geschrieben werden: {e}"))?;
        #[cfg(test)]
        fail_at("sync")?;
        file.sync_all()
            .map_err(|e| format!("Sprite-Datei konnte nicht gespeichert werden: {e}"))?;
        let metadata = file.metadata().map_err(|e| e.to_string())?;
        if metadata.len() != bytes.len() as u64 {
            return Err("Sprite-Ausgabe wurde während des Schreibens verändert".into());
        }
        self.files
            .last_mut()
            .ok_or("Sprite-Ausgabe wurde nicht vorbereitet")?
            .1 = SourceStamp::read(&metadata)?;
        self.check()
    }
    fn sync(&self) -> Result<()> {
        self.check()?;
        #[cfg(target_os = "macos")]
        self.stage_fd
            .sync_all()
            .map_err(|e| format!("Sprite-Ordner konnte nicht gespeichert werden: {e}"))?;
        Ok(())
    }
    fn publish(&mut self, destination: &std::ffi::OsStr) -> Result<()> {
        self.check()?;
        for (name, stamp) in &self.files {
            if anchored::output_stamp(&self.stage_fd, name)? != *stamp {
                return Err("Sprite-Ausgabe wurde während der Vorbereitung verändert".into());
            }
        }
        anchored::check_entries(&self.stage_fd, &self.files)?;
        #[cfg(test)]
        fail_at("publish")?;
        #[cfg(target_os = "macos")]
        anchored::publish(&self.parent_fd, &self.name, destination)?;
        #[cfg(target_os = "macos")]
        {
            self.published = true;
            Ok(())
        }
    }
}
#[cfg(target_os = "macos")]
impl Drop for Stage {
    fn drop(&mut self) {
        if self.published {
            return;
        }
        #[cfg(target_os = "macos")]
        {
            for (name, expected) in self.files.iter().rev() {
                anchored::remove_file(&self.stage_fd, name, &expected.identity);
            }
            if anchored::directory_identity(&self.parent_fd, &self.name)
                .ok()
                .as_ref()
                == Some(&self.stage_id)
            {
                anchored::remove_directory(&self.parent_fd, &self.name);
            }
        }
    }
}

#[cfg(target_os = "macos")]
pub(super) mod anchored {
    use super::*;
    use std::{
        ffi::{CString, OsStr},
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::ffi::OsStrExt,
        },
    };
    extern "C" {
        fn mkdirat(fd: i32, name: *const std::ffi::c_char, mode: u16) -> i32;
        fn openat(fd: i32, name: *const std::ffi::c_char, flags: i32, ...) -> i32;
        fn unlinkat(fd: i32, name: *const std::ffi::c_char, flags: i32) -> i32;
        fn renameatx_np(
            from: i32,
            source: *const std::ffi::c_char,
            to: i32,
            destination: *const std::ffi::c_char,
            flags: u32,
        ) -> i32;
        #[cfg_attr(target_arch = "x86_64", link_name = "fdopendir$INODE64")]
        fn fdopendir(fd: i32) -> *mut std::ffi::c_void;
        #[cfg_attr(target_arch = "x86_64", link_name = "readdir$INODE64")]
        fn readdir(directory: *mut std::ffi::c_void) -> *const DirectoryEntry;
        fn closedir(directory: *mut std::ffi::c_void) -> i32;
        fn __error() -> *mut i32;
    }
    // Darwin SDK sys/dirent.h, 64-bit inode layout (arm64 and x86_64).
    #[repr(C)]
    struct DirectoryEntry {
        inode: u64,
        offset: u64,
        record_length: u16,
        name_length: u16,
        kind: u8,
        name: [u8; 1024],
    }
    struct Directory(*mut std::ffi::c_void);
    impl Drop for Directory {
        fn drop(&mut self) {
            // SAFETY: fdopendir transferred the uniquely owned listing descriptor.
            let _ = unsafe { closedir(self.0) };
        }
    }
    fn name(value: &OsStr) -> Result<CString> {
        if Path::new(value).components().count() != 1
            || !matches!(
                Path::new(value).components().next(),
                Some(Component::Normal(_))
            )
        {
            return Err("Ungültiger Exportname".into());
        }
        CString::new(value.as_bytes()).map_err(|_| "Ungültiger Exportname".into())
    }
    fn opened(fd: i32) -> Result<fs::File> {
        if fd < 0 {
            Err(format!(
                "Sprite-Export: {}",
                std::io::Error::last_os_error()
            ))
        } else {
            // SAFETY: successful openat returns a uniquely owned descriptor.
            Ok(unsafe { fs::File::from_raw_fd(fd) })
        }
    }
    pub(super) fn create(parent: &Path, stage: &str) -> Result<(fs::File, fs::File)> {
        let directory = fs::File::open(parent).map_err(|e| e.to_string())?;
        if identity(&directory.metadata().map_err(|e| e.to_string())?)?
            != super::directory_identity(parent)?
        {
            return Err("Exportordner wurde ersetzt".into());
        }
        let file = create_at(&directory, stage)?;
        Ok((directory, file))
    }
    pub(in crate::library) fn create_at(directory: &fs::File, stage: &str) -> Result<fs::File> {
        create_at_report_retained(directory, stage, || {})
    }
    pub(in crate::library) fn create_at_report_retained(
        directory: &fs::File,
        stage: &str,
        retained: impl FnOnce(),
    ) -> Result<fs::File> {
        let stage_name = stage;
        let stage = name(OsStr::new(stage))?;
        // SAFETY: valid directory descriptor and NUL-terminated names; Darwin SDK constants.
        if unsafe { mkdirat(directory.as_raw_fd(), stage.as_ptr(), 0o700) } != 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        match opened(unsafe {
            openat(
                directory.as_raw_fd(),
                stage.as_ptr(),
                0x100000 | 0x100 | 0x1000000,
            )
        }) {
            Ok(file) => Ok(file),
            Err(error) => {
                // No descriptor pins the newly created identity; unlinking could remove a replacement.
                retained();
                Err(format!(
                    "{error}; der neu angelegte Ordner „{stage_name}“ konnte nicht geöffnet werden und wurde nicht sicher entfernt. Bitte den Zielordner prüfen."
                ))
            }
        }
    }
    pub(in crate::library) fn create_file(stage: &fs::File, filename: &str) -> Result<fs::File> {
        let filename = name(OsStr::new(filename))?;
        // SAFETY: anchored owned directory; O_EXCL|O_NOFOLLOW prevents replacing existing resources.
        opened(unsafe {
            openat(
                stage.as_raw_fd(),
                filename.as_ptr(),
                1 | 0x200 | 0x800 | 0x100 | 0x1000000,
                0o600_i32,
            )
        })
    }
    pub(super) fn directory_identity(parent: &fs::File, filename: &str) -> Result<String> {
        let filename = name(OsStr::new(filename))?;
        // SAFETY: one leaf in the retained parent; no symlink traversal.
        let file = opened(unsafe {
            openat(
                parent.as_raw_fd(),
                filename.as_ptr(),
                0x100000 | 0x100 | 0x1000000,
            )
        })?;
        identity(&file.metadata().map_err(|e| e.to_string())?)
    }
    pub(super) fn output_stamp(stage: &fs::File, filename: &str) -> Result<SourceStamp> {
        let filename = name(OsStr::new(filename))?;
        // SAFETY: anchored no-follow leaf; O_NONBLOCK prevents substituted FIFOs from waiting for a writer.
        let file =
            opened(unsafe { openat(stage.as_raw_fd(), filename.as_ptr(), 4 | 0x100 | 0x1000000) })?;
        let metadata = file.metadata().map_err(|e| e.to_string())?;
        if !metadata.is_file() {
            return Err("Sprite-Ausgabe ist keine reguläre Datei".into());
        }
        SourceStamp::read(&metadata)
    }
    pub(super) fn check_entries(stage: &fs::File, files: &[(String, SourceStamp)]) -> Result<()> {
        // SAFETY: a new anchored directory descriptor has an independent listing offset.
        let listing = opened(unsafe {
            openat(
                stage.as_raw_fd(),
                c".".as_ptr(),
                0x100000 | 0x100 | 0x1000000,
            )
        })?;
        let raw = unsafe { fdopendir(listing.as_raw_fd()) };
        if raw.is_null() {
            return Err(std::io::Error::last_os_error().to_string());
        }
        std::mem::forget(listing);
        let directory = Directory(raw);
        let mut found = std::collections::BTreeSet::new();
        loop {
            // SAFETY: thread-local errno and live opaque DIR. The entry remains valid until the next readdir.
            unsafe {
                *__error() = 0;
            }
            let entry = unsafe { readdir(directory.0) };
            if entry.is_null() {
                if unsafe { *__error() } != 0 {
                    return Err(std::io::Error::last_os_error().to_string());
                }
                break;
            }
            let length = unsafe { (*entry).name_length as usize };
            let name_offset = std::mem::offset_of!(DirectoryEntry, name);
            if length == 0
                || length > 1023
                || unsafe { (*entry).record_length as usize } < name_offset + length + 1
            {
                return Err("Ungültiger Name im Sprite-Exportordner".into());
            }
            let bytes = unsafe {
                std::slice::from_raw_parts(std::ptr::addr_of!((*entry).name).cast::<u8>(), length)
            };
            if bytes == b"." || bytes == b".." {
                continue;
            }
            let filename = std::str::from_utf8(bytes)
                .map_err(|_| "Unbekannte Datei im Sprite-Exportordner")?;
            if !files.iter().any(|(name, _)| name == filename) || !found.insert(filename.to_owned())
            {
                return Err("Unbekannte Datei im Sprite-Exportordner; Export abgebrochen".into());
            }
        }
        if found.len() != files.len() {
            return Err("Sprite-Exportdatei fehlt".into());
        }
        Ok(())
    }
    pub(super) fn remove_file(stage: &fs::File, filename: &str, expected: &str) {
        let Ok(filename) = name(OsStr::new(filename)) else {
            return;
        };
        // SAFETY: leaf lookups stay in the owned stage even if its name moved.
        let Ok(file) =
            opened(unsafe { openat(stage.as_raw_fd(), filename.as_ptr(), 4 | 0x100 | 0x1000000) })
        else {
            return;
        };
        if file
            .metadata()
            .ok()
            .filter(|m| m.is_file())
            .and_then(|m| identity(&m).ok())
            .as_deref()
            == Some(expected)
        {
            let _ = unsafe { unlinkat(stage.as_raw_fd(), filename.as_ptr(), 0) };
        }
    }
    #[cfg(any(feature = "desktop", test))]
    pub(in crate::library) fn remove_owned(
        parent: &fs::File,
        filename: &OsStr,
        expected: (u64, u64),
        directory: bool,
    ) -> bool {
        let Ok(stamp) = crate::library::storage::anchored::child_stamp(parent, filename) else {
            return false;
        };
        if stamp.identity() != expected
            || (directory && !stamp.is_dir())
            || (!directory && !stamp.is_file())
        {
            return false;
        }
        let Ok(filename) = name(filename) else {
            return false;
        };
        // SAFETY: single checked owned leaf; directory removal is empty-only, never recursive through a replacement.
        (unsafe {
            unlinkat(
                parent.as_raw_fd(),
                filename.as_ptr(),
                if directory { 0x80 } else { 0 },
            )
        }) == 0
    }
    pub(super) fn remove_directory(parent: &fs::File, filename: &str) {
        let Ok(filename) = name(OsStr::new(filename)) else {
            return;
        };
        // SAFETY: AT_REMOVEDIR removes only the checked empty owned staging name.
        let _ = unsafe { unlinkat(parent.as_raw_fd(), filename.as_ptr(), 0x80) };
    }
    pub(in crate::library) fn publish(
        parent: &fs::File,
        stage: &str,
        destination: &OsStr,
    ) -> Result<()> {
        let stage = name(OsStr::new(stage))?;
        let destination = name(destination)?;
        // SAFETY: both names resolve in the retained parent directory. RENAME_EXCL never replaces a collision.
        if unsafe {
            renameatx_np(
                parent.as_raw_fd(),
                stage.as_ptr(),
                parent.as_raw_fd(),
                destination.as_ptr(),
                4,
            )
        } != 0
        {
            return Err(format!("Sprite-Ordner konnte nicht veröffentlicht werden (bestehende Ziele bleiben erhalten): {}",std::io::Error::last_os_error()));
        }
        Ok(())
    }
}

#[cfg(all(test, target_os = "macos"))]
#[path = "sprite_export_tests.rs"]
mod tests;
