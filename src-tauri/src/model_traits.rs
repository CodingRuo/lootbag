//! Header-level model inspection: does a file carry mesh geometry, keyed animation, or both?
use super::{location_row, model_resources::gltf_json, AssetKind, Library, Result};
use rusqlite::params;
use std::{
    fs,
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

/// `None` means unknown (unsupported format, unreadable or malformed file).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelTraits {
    pub has_mesh: Option<bool>,
    pub has_animation: Option<bool>,
}

/// SQL condition: the asset `asset` (an id column) is a model whose every active variant is
/// confirmed animation-only. Shared by the catalog filter and the type counts.
pub(super) fn animation_asset(asset: &str) -> String {
    format!("NOT EXISTS (SELECT 1 FROM active_asset_files v LEFT JOIN model_traits t ON t.file_id = v.id
        WHERE v.asset_id = {asset} AND NOT (t.has_animation IS 1 AND t.has_mesh IS 0))")
}

pub(super) fn inspect(path: &Path, format: &str) -> Result<ModelTraits> {
    let known = |mesh, animation| ModelTraits {
        has_mesh: Some(mesh),
        has_animation: Some(animation),
    };
    match format {
        "OBJ" => Ok(known(true, false)),
        "FBX" => {
            let file = fs::File::open(path).map_err(|e| format!("FBX nicht lesbar: {e}"))?;
            fbx(file)
                .map(|(mesh, animation)| known(mesh, animation))
                .map_err(|e| format!("FBX nicht lesbar: {e}"))
        }
        "GLB" | "GLTF" => {
            let mut file = fs::File::open(path).map_err(|e| format!("glTF nicht lesbar: {e}"))?;
            let json = gltf_json(&mut file, format)?;
            let any = |array: &str, field: &str| {
                json[array].as_array().is_some_and(|entries| {
                    entries
                        .iter()
                        .any(|entry| entry[field].as_array().is_some_and(|v| !v.is_empty()))
                })
            };
            Ok(known(
                any("meshes", "primitives"),
                any("animations", "channels"),
            ))
        }
        _ => Ok(ModelTraits::default()),
    }
}

const FBX_MAGIC: &[u8; 23] = b"Kaydara FBX Binary  \0\x1a\0";

fn malformed(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.to_owned())
}

/// (has_mesh, has_animation) of a binary or ASCII FBX 7.x file.
fn fbx(file: fs::File) -> io::Result<(bool, bool)> {
    let len = file.metadata()?.len();
    let mut input = BufReader::new(file);
    let mut head = [0_u8; 27];
    let mut read = 0;
    while read < head.len() {
        match input.read(&mut head[read..])? {
            0 => break,
            n => read += n,
        }
    }
    if read < head.len() || &head[..23] != FBX_MAGIC {
        input.seek(SeekFrom::Start(0))?;
        return fbx_ascii(input);
    }
    let version = u32::from_le_bytes(head[23..27].try_into().unwrap());
    let mut reader = Fbx {
        input,
        pos: 27,
        wide: version >= 7500,
    };
    let (mut mesh, mut animation) = (false, false);
    while let Some(top) = reader.node(len)? {
        if top.name == b"Objects" {
            reader.seek(top.children)?;
            while !(mesh && animation) {
                let Some(object) = reader.node(top.end)? else {
                    break;
                };
                match &object.name[..] {
                    b"Geometry" if !mesh => {
                        mesh = object.props >= 3 && reader.third_string(&object)? == b"Mesh"
                    }
                    // Mesh files often carry an empty "Take 001"; only a keyed curve counts.
                    b"AnimationCurve" if !animation => {
                        reader.seek(object.children)?;
                        while let Some(child) = reader.node(object.end)? {
                            if child.name == b"KeyTime" && child.props >= 1 {
                                reader.seek(child.props_start)?;
                                animation = reader.array_len(child.children)? > 0;
                            }
                            reader.seek(child.end)?;
                            if animation {
                                break;
                            }
                        }
                    }
                    _ => {}
                }
                reader.seek(object.end)?;
            }
        }
        reader.seek(top.end)?;
    }
    Ok((mesh, animation))
}

struct Fbx {
    input: BufReader<fs::File>,
    pos: u64,
    wide: bool,
}

struct Node {
    name: Vec<u8>,
    end: u64,
    props: u64,
    props_start: u64,
    children: u64,
}

impl Fbx {
    fn seek(&mut self, to: u64) -> io::Result<()> {
        let delta = i64::try_from(i128::from(to) - i128::from(self.pos))
            .map_err(|_| malformed("offset"))?;
        self.input.seek_relative(delta)?;
        self.pos = to;
        Ok(())
    }

    fn bytes<const N: usize>(&mut self) -> io::Result<[u8; N]> {
        let mut bytes = [0; N];
        self.input.read_exact(&mut bytes)?;
        self.pos += N as u64;
        Ok(bytes)
    }

    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(self.bytes()?))
    }

    fn uint(&mut self) -> io::Result<u64> {
        if self.wide {
            Ok(u64::from_le_bytes(self.bytes()?))
        } else {
            self.u32().map(u64::from)
        }
    }

    /// The next node header before `end`; `None` at the end of the list or its null record.
    fn node(&mut self, end: u64) -> io::Result<Option<Node>> {
        let start = self.pos;
        if start >= end {
            return Ok(None);
        }
        let node_end = self.uint()?;
        let props = self.uint()?;
        let props_len = self.uint()?;
        let [name_len] = self.bytes::<1>()?;
        if node_end == 0 {
            return Ok(None);
        }
        let mut name = vec![0; usize::from(name_len)];
        self.input.read_exact(&mut name)?;
        self.pos += u64::from(name_len);
        let props_start = self.pos;
        let children = props_start
            .checked_add(props_len)
            .ok_or_else(|| malformed("property list"))?;
        if node_end > end || node_end <= start || children > node_end {
            return Err(malformed("node record"));
        }
        Ok(Some(Node {
            name,
            end: node_end,
            props,
            props_start,
            children,
        }))
    }

    /// The third property of `node` when it is a short string, e.g. the class of a Geometry.
    fn third_string(&mut self, node: &Node) -> io::Result<Vec<u8>> {
        self.seek(node.props_start)?;
        for index in 0..3 {
            let [code] = self.bytes::<1>()?;
            let skip = match code {
                b'C' => 1,
                b'Y' => 2,
                b'I' | b'F' => 4,
                b'L' | b'D' => 8,
                b'S' | b'R' => {
                    let length = u64::from(self.u32()?);
                    if index == 2
                        && code == b'S'
                        && length <= 64
                        && self.pos + length <= node.children
                    {
                        let mut text = vec![0; length as usize];
                        self.input.read_exact(&mut text)?;
                        self.pos += length;
                        return Ok(text);
                    }
                    length
                }
                b'f' | b'd' | b'l' | b'i' | b'b' | b'c' => {
                    let _count = self.u32()?;
                    let _encoding = self.u32()?;
                    u64::from(self.u32()?)
                }
                _ => return Err(malformed("property type")),
            };
            let next = self.pos + skip;
            if next > node.children {
                return Err(malformed("property"));
            }
            self.seek(next)?;
        }
        Ok(Vec::new())
    }

    /// Element count of the array property at the current position; 0 for other types.
    fn array_len(&mut self, props_end: u64) -> io::Result<u32> {
        if self.pos + 5 > props_end {
            return Err(malformed("property"));
        }
        let [code] = self.bytes::<1>()?;
        if b"fdlibc".contains(&code) {
            self.u32()
        } else {
            Ok(0)
        }
    }
}

/// Conservative ASCII FBX 7.x scan. Older ASCII versions keep animation in "Takes" and stay unknown.
fn fbx_ascii(mut input: BufReader<fs::File>) -> io::Result<(bool, bool)> {
    let (mut version, mut mesh, mut animation) = (None, false, false);
    let mut line = Vec::new();
    loop {
        line.clear();
        // Only line starts matter; long array lines are skipped instead of buffered.
        if (&mut input).take(4096).read_until(b'\n', &mut line)? == 0 {
            break;
        }
        if line.last() != Some(&b'\n') {
            input.skip_until(b'\n')?;
        }
        let text = line.trim_ascii();
        if text.starts_with(b";") {
            continue;
        }
        let field = |name: &str| text.strip_prefix(name.as_bytes()).map(<[u8]>::trim_ascii);
        if let Some(value) = field("FBXVersion:") {
            version = std::str::from_utf8(value)
                .ok()
                .and_then(|v| v.parse::<u32>().ok());
        } else if let Some(value) = field("Geometry:") {
            let class = value
                .split(|byte| *byte == b',')
                .nth(2)
                .map(<[u8]>::trim_ascii);
            let class = class.map(|class| class.strip_suffix(b"{").unwrap_or(class).trim_ascii());
            mesh |= class == Some(b"\"Mesh\"");
        } else if let Some(value) = field("KeyTime:") {
            // KeyTime only occurs inside AnimationCurve objects in FBX 7.
            animation |= value
                .strip_prefix(b"*")
                .and_then(|count| {
                    let digits = count.iter().take_while(|b| b.is_ascii_digit()).count();
                    std::str::from_utf8(&count[..digits])
                        .ok()?
                        .parse::<u64>()
                        .ok()
                })
                .is_some_and(|count| count > 0);
        }
    }
    match version {
        Some(7000..) => Ok((mesh, animation)),
        _ => Err(malformed("unsupported FBX version")),
    }
}

impl Library {
    /// Inspects up to `batch` model files after `after` (a file id cursor) that have no traits
    /// row yet; returns (inspected, still pending after the cursor). Unavailable files get no row,
    /// so they are retried on a later run, while the cursor keeps one run from looping on them.
    pub fn inspect_pending_models(
        &self,
        after: &mut i64,
        batch: usize,
        cancel: &AtomicBool,
    ) -> Result<(usize, usize)> {
        let failed = |e: rusqlite::Error| format!("Modellanalyse fehlgeschlagen: {e}");
        let pending = format!(
            "FROM active_asset_files f JOIN assets a ON a.id = f.asset_id JOIN packages p ON p.id = a.package_id
            LEFT JOIN external_sources s ON s.package_id = p.id LEFT JOIN external_files e ON e.file_id = f.id
            WHERE f.format IN ('{}') AND f.id > ?1 AND NOT EXISTS (SELECT 1 FROM model_traits t WHERE t.file_id = f.id)",
            AssetKind::Model.formats().join("','")
        );
        let files = self
            .connection
            .prepare(&format!("SELECT f.id, f.format, f.relative_path, s.root, s.identity, p.relative_path, e.identity, e.modified, f.size_bytes {pending} ORDER BY f.id LIMIT ?2"))
            .map_err(failed)?
            .query_map(params![*after, batch as i64], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, location_row(row, 3)?))
            })
            .map_err(failed)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(failed)?;
        let mut results = Vec::new();
        for (id, format, relative, external) in files {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            *after = id;
            if let Ok(path) = self.locate(&relative, external) {
                results.push((id, inspect(&path, &format)));
            }
        }
        let transaction = self.connection.unchecked_transaction().map_err(failed)?;
        for (id, result) in &results {
            let traits = result.clone().unwrap_or_default();
            // A file deleted from the catalog meanwhile is simply skipped.
            transaction
                .execute(
                    "INSERT OR REPLACE INTO model_traits(file_id, has_mesh, has_animation, error)
                SELECT ?1, ?2, ?3, ?4 WHERE EXISTS (SELECT 1 FROM asset_files WHERE id = ?1)",
                    params![
                        id,
                        traits.has_mesh,
                        traits.has_animation,
                        result.as_ref().err()
                    ],
                )
                .map_err(failed)?;
        }
        transaction.commit().map_err(failed)?;
        let remaining: i64 = self
            .connection
            .query_row(&format!("SELECT COUNT(*) {pending}"), [*after], |row| {
                row.get(0)
            })
            .map_err(failed)?;
        Ok((results.len(), remaining as usize))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    enum Prop {
        Long(i64),
        Text(&'static [u8]),
        /// An `l` array with this many (zeroed) elements.
        Array(u32),
    }

    struct Node(&'static str, Vec<Prop>, Vec<Node>);

    fn uint(out: &mut Vec<u8>, value: u64, wide: bool) {
        if wide {
            out.extend(value.to_le_bytes());
        } else {
            out.extend((value as u32).to_le_bytes());
        }
    }

    fn write(out: &mut Vec<u8>, node: &Node, wide: bool) {
        let start = out.len();
        let width = if wide { 8 } else { 4 };
        out.resize(start + 3 * width + 1, 0);
        out[start + 3 * width] = node.0.len() as u8;
        out.extend(node.0.as_bytes());
        let props_start = out.len();
        for prop in &node.1 {
            match prop {
                Prop::Long(value) => {
                    out.push(b'L');
                    out.extend(value.to_le_bytes());
                }
                Prop::Text(text) => {
                    out.push(b'S');
                    out.extend((text.len() as u32).to_le_bytes());
                    out.extend(*text);
                }
                Prop::Array(count) => {
                    out.push(b'l');
                    for value in [*count, 0, count * 8] {
                        out.extend(value.to_le_bytes());
                    }
                    out.resize(out.len() + *count as usize * 8, 0);
                }
            }
        }
        let props_len = (out.len() - props_start) as u64;
        for child in &node.2 {
            write(out, child, wide);
        }
        if !node.2.is_empty() {
            out.resize(out.len() + 3 * width + 1, 0);
        }
        let mut header = Vec::new();
        uint(&mut header, out.len() as u64, wide);
        uint(&mut header, node.1.len() as u64, wide);
        uint(&mut header, props_len, wide);
        out[start..start + 3 * width].copy_from_slice(&header);
    }

    fn binary(version: u32, objects: Vec<Node>) -> Vec<u8> {
        let wide = version >= 7500;
        let mut out = FBX_MAGIC.to_vec();
        out.extend(version.to_le_bytes());
        write(
            &mut out,
            &Node(
                "FBXHeaderExtension",
                vec![],
                vec![Node("FBXVersion", vec![Prop::Long(7400)], vec![])],
            ),
            wide,
        );
        write(&mut out, &Node("Objects", vec![], objects), wide);
        write(&mut out, &Node("Connections", vec![], vec![]), wide);
        out.resize(out.len() + if wide { 25 } else { 13 }, 0);
        out.extend([0xfa; 16]);
        out
    }

    fn file(bytes: &[u8], extension: &str) -> std::path::PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "model-traits-{}-{}.{extension}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&path, bytes).unwrap();
        path
    }

    fn traits(bytes: &[u8], format: &str) -> ModelTraits {
        let path = file(bytes, &format.to_lowercase());
        let result = inspect(&path, format).unwrap_or_default();
        fs::remove_file(path).unwrap();
        result
    }

    fn known(mesh: bool, animation: bool) -> ModelTraits {
        ModelTraits {
            has_mesh: Some(mesh),
            has_animation: Some(animation),
        }
    }

    fn geometry(class: &'static [u8]) -> Node {
        Node(
            "Geometry",
            vec![
                Prop::Long(1),
                Prop::Text(b"Body\0\x01Geometry"),
                Prop::Text(class),
            ],
            vec![Node("Vertices", vec![Prop::Array(30)], vec![])],
        )
    }

    fn curve(keys: u32) -> Node {
        Node(
            "AnimationCurve",
            vec![
                Prop::Long(2),
                Prop::Text(b"\0\x01AnimCurve"),
                Prop::Text(b""),
            ],
            vec![
                Node("Default", vec![Prop::Long(0)], vec![]),
                Node("KeyTime", vec![Prop::Array(keys)], vec![]),
            ],
        )
    }

    fn take() -> Node {
        Node(
            "AnimationStack",
            vec![
                Prop::Long(3),
                Prop::Text(b"Take 001\0\x01AnimStack"),
                Prop::Text(b""),
            ],
            vec![],
        )
    }

    #[test]
    fn binary_fbx_distinguishes_meshes_empty_takes_and_keyed_curves_in_both_header_widths() {
        for version in [7400, 7500] {
            let curve_node = Node(
                "AnimationCurveNode",
                vec![
                    Prop::Long(4),
                    Prop::Text(b"T\0\x01AnimCurveNode"),
                    Prop::Text(b""),
                ],
                vec![],
            );
            assert_eq!(
                traits(&binary(version, vec![geometry(b"Mesh"), take()]), "FBX"),
                known(true, false)
            );
            assert_eq!(
                traits(
                    &binary(version, vec![geometry(b"Mesh"), take(), curve_node]),
                    "FBX"
                ),
                known(true, false)
            );
            assert_eq!(
                traits(&binary(version, vec![take(), curve(0)]), "FBX"),
                known(false, false)
            );
            assert_eq!(
                traits(&binary(version, vec![take(), curve(0), curve(12)]), "FBX"),
                known(false, true)
            );
            assert_eq!(
                traits(&binary(version, vec![geometry(b"Shape"), curve(3)]), "FBX"),
                known(false, true)
            );
            assert_eq!(
                traits(&binary(version, vec![geometry(b"Mesh"), curve(3)]), "FBX"),
                known(true, true)
            );
            let whole = binary(version, vec![geometry(b"Mesh"), curve(3)]);
            assert_eq!(
                traits(&whole[..whole.len() / 2], "FBX"),
                ModelTraits::default(),
                "{version} truncated"
            );
            let mut corrupt = whole.clone();
            corrupt[27..31].copy_from_slice(&u32::MAX.to_le_bytes());
            assert_eq!(
                traits(&corrupt, "FBX"),
                ModelTraits::default(),
                "{version} corrupt"
            );
        }
    }

    #[test]
    fn ascii_fbx_gltf_and_obj_report_their_contents() {
        let ascii = b"; FBX 7.4.0 project file\n; Geometry: 1, \"x\", \"Mesh\"\nFBXHeaderExtension:  {\n\tFBXVersion: 7400\n}\nObjects:  {\n\tAnimationStack: 3, \"AnimStack::Take 001\", \"\" {\n\t}\n\tAnimationCurve: 5, \"AnimCurve::\", \"\" {\n\t\tKeyTime: *0 {\n\t\t}\n\t}\n}\n";
        assert_eq!(traits(ascii, "FBX"), known(false, false));
        let keyed = String::from_utf8_lossy(ascii)
            .replace("*0", "*4")
            .replace("; Geometry", "Geometry");
        assert_eq!(traits(keyed.as_bytes(), "FBX"), known(true, true));
        assert_eq!(
            traits(b"; FBX 6.1.0 project file\nFBXVersion: 6100\n", "FBX"),
            ModelTraits::default()
        );
        assert_eq!(traits(b"not a model", "FBX"), ModelTraits::default());
        let gltf = br#"{"asset":{"version":"2.0"},"animations":[{"channels":[{"sampler":0}],"samplers":[]}]}"#;
        assert_eq!(traits(gltf, "GLTF"), known(false, true));
        let gltf = br#"{"asset":{"version":"2.0"},"meshes":[{"primitives":[{}]}],"animations":[{"channels":[]}]}"#;
        assert_eq!(traits(gltf, "GLTF"), known(true, false));
        assert_eq!(traits(b"v 0 0 0\n", "OBJ"), known(true, false));
        assert_eq!(traits(b"BLENDER", "BLEND"), ModelTraits::default());
    }
}
