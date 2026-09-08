use std::{env, process::Command};

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
        if let Some(path) = git(&["rev-parse", "--git-path", &reference]) {
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
