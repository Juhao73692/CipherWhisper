use fs2::FileExt;
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

fn source_files(dir: &Path, files: &mut Vec<PathBuf>) {
    for item in fs::read_dir(dir).expect("cannot read source directory") {
        let path = item.unwrap().path();
        if path.is_dir() {
            if !matches!(
                path.file_name().and_then(|n| n.to_str()),
                Some(
                    "node_modules"
                        | "ui"
                        | "dist"
                        | "test-results"
                        | "playwright-report"
                        | "__pycache__"
                )
            ) {
                source_files(&path, files);
            }
        } else if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some(
                "rs" | "svelte"
                    | "css"
                    | "ts"
                    | "js"
                    | "mjs"
                    | "py"
                    | "sh"
                    | "toml"
                    | "json"
                    | "html"
                    | "plist"
                    | "lock"
            )
        ) {
            files.push(path);
        }
    }
}

fn git(repo: &Path, args: &[&str]) -> Option<String> {
    let result = Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .ok()?;
    result
        .status
        .success()
        .then(|| String::from_utf8_lossy(&result.stdout).trim().to_owned())
}

fn read_info(path: &Path) -> Option<serde_json::Value> {
    match fs::read(path) {
        Ok(bytes) => Some(serde_json::from_slice(&bytes).expect("invalid build metadata")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => panic!("cannot read build metadata: {e}"),
    }
}

fn save_info(path: &Path, info: &serde_json::Value) {
    let bytes = format!("{}\n", serde_json::to_string_pretty(info).unwrap()).into_bytes();
    if fs::read(path).ok().as_deref() != Some(bytes.as_slice()) {
        fs::write(path, bytes).expect("cannot save build metadata");
    }
}

fn build_info(manifest: &Path, output: &Path) {
    let repo = manifest.parent().unwrap().parent().unwrap();
    // Git status can change without touching source files (staging, untracked files,
    // etc.). An intentionally absent path makes Cargo check provenance every build.
    println!(
        "cargo:rerun-if-changed={}",
        output.join("check-git-state-on-every-build").display()
    );
    let mut files = vec![repo.join("Cargo.toml"), repo.join("Cargo.lock")];
    for dir in ["apps", "crates", "server", "scripts", ".github"] {
        let path = repo.join(dir);
        println!("cargo:rerun-if-changed={}", path.display());
        source_files(&path, &mut files);
    }
    files.sort();
    let mut hash = Sha256::new();
    for path in files {
        println!("cargo:rerun-if-changed={}", path.display());
        let name = path.strip_prefix(repo).unwrap().to_string_lossy();
        let bytes = fs::read(&path).expect("cannot read source file");
        hash.update((name.len() as u64).to_le_bytes());
        hash.update(name.as_bytes());
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    let commit = git(repo, &["rev-parse", "--verify", "HEAD"]);
    let commit_count = git(repo, &["rev-list", "--count", "HEAD"])
        .map(|count| count.parse::<u64>().expect("invalid Git commit count"));
    let dirty = git(repo, &["status", "--porcelain", "--untracked-files=normal"])
        .map(|status| !status.is_empty());
    let fingerprint: String = hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(repo.join(".build-info.lock"))
        .unwrap();
    lock.lock_exclusive().expect("cannot lock build metadata");
    let state = repo.join(".build-info-state.json");
    let previous_build = read_info(&state);
    // A source archive without .git can retain the provenance from its saved metadata.
    let commit = commit
        .or_else(|| {
            previous_build
                .as_ref()?
                .get("commit")?
                .as_str()
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "unknown".into());
    let dirty = dirty.unwrap_or_else(|| {
        previous_build
            .as_ref()
            .and_then(|info| info["dirty"].as_bool())
            .unwrap_or(false)
    });
    // Count all commits reachable from HEAD, including the initial commit.
    // Dirty is one prospective commit, irrespective of the number of changed files.
    let number = commit_count.map_or_else(
        || {
            previous_build
                .as_ref()
                .and_then(|info| info["number"].as_str()?.parse::<u64>().ok())
                .unwrap_or(0)
        },
        |count| {
            count
                .checked_add(u64::from(dirty))
                .expect("build number overflow")
        },
    );
    let number = format!("{number:03}");
    let unchanged = previous_build.as_ref().is_some_and(|info| {
        info["sourceDigest"] == fingerprint
            && info["number"] == number
            && info["commit"] == commit
            && info["dirty"] == dirty
    });
    let built_at = if unchanged {
        previous_build.unwrap()["builtAt"]
            .as_str()
            .unwrap()
            .to_owned()
    } else {
        time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap()
    };
    let info = serde_json::json!({"number":number, "commit":commit, "dirty":dirty, "builtAt":built_at, "sourceDigest":fingerprint});
    save_info(&state, &info);
    save_info(&output.join("build-info.json"), &info);
}
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
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let output = PathBuf::from(env::var("OUT_DIR").unwrap());
    build_info(&manifest, &output);
    let root = manifest.join("ui");
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
