use std::{env, fs, path::Path};
fn collect(dir: &Path, root: &Path, files: &mut Vec<(String, String)>) {
    for item in
        fs::read_dir(dir).expect("UI assets missing; run npm ci && npm run build in apps/local-ui")
    {
        let path = item.unwrap().path();
        if path.is_dir() {
            collect(&path, root, files);
        } else {
            let key = path
                .strip_prefix(root)
                .unwrap()
                .to_str()
                .unwrap()
                .replace('\\', "/");
            files.push((key, path.to_str().unwrap().to_owned()));
        }
    }
}
fn main() {
    let root = Path::new(&env::var("CARGO_MANIFEST_DIR").unwrap()).join("ui");
    println!("cargo:rerun-if-changed=ui");
    let mut files = Vec::new();
    collect(&root, &root, &mut files);
    files.sort();
    assert!(
        files.iter().any(|(key, _)| key == "index.html"),
        "Build the local UI first"
    );
    let mut source = String::from(
        "fn asset(path: &str) -> Option<(&'static [u8], &'static str)> { match path {\n",
    );
    for (key, path) in files {
        let mime = match Path::new(&key)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
        {
            "html" => "text/html; charset=utf-8",
            "js" => "text/javascript; charset=utf-8",
            "css" => "text/css; charset=utf-8",
            "woff2" => "font/woff2",
            "woff" => "font/woff",
            "ttf" => "font/ttf",
            "txt" => "text/plain; charset=utf-8",
            _ => "application/octet-stream",
        };
        source.push_str(&format!(
            "{key:?} => Some((include_bytes!({path:?}), {mime:?})),\n"
        ));
    }
    source.push_str("_ => None, } }\n");
    fs::write(
        Path::new(&env::var("OUT_DIR").unwrap()).join("ui_assets.rs"),
        source,
    )
    .unwrap();
}
