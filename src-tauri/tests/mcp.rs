#![cfg(feature = "desktop")]

use game_asset_library::{
    library::{CatalogQuery, Library},
    mcp,
};
use serde_json::{json, Value};
use std::{
    fs,
    io::{Cursor, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};

const PIXEL_PNG: &[u8] = b"\x89\x50\x4e\x47\x0d\x0a\x1a\x0a\x00\x00\x00\x0d\x49\x48\x44\x52\x00\x00\x00\x01\x00\x00\x00\x01\x08\x06\x00\x00\x00\x1f\x15\xc4\x89\x00\x00\x00\x0d\x49\x44\x41\x54\x78\x9c\x63\x60\x60\xf8\xff\x1f\x00\x03\x02\x01\xff\xe6\x77\x0b\xae\x00\x00\x00\x00\x49\x45\x4e\x44\xae\x42\x60\x82";
const TRIANGLE_OBJ: &[u8] = b"v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n";

struct Workspace(PathBuf);

impl Workspace {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "game-asset-mcp-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn modern(id: i64, method: &str, mut params: Value) -> Value {
    params["_meta"] = json!({"io.modelcontextprotocol/protocolVersion":"2026-07-28", "io.modelcontextprotocol/clientCapabilities":{}});
    json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params})
}

fn call(id: i64, name: &str, arguments: Value) -> Value {
    modern(id, "tools/call", json!({"name":name,"arguments":arguments}))
}

fn serve(root: &Path, requests: &[Value]) -> Vec<Value> {
    let input = requests
        .iter()
        .map(|request| format!("{request}\n"))
        .collect::<String>();
    let mut output = Vec::new();
    mcp::serve(root, Cursor::new(input), &mut output).unwrap();
    String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn process(root: &Path, input: &str) -> Vec<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_game-asset-library"))
        .arg("--mcp")
        .arg("--library")
        .arg(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "No diagnostics are expected on successful requests"
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).expect("stdout contains only JSON-RPC responses"))
        .collect()
}

#[test]
fn mcp_process_rejects_invalid_or_unbounded_requests_and_recovers_until_eof() {
    let workspace = Workspace::new();
    let root = workspace.path("Library");
    drop(Library::create(&root).unwrap());
    let before = fs::read(root.join("catalog.sqlite3")).unwrap();
    let mut requests = vec![
        modern(1, "tools/list", json!({})),
        modern(2, "unknown/method", json!({})),
        call(3, "execute_sql", json!({"sql":"DELETE FROM assets"})),
        call(4, "search_assets", json!({"library":"/private/tmp"})),
        call(5, "get_asset", json!({"path":"/etc/passwd"})),
        call(6, "search_assets", json!({"offset":-1})),
        call(7, "search_assets", json!({"kind":"image","limit":101})),
        call(8, "list_packages", json!({"limit":0})),
        call(
            9,
            "list_folders",
            json!({"offset":9_007_199_254_740_992_i64}),
        ),
        call(10, "get_asset", json!({"assetId":0})),
        call(11, "get_asset", json!({"assetId":1.5})),
        call(12, "search_assets", json!({"text":"a".repeat(1025)})),
        call(13, "search_assets", json!({"text":"bad\ntext"})),
        json!({"jsonrpc":"2.0","id":14,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{}}}),
        json!({"jsonrpc":"2.0","method":"notifications/unknown","params":{}}),
        call(17, "search_assets", json!({"kind":"audio"})),
        call(18, "search_assets", json!({"kind":"Model"})),
        call(19, "search_assets", json!({"kind":null})),
        call(23, "search_assets", json!({"kind":1})),
        call(24, "search_assets", json!({"kind":[]})),
        call(25, "search_assets", json!({"kind":{}})),
        call(26, "search_assets", json!({"kind":""})),
        call(27, "search_assets", json!({"kind":{"image":null}})),
    ];
    let mut unknown_version = modern(15, "tools/list", json!({}));
    unknown_version["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"] =
        json!("2099-01-01");
    requests.push(unknown_version);
    let mut missing_capabilities = modern(16, "tools/list", json!({}));
    missing_capabilities["params"]["_meta"]
        .as_object_mut()
        .unwrap()
        .remove("io.modelcontextprotocol/clientCapabilities");
    requests.push(missing_capabilities);
    for (id, version) in [(20, "2025-03-26"), (21, "2025-06-18"), (22, "2025-11-25")] {
        requests.push(json!({"jsonrpc":"2.0","id":id,"method":"initialize","params":{"protocolVersion":version,"capabilities":{},"clientInfo":{"name":"acceptance","version":"1"}}}));
    }
    requests.push(json!({"jsonrpc":"2.0","id":"legacy-list","method":"tools/list"}));
    requests.push(json!({"jsonrpc":"2.0","id":"legacy-meta","method":"tools/call","params":{"_meta":{"progressToken":"catalog-read"},"name":"list_packages","arguments":{}}}));
    let mut input = String::from("{\n[]\n{\"jsonrpc\":\"2.0\",\"id\":null,\"method\":\"ping\"}\n");
    input.push_str(
        &requests
            .iter()
            .map(|request| format!("{request}\n"))
            .collect::<String>(),
    );
    input.push_str(&" ".repeat(65_537));
    input.push_str("{}\n");
    input.push_str(&format!("{}\n", modern(99, "ping", json!({}))));
    let replies = process(&root, &input);
    assert_eq!(replies[0]["error"]["code"], -32700);
    assert_eq!(replies[1]["error"]["code"], -32600);
    assert_eq!(replies[2]["error"]["code"], -32600);
    for id in (2..=19).chain(23..=27) {
        let reply = replies.iter().find(|reply| reply["id"] == id).unwrap();
        let expected = match id {
            2 => -32601,
            15 => -32022,
            _ => -32602,
        };
        assert_eq!(reply["error"]["code"], expected, "request {id}: {reply}");
    }
    let version_error = replies.iter().find(|reply| reply["id"] == 15).unwrap();
    assert_eq!(version_error["error"]["data"]["requested"], "2099-01-01");
    assert_eq!(version_error["error"]["data"]["supported"][0], "2026-07-28");
    for (id, version) in [(20, "2025-03-26"), (21, "2025-06-18"), (22, "2025-11-25")] {
        assert_eq!(
            replies.iter().find(|reply| reply["id"] == id).unwrap()["result"]["protocolVersion"],
            version
        );
    }
    assert_eq!(
        replies
            .iter()
            .find(|reply| reply["id"] == "legacy-list")
            .unwrap()["result"]["tools"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    assert_eq!(
        replies
            .iter()
            .find(|reply| reply["id"] == "legacy-meta")
            .unwrap()["result"]["structuredContent"]["total"],
        0,
        "legacy progress metadata does not require modern protocol fields"
    );
    assert_eq!(replies[replies.len() - 2]["error"]["code"], -32600);
    assert_eq!(
        replies.last().unwrap()["id"],
        99,
        "valid requests still work after rejected input"
    );
    assert_eq!(fs::read(root.join("catalog.sqlite3")).unwrap(), before);
}

#[test]
fn mcp_discovers_and_searches_the_read_only_inventory_in_both_protocol_versions() {
    let workspace = Workspace::new();
    let root = workspace.path("Library");
    let source = workspace.path("Original Pack");
    fs::create_dir_all(source.join("Water")).unwrap();
    let original = source.join("Water/River.mp3");
    let bytes = include_bytes!("fixtures/audio/stereo44100.mp3");
    fs::write(&original, bytes).unwrap();
    let mut library = Library::create(&root).unwrap();
    let package = library.import_folder(&source).unwrap();
    let asset = library
        .query(CatalogQuery::default())
        .unwrap()
        .items
        .remove(0);
    let project = library.create_folder("Salzburg Protocol", None).unwrap();
    let sounds = library.create_folder("Sounds", Some(project.id)).unwrap();
    library.add_to_folder(sounds.id, asset.id).unwrap();
    library
        .annotate_asset(asset.id, "River ambience", &["water".into()])
        .unwrap();
    library
        .annotate_package(package.id, &["licensed".into()])
        .unwrap();
    library.analyze_audio(asset.files[0].id).unwrap();
    let path = library.resolve_preview(asset.files[0].id).unwrap();
    drop(library);
    let catalog_before = fs::read(root.join("catalog.sqlite3")).unwrap();
    let replies = serve(
        &root,
        &[
            modern(1, "server/discover", json!({})),
            modern(2, "tools/list", json!({})),
            call(3, "list_packages", json!({})),
            call(4, "list_folders", json!({})),
            call(
                5,
                "search_assets",
                json!({"folderId":project.id,"tag":"water","limit":1}),
            ),
            call(6, "get_asset", json!({"assetId":asset.id})),
            json!({"jsonrpc":"2.0","id":7,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"acceptance","version":"1"}}}),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            json!({"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"search_assets","arguments":{"text":"River"}}}),
        ],
    );
    assert_eq!(replies.len(), 8, "notifications have no response");
    assert_eq!(replies[0]["result"]["resultType"], "complete");
    assert_eq!(
        replies[0]["result"]["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        "game-asset-library"
    );
    assert_eq!(replies[1]["result"]["tools"].as_array().unwrap().len(), 4);
    assert!(replies[1]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .all(|tool| tool["annotations"]["readOnlyHint"] == true
            && tool["annotations"]["destructiveHint"] == false));
    assert_eq!(
        replies[2]["result"]["structuredContent"]["items"][0]["id"],
        package.id
    );
    assert_eq!(
        replies[2]["result"]["structuredContent"]["items"][0]["tags"],
        json!(["licensed"])
    );
    assert_eq!(replies[3]["result"]["structuredContent"]["total"], 2);
    assert_eq!(replies[4]["result"]["structuredContent"]["total"], 1);
    let detail = &replies[5]["result"]["structuredContent"];
    assert_eq!(detail["name"], "River ambience");
    assert_eq!(detail["tags"], json!(["water"]));
    assert_eq!(detail["folderIds"], json!([sounds.id]));
    assert_eq!(detail["files"][0]["originalFilename"], "River.mp3");
    assert_eq!(detail["files"][0]["originalFolder"], "Water");
    assert_eq!(detail["files"][0]["originalPath"], path.to_str().unwrap());
    assert_eq!(detail["files"][0]["audio"]["sampleRate"], 44100);
    assert_eq!(
        detail["package"]["sourcePath"],
        source.canonicalize().unwrap().to_str().unwrap()
    );
    assert_eq!(replies[6]["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(
        replies[7]["result"]["structuredContent"]["items"][0]["id"],
        asset.id
    );
    assert!(replies[7]["result"].get("resultType").is_none());
    for reply in [
        &replies[2],
        &replies[3],
        &replies[4],
        &replies[5],
        &replies[7],
    ] {
        let text: Value =
            serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(text, reply["result"]["structuredContent"]);
    }
    assert_eq!(
        fs::read(root.join("catalog.sqlite3")).unwrap(),
        catalog_before
    );
    assert_eq!(fs::read(original).unwrap(), bytes);
    assert_eq!(fs::read(path).unwrap(), bytes);
}

#[test]
fn mcp_process_resolves_only_active_available_files_and_keeps_pagination_monotonic() {
    let workspace = Workspace::new();
    let root = workspace.path("Library");
    let source = workspace.path("Managed Pack");
    let external = workspace.path("External Pack");
    fs::create_dir_all(source.join("Water")).unwrap();
    fs::create_dir_all(&external).unwrap();
    let mp3 = include_bytes!("fixtures/audio/stereo44100.mp3");
    let ogg = include_bytes!("fixtures/audio/stereo32000.ogg");
    fs::write(source.join("Water/River.mp3"), mp3).unwrap();
    fs::write(source.join("Water/River.ogg"), ogg).unwrap();
    fs::write(source.join("Ocean.png"), PIXEL_PNG).unwrap();
    fs::write(external.join("Wind.obj"), TRIANGLE_OBJ).unwrap();
    let mut library = Library::create(&root).unwrap();
    let package = library.import_folder(&source).unwrap();
    let external_package = library.reference_folder(&external).unwrap();
    let assets = library.query(CatalogQuery::default()).unwrap().items;
    let river = assets.iter().find(|asset| asset.name == "River").unwrap();
    let ocean = assets.iter().find(|asset| asset.name == "Ocean").unwrap();
    let wind = assets.iter().find(|asset| asset.name == "Wind").unwrap();
    let river_mp3 = river
        .files
        .iter()
        .find(|file| file.format == "MP3")
        .unwrap();
    let river_ogg = river
        .files
        .iter()
        .find(|file| file.format == "OGG")
        .unwrap();
    let river_path = library.resolve_preview(river_mp3.id).unwrap();
    let hidden_path = library.resolve_preview(ocean.files[0].id).unwrap();
    library.analyze_audio(river_mp3.id).unwrap();
    library.trash_file(river_ogg.id).unwrap();
    library.trash_file(ocean.files[0].id).unwrap();
    let offline = workspace.path("Offline Pack");
    fs::rename(&external, &offline).unwrap();
    drop(library);
    let before = fs::read(root.join("catalog.sqlite3")).unwrap();
    let requests = [
        call(1, "search_assets", json!({"limit":1})),
        call(2, "search_assets", json!({"offset":1,"limit":1})),
        call(3, "search_assets", json!({"offset":2,"limit":1})),
        call(4, "search_assets", json!({"offset":100,"limit":1})),
        call(5, "list_packages", json!({"offset":2})),
        call(6, "list_folders", json!({"offset":100})),
        call(7, "get_asset", json!({"assetId":river.id})),
        call(8, "get_asset", json!({"assetId":wind.id})),
        call(9, "get_asset", json!({"assetId":ocean.id})),
        call(
            10,
            "search_assets",
            json!({"packageId":package.id,"format":"OGG"}),
        ),
        call(11, "search_assets", json!({"kind":"image"})),
        call(12, "search_assets", json!({"kind":"model"})),
    ];
    let input = requests
        .iter()
        .map(|request| format!("{request}\n"))
        .collect::<String>();
    let replies = process(&root, &input);
    assert_eq!(
        replies[0]["result"]["structuredContent"]["items"][0]["id"],
        river.id
    );
    assert_eq!(
        replies[1]["result"]["structuredContent"]["items"][0]["id"],
        wind.id
    );
    for (index, offset) in [(2, 2), (3, 100), (4, 2), (5, 100)] {
        let page = &replies[index]["result"]["structuredContent"];
        assert_eq!(page["offset"], offset);
        assert_eq!(page["items"], json!([]));
    }
    let detail = &replies[6]["result"]["structuredContent"];
    assert_eq!(
        detail["files"].as_array().unwrap().len(),
        1,
        "trashed format variants are excluded"
    );
    assert_eq!(detail["files"][0]["id"], river_mp3.id);
    assert_eq!(
        detail["files"][0]["originalPath"],
        river_path.to_str().unwrap()
    );
    assert_eq!(detail["files"][0]["available"], true);
    assert_eq!(detail["files"][0]["audio"]["status"], "ready");
    let missing = &replies[7]["result"]["structuredContent"];
    assert_eq!(missing["kind"], "model");
    assert_eq!(missing["files"][0]["kind"], "model");
    assert_eq!(missing["files"][0]["originalPath"], Value::Null);
    assert_eq!(missing["files"][0]["available"], false);
    assert_eq!(missing["package"]["id"], external_package.id);
    assert_eq!(missing["package"]["available"], false);
    assert_eq!(
        replies[8]["error"]["code"], -32602,
        "a hidden asset cannot be resolved by guessing its ID"
    );
    assert_eq!(replies[9]["result"]["structuredContent"]["total"], 0);
    assert_eq!(replies[10]["result"]["structuredContent"]["total"], 0);
    assert_eq!(replies[11]["result"]["structuredContent"]["total"], 1);
    assert_eq!(
        replies[11]["result"]["structuredContent"]["items"][0]["files"][0]["originalPath"],
        Value::Null
    );
    assert_eq!(fs::read(root.join("catalog.sqlite3")).unwrap(), before);
    assert_eq!(fs::read(&river_path).unwrap(), mp3);
    assert_eq!(fs::read(hidden_path).unwrap(), PIXEL_PNG);
    assert_eq!(fs::read(offline.join("Wind.obj")).unwrap(), TRIANGLE_OBJ);
    assert_eq!(fs::read(source.join("Water/River.ogg")).unwrap(), ogg);
    let mut library = Library::open(&root).unwrap();
    library.trash_package(package.id).unwrap();
    library.trash_package(external_package.id).unwrap();
    drop(library);
    let before = fs::read(root.join("catalog.sqlite3")).unwrap();
    let replies = serve(
        &root,
        &[
            call(1, "list_packages", json!({})),
            call(2, "search_assets", json!({})),
            call(3, "get_asset", json!({"assetId":river.id})),
        ],
    );
    assert_eq!(replies[0]["result"]["structuredContent"]["total"], 0);
    assert_eq!(replies[1]["result"]["structuredContent"]["total"], 0);
    assert_eq!(replies[2]["error"]["code"], -32602);
    assert_eq!(fs::read(root.join("catalog.sqlite3")).unwrap(), before);
    assert_eq!(fs::read(river_path).unwrap(), mp3);
}

#[test]
fn mcp_process_filters_a_mixed_inventory_and_reports_asset_and_file_kinds() {
    let workspace = Workspace::new();
    let root = workspace.path("Library");
    let source = workspace.path("Mixed Pack");
    fs::create_dir_all(source.join("Game")).unwrap();
    let files: [(&str, &[u8]); 4] = [
        (
            "Game/Hero.mp3",
            include_bytes!("fixtures/audio/stereo44100.mp3"),
        ),
        ("Game/Hero.png", PIXEL_PNG),
        ("Game/Hero.psd", b"8BPS layered photoshop source"),
        ("Game/Hero.obj", TRIANGLE_OBJ),
    ];
    for (name, bytes) in files {
        fs::write(source.join(name), bytes).unwrap();
    }
    let mut library = Library::create(&root).unwrap();
    let package = library.import_folder(&source).unwrap();
    let project = library.create_folder("Salzburg Protocol", None).unwrap();
    let assets = library.query(CatalogQuery::default()).unwrap().items;
    assert_eq!(assets.len(), 3, "different kinds remain separate assets");
    for asset in &assets {
        library.add_to_folder(project.id, asset.id).unwrap();
        library
            .annotate_asset(asset.id, "Hero", &["player".into()])
            .unwrap();
    }
    let paths = assets
        .iter()
        .map(|asset| library.resolve_preview(asset.files[0].id).unwrap())
        .collect::<Vec<_>>();
    let psd = assets
        .iter()
        .flat_map(|asset| &asset.files)
        .find(|file| file.format == "PSD")
        .unwrap();
    let psd_path = library.resolve_preview(psd.id).unwrap();
    drop(library);
    let catalog_before = fs::read(root.join("catalog.sqlite3")).unwrap();
    let mut requests = vec![modern(1, "tools/list", json!({}))];
    for (index, kind) in ["sound", "image", "model"].iter().enumerate() {
        requests.push(call(
            index as i64 + 2,
            "search_assets",
            json!({"kind":kind,"packageId":package.id,"folderId":project.id,"tag":"player"}),
        ));
    }
    let model = assets
        .iter()
        .find(|asset| asset.files[0].format == "OBJ")
        .unwrap();
    requests.push(call(5, "get_asset", json!({"assetId":model.id})));
    requests.push(call(
        6,
        "search_assets",
        json!({"kind":"image","offset":100,"limit":1}),
    ));
    requests.push(call(
        7,
        "search_assets",
        json!({"kind":"model","format":"PNG"}),
    ));
    requests.push(json!({"jsonrpc":"2.0","id":8,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"acceptance","version":"1"}}}));
    requests.push(json!({"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"search_assets","arguments":{"kind":"image"}}}));
    requests.push(call(
        10,
        "search_assets",
        json!({"kind":"image","format":"psd"}),
    ));
    requests.push(call(
        11,
        "search_assets",
        json!({"kind":"model","format":"PSD"}),
    ));
    let input = requests
        .iter()
        .map(|request| format!("{request}\n"))
        .collect::<String>();
    let replies = process(&root, &input);
    let tools = replies[0]["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 4);
    let search = tools
        .iter()
        .find(|tool| tool["name"] == "search_assets")
        .unwrap();
    assert_eq!(
        search["inputSchema"]["properties"]["kind"]["enum"],
        json!(["sound", "image", "model"])
    );
    for (index, (kind, format)) in [("sound", "MP3"), ("image", "PNG"), ("model", "OBJ")]
        .iter()
        .enumerate()
    {
        let page = &replies[index + 1]["result"]["structuredContent"];
        assert_eq!(page["total"], 1);
        let asset = &page["items"][0];
        assert_eq!(asset["kind"], *kind);
        assert_eq!(asset["files"][0]["kind"], *kind);
        assert_eq!(asset["files"][0]["format"], *format);
        assert_eq!(asset["files"][0]["available"], true);
        let position = assets
            .iter()
            .position(|asset| asset.files[0].format == *format)
            .unwrap();
        assert_eq!(
            asset["files"][0]["originalPath"],
            paths[position].to_str().unwrap()
        );
        assert_eq!(asset["originalFolder"], "Game");
        assert_eq!(asset["folderIds"], json!([project.id]));
    }
    let detail = &replies[4]["result"]["structuredContent"];
    assert_eq!(detail["kind"], "model");
    assert_eq!(detail["files"][0]["kind"], "model");
    assert_eq!(detail["files"][0]["originalFilename"], "Hero.obj");
    assert_eq!(detail["files"][0]["audio"], Value::Null);
    assert_eq!(detail["package"]["id"], package.id);
    assert_eq!(
        detail["package"]["sourcePath"],
        source.canonicalize().unwrap().to_str().unwrap()
    );
    assert_eq!(replies[5]["result"]["structuredContent"]["offset"], 100);
    assert_eq!(
        replies[5]["result"]["structuredContent"]["items"],
        json!([])
    );
    assert_eq!(replies[6]["result"]["structuredContent"]["total"], 0);
    assert_eq!(
        replies[8]["result"]["structuredContent"]["items"][0]["kind"],
        "image"
    );
    let psd_page = &replies[9]["result"]["structuredContent"];
    assert_eq!(psd_page["total"], 1);
    let psd_asset = &psd_page["items"][0];
    assert_eq!(psd_asset["kind"], "image");
    let psd_file = psd_asset["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| file["format"] == "PSD")
        .unwrap();
    assert_eq!(psd_file["kind"], "image");
    assert_eq!(psd_file["originalPath"], psd_path.to_str().unwrap());
    assert_eq!(psd_file["available"], true);
    assert_eq!(psd_file["audio"], Value::Null);
    assert_eq!(replies[10]["result"]["structuredContent"]["total"], 0);
    assert_eq!(
        fs::read(root.join("catalog.sqlite3")).unwrap(),
        catalog_before
    );
    for (name, bytes) in files {
        assert_eq!(fs::read(source.join(name)).unwrap(), bytes);
    }
    for path in paths {
        let name = path.file_name().unwrap();
        assert_eq!(
            fs::read(&path).unwrap(),
            fs::read(source.join("Game").join(name)).unwrap()
        );
    }
}

#[test]
fn mcp_startup_rejects_missing_legacy_or_linked_catalogs_without_creating_files() {
    let workspace = Workspace::new();
    let empty = workspace.path("Empty Library");
    fs::create_dir(&empty).unwrap();
    assert!(mcp::serve(&empty, Cursor::new(""), Vec::new()).is_err());
    assert_eq!(fs::read_dir(&empty).unwrap().count(), 0);
    assert!(mcp::serve(Path::new("relative-library"), Cursor::new(""), Vec::new()).is_err());
    let legacy = workspace.path("Legacy Library");
    fs::create_dir(&legacy).unwrap();
    let connection = rusqlite::Connection::open(legacy.join("catalog.sqlite3")).unwrap();
    connection
        .execute_batch("PRAGMA user_version = 1; CREATE TABLE packages (id INTEGER PRIMARY KEY)")
        .unwrap();
    drop(connection);
    let before = fs::read(legacy.join("catalog.sqlite3")).unwrap();
    assert!(mcp::serve(&legacy, Cursor::new(""), Vec::new()).is_err());
    assert_eq!(fs::read(legacy.join("catalog.sqlite3")).unwrap(), before);
    assert_eq!(
        fs::read_dir(&legacy).unwrap().count(),
        1,
        "startup never upgrades a legacy catalog"
    );
    let root = workspace.path("Library");
    drop(Library::create(&root).unwrap());
    let before = fs::read(root.join("catalog.sqlite3")).unwrap();
    for args in [
        vec!["--mcp"],
        vec!["--mcp", "--library", "relative-library"],
        vec!["--mcp", "--library", root.to_str().unwrap(), "--extra"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_game-asset-library"))
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(
            output.stdout.is_empty(),
            "startup errors never contaminate stdout"
        );
        assert!(!output.stderr.is_empty());
    }
    assert_eq!(fs::read(root.join("catalog.sqlite3")).unwrap(), before);
    #[cfg(unix)]
    {
        let saved = workspace.path("saved-catalog.sqlite3");
        fs::rename(root.join("catalog.sqlite3"), &saved).unwrap();
        std::os::unix::fs::symlink(&saved, root.join("catalog.sqlite3")).unwrap();
        assert!(mcp::serve(&root, Cursor::new(""), Vec::new()).is_err());
        assert_eq!(fs::read(saved).unwrap(), before);
    }
}
