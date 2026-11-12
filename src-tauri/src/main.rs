#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args.iter().any(|arg| arg == "--mcp") {
        if args.len() != 3 || args[0] != "--mcp" || args[1] != "--library" {
            eprintln!("Usage: game-asset-library --mcp --library <absolute-library-folder>");
            std::process::exit(2);
        }
        if let Err(error) = game_asset_library::mcp::serve(
            std::path::Path::new(&args[2]),
            std::io::stdin().lock(),
            std::io::stdout().lock(),
        ) {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    game_asset_library::run();
}
