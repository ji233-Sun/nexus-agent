use std::{env, path::Path, process::Command};

fn git(arguments: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .env("TZ", "UTC")
        .args(arguments)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8(output.stdout).ok())
        .flatten()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn main() {
    embed_pdf_assets();
    #[cfg(target_os = "macos")]
    {
        cc::Build::new()
            .file("src/infrastructure/voice/apple_speech.m")
            .flag("-fobjc-arc")
            .flag("-fblocks")
            .compile("nexus_apple_speech");
        println!("cargo::rustc-link-lib=framework=AVFoundation");
        println!("cargo::rustc-link-lib=framework=Foundation");
        println!("cargo::rustc-link-lib=framework=Speech");
        println!("cargo::rerun-if-changed=src/infrastructure/voice/apple_speech.m");
    }
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-env-changed=NEXUS_RELEASE_TAG");
    for reference in [
        Some("HEAD".to_owned()),
        Some("packed-refs".to_owned()),
        git(&["symbolic-ref", "-q", "HEAD"]),
    ]
    .into_iter()
    .flatten()
    {
        if let Some(path) = git(&["rev-parse", "--git-path", &reference])
            && Path::new(&path).exists()
        {
            println!("cargo::rerun-if-changed={path}");
        }
    }
    let tag = env::var("NEXUS_RELEASE_TAG")
        .ok()
        .filter(|tag| !tag.is_empty())
        .or_else(|| {
            git(&[
                "describe",
                "--exact-match",
                "--tags",
                "--match",
                "v[0-9]*",
                "HEAD",
            ])
        })
        .or_else(|| {
            git(&[
                "show",
                "-s",
                "--date=format-local:%Y-%m-%d",
                "--format=nightly-%cd-%ct-%h",
                "--abbrev=12",
                "HEAD",
            ])
        })
        .unwrap_or_else(|| format!("v{}", env::var("CARGO_PKG_VERSION").unwrap()));
    assert!(
        tag.bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b".-+".contains(&byte)),
        "Invalid NEXUS_RELEASE_TAG"
    );
    println!("cargo::rustc-env=NEXUS_RELEASE_TAG={tag}");
}

fn embed_pdf_assets() {
    fn visit(directory: &Path, files: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(directory)
            .expect("PDF assets: run npm ci && npm run build in apps/pdf-viewer")
        {
            let path = entry.expect("PDF asset entry").path();
            if path.is_dir() {
                visit(&path, files);
            } else if path.extension().is_some_and(|ext| ext == "gz") {
                files.push(path);
            }
        }
    }
    let root = Path::new("../pdf-viewer/dist")
        .canonicalize()
        .expect("bundled PDF viewer assets");
    println!("cargo::rerun-if-changed={}", root.display());
    let mut files = Vec::new();
    visit(&root, &mut files);
    files.sort();
    let mut output = String::from("const ASSETS: &[(&str, &[u8])] = &[\n");
    for file in files {
        let name = file
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        output.push_str(&format!(
            "({:?}, include_bytes!({:?})),\n",
            name.strip_suffix(".gz").unwrap(),
            file
        ));
    }
    output.push_str("];\n");
    std::fs::write(
        Path::new(&env::var_os("OUT_DIR").unwrap()).join("pdf_assets.rs"),
        output,
    )
    .expect("write PDF asset index");
}
