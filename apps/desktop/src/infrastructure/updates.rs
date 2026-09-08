use std::{
    cmp::Ordering,
    fs::{self, File, OpenOptions},
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
    sync::{
        Arc,
        mpsc::{self, Receiver},
    },
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result, bail, ensure};
use gpui_kit::http_client::{
    AsyncBody, HttpClient, HttpRequestExt as _, RedirectPolicy, Request, Response,
};
use semver::Version;
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use smol::io::AsyncReadExt as _;

use crate::{
    i18n::LocalizedText,
    model::updates::{UpdateAsset, UpdateChannel, UpdatePackage, UpdateState, installed_tag},
};

const RELEASES_URL: &str = "https://api.github.com/repos/ji233-Sun/nexus-agent/releases";
const DOWNLOADS_URL: &str = "https://github.com/ji233-Sun/nexus-agent/releases/download";
const PAGE_SIZE: usize = 100;
const MAX_PAGES: usize = 50;

#[derive(Debug, PartialEq, Eq)]
enum ReleaseVersion {
    Release(Version),
    Nightly(i64),
}

impl ReleaseVersion {
    fn parse(tag: &str) -> Option<Self> {
        if let Some(version) = tag.strip_prefix('v') {
            return Version::parse(version).ok().map(Self::Release);
        }
        let (prefix, sha) = tag.strip_prefix("nightly-")?.rsplit_once('-')?;
        let (date, timestamp) = prefix.rsplit_once('-')?;
        if sha.len() != 12 || !sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        let timestamp: i64 = timestamp.parse().ok()?;
        if timestamp < 0
            || chrono::DateTime::from_timestamp(timestamp, 0)?
                .format("%Y-%m-%d")
                .to_string()
                != date
        {
            return None;
        }
        Some(Self::Nightly(timestamp))
    }

    fn channel(&self) -> UpdateChannel {
        match self {
            Self::Release(_) => UpdateChannel::Release,
            Self::Nightly(_) => UpdateChannel::Nightly,
        }
    }

    fn newer_than(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Release(left), Self::Release(right)) => {
                left.cmp_precedence(right) == Ordering::Greater
            }
            (Self::Nightly(left), Self::Nightly(right)) => left > right,
            // Choosing another channel explicitly requests its latest package.
            _ => true,
        }
    }
}

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    #[serde(default)]
    body: Option<String>,
    assets: Vec<UpdateAsset>,
}

fn platform_asset_suffix(os: &str, arch: &str, abi: &str) -> Result<&'static str> {
    match (os, arch, abi) {
        ("macos", "aarch64", _) => Ok("aarch64-apple-darwin.zip"),
        ("macos", "x86_64", _) => Ok("x86_64-apple-darwin.zip"),
        ("linux", "x86_64", "gnu") => Ok("x86_64-unknown-linux-gnu.tar.gz"),
        ("windows", "x86_64", "msvc") => Ok("x86_64-pc-windows-msvc.zip"),
        _ => bail!("No update package is published for {os}/{arch}/{abi}"),
    }
}

fn current_asset_suffix() -> Result<&'static str> {
    let abi = if cfg!(target_env = "gnu") {
        "gnu"
    } else if cfg!(target_env = "msvc") {
        "msvc"
    } else {
        ""
    };
    platform_asset_suffix(std::env::consts::OS, std::env::consts::ARCH, abi)
}

fn select_release(
    releases: impl IntoIterator<Item = Release>,
    current_tag: &str,
    channel: UpdateChannel,
) -> Result<Option<Release>> {
    let current = ReleaseVersion::parse(current_tag).context("Invalid installed release tag")?;
    let mut selected: Option<(ReleaseVersion, Release)> = None;
    for release in releases {
        if release.draft {
            continue;
        }
        let Some(version) = ReleaseVersion::parse(&release.tag_name) else {
            continue;
        };
        if version.channel() == channel
            && version.newer_than(&current)
            && selected
                .as_ref()
                .is_none_or(|(best, _)| version.newer_than(best))
        {
            selected = Some((version, release));
        }
    }
    Ok(selected.map(|(_, release)| release))
}

fn select_package(release: Release, suffix: &str) -> Result<UpdatePackage> {
    let name = format!("nexus-agent-{}-{suffix}", release.tag_name);
    let asset = release
        .assets
        .into_iter()
        .find(|asset| asset.name == name)
        .with_context(|| format!("Release {} has no package for {suffix}", release.tag_name))?;
    // Only accept the exact published archive from this repository, never a source archive.
    ensure!(
        asset.browser_download_url == format!("{DOWNLOADS_URL}/{}/{name}", release.tag_name),
        "Unexpected release download URL"
    );
    ensure!(asset.size > 0, "The update package is empty");
    let digest = asset
        .digest
        .as_deref()
        .and_then(|digest| digest.strip_prefix("sha256:"))
        .context("The update package has no SHA-256 digest")?;
    ensure!(
        digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "Invalid update package SHA-256 digest"
    );
    Ok(UpdatePackage {
        tag: release.tag_name,
        notes: release.body.unwrap_or_default(),
        asset,
    })
}

async fn request(
    http: &dyn HttpClient,
    url: &str,
    timeout: Duration,
) -> Result<Response<AsyncBody>> {
    let response = http
        .send(
            Request::get(url)
                .header(
                    "User-Agent",
                    concat!("Nexus-Agent/", env!("CARGO_PKG_VERSION")),
                )
                .header(
                    "Accept",
                    if url.starts_with(RELEASES_URL) {
                        "application/vnd.github+json"
                    } else {
                        "application/octet-stream"
                    },
                )
                .header("X-GitHub-Api-Version", "2022-11-28")
                .follow_redirects(RedirectPolicy::FollowLimit(5))
                .timeout(timeout)
                .body(AsyncBody::default())?,
        )
        .await?;
    ensure!(
        response.status().is_success(),
        "GitHub returned HTTP {}",
        response.status()
    );
    Ok(response)
}

async fn find_package(
    http: &dyn HttpClient,
    current_tag: &str,
    channel: UpdateChannel,
    suffix: &str,
) -> Result<Option<UpdatePackage>> {
    let mut selected = None;
    for page in 1..=MAX_PAGES {
        let url = format!("{RELEASES_URL}?per_page={PAGE_SIZE}&page={page}");
        let mut response = request(http, &url, Duration::from_secs(30)).await?;
        let has_next = response
            .headers()
            .get("link")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.split(',').any(|link| link.contains("rel=\"next\"")));
        let mut body = Vec::new();
        response
            .body_mut()
            .take(4 * 1024 * 1024 + 1)
            .read_to_end(&mut body)
            .await?;
        ensure!(
            body.len() <= 4 * 1024 * 1024,
            "GitHub release response is too large"
        );
        let mut releases: Vec<Release> = serde_json::from_slice(&body)?;
        releases.extend(selected.take());
        selected = select_release(releases, current_tag, channel)?;
        if !has_next {
            return selected
                .map(|release| select_package(release, suffix))
                .transpose();
        }
    }
    bail!("Too many GitHub release pages; please retry later")
}

pub(crate) fn failure(error: anyhow::Error) -> UpdateState {
    UpdateState::Failed(LocalizedText::new(
        "更新失败：{error}",
        &[("error", format!("{error:#}"))],
    ))
}

pub(crate) fn spawn_check(channel: UpdateChannel) -> Result<Receiver<UpdateState>> {
    let (sender, receiver) = mpsc::channel();
    std::thread::Builder::new()
        .name("nexus-updates".into())
        .spawn(move || {
            let result = smol::block_on(async {
                let http = reqwest_client::ReqwestClient::user_agent(concat!(
                    "Nexus-Agent/",
                    env!("CARGO_PKG_VERSION")
                ))?;
                let suffix = current_asset_suffix()?;
                check(&http, installed_tag(), channel, suffix).await
            });
            let _ = sender.send(result.unwrap_or_else(failure));
        })?;
    Ok(receiver)
}

async fn check(
    http: &dyn HttpClient,
    current_tag: &str,
    channel: UpdateChannel,
    suffix: &str,
) -> Result<UpdateState> {
    let Some(package) = find_package(http, current_tag, channel, suffix).await? else {
        return Ok(UpdateState::UpToDate);
    };
    Ok(UpdateState::Available(Arc::new(package)))
}

pub(crate) fn spawn_download(
    package: Arc<UpdatePackage>,
    channel: UpdateChannel,
) -> Result<Receiver<UpdateState>> {
    let (sender, receiver) = mpsc::channel();
    std::thread::Builder::new()
        .name("nexus-update-download".into())
        .spawn(move || {
            let result = smol::block_on(async {
                let http = reqwest_client::ReqwestClient::user_agent(concat!(
                    "Nexus-Agent/",
                    env!("CARGO_PKG_VERSION")
                ))?;
                let directory = super::paths::data_directory()?
                    .join("updates")
                    .join(channel.as_str());
                download(&http, package, &directory, |state| {
                    sender.send(state).context("Update task was closed")
                })
                .await
            });
            let _ = sender.send(result.unwrap_or_else(failure));
        })?;
    Ok(receiver)
}

async fn download(
    http: &dyn HttpClient,
    package: Arc<UpdatePackage>,
    directory: &Path,
    mut progress: impl FnMut(UpdateState) -> Result<()>,
) -> Result<UpdateState> {
    let mut last_progress = Instant::now();
    let path = download_package(http, &package, directory, |received| {
        if received == package.asset.size || last_progress.elapsed() >= Duration::from_millis(100) {
            progress(UpdateState::Downloading {
                package: package.clone(),
                received,
            })?;
            last_progress = Instant::now();
        }
        Ok(())
    })
    .await?;
    Ok(UpdateState::Ready { package, path })
}

pub(crate) fn spawn_install(
    package: Arc<UpdatePackage>,
    path: PathBuf,
) -> Result<Receiver<UpdateState>> {
    let (sender, receiver) = mpsc::channel();
    std::thread::Builder::new()
        .name("nexus-update-install".into())
        .spawn(move || {
            let result = super::update_installation::prepare_and_launch(&package, &path)
                .map(|()| UpdateState::Restarting(package));
            let _ = sender.send(result.unwrap_or_else(failure));
        })?;
    Ok(receiver)
}

// Partial files never become installable packages; errors drop this guard.
struct PartialDownload(PathBuf);

impl Drop for PartialDownload {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

pub(super) fn cached_package_matches(path: &Path, size: u64, digest: &str) -> Result<bool> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    if file.metadata()?.len() != size {
        return Ok(false);
    }
    let mut hasher = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()).eq_ignore_ascii_case(digest))
}

async fn download_package(
    http: &dyn HttpClient,
    package: &UpdatePackage,
    directory: &Path,
    mut progress: impl FnMut(u64) -> Result<()>,
) -> Result<PathBuf> {
    let asset = &package.asset;
    let digest = asset
        .digest
        .as_deref()
        .and_then(|digest| digest.strip_prefix("sha256:"))
        .context("The update package has no SHA-256 digest")?;
    fs::create_dir_all(directory).context("Cannot create the update download directory")?;
    let destination = directory.join(&asset.name);
    if cached_package_matches(&destination, asset.size, digest)? {
        return Ok(destination);
    }
    let mut response = request(
        http,
        &asset.browser_download_url,
        Duration::from_secs(30 * 60),
    )
    .await?;
    let partial =
        PartialDownload(directory.join(format!(".{}.{}.part", asset.name, uuid::Uuid::new_v4())));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&partial.0)?;
    let mut hasher = Sha256::new();
    let mut received = 0_u64;
    let mut buffer = [0; 64 * 1024];
    loop {
        // An idle connection must fail too, even if the total download deadline is long.
        let count = smol::future::or(
            async {
                response
                    .body_mut()
                    .read(&mut buffer)
                    .await
                    .map_err(anyhow::Error::from)
            },
            async {
                smol::Timer::after(Duration::from_secs(30)).await;
                bail!("Update download timed out")
            },
        )
        .await?;
        if count == 0 {
            break;
        }
        received += count as u64;
        ensure!(
            received <= asset.size,
            "Update package exceeds its published size"
        );
        file.write_all(&buffer[..count])?;
        hasher.update(&buffer[..count]);
        progress(received)?;
    }
    ensure!(
        received == asset.size,
        "Update package is incomplete: expected {} bytes, received {received}",
        asset.size
    );
    ensure!(
        format!("{:x}", hasher.finalize()).eq_ignore_ascii_case(digest),
        "Update package SHA-256 mismatch"
    );
    file.sync_all()?;
    drop(file);
    // Windows cannot rename over an existing cache file. Replace it only after validation.
    if destination.exists() {
        fs::remove_file(&destination)?;
    }
    fs::rename(&partial.0, &destination)?;
    Ok(destination)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::http_client::{FakeHttpClient, RequestTimeout};
    use serde_json::{Value, json};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering as AtomicOrdering},
    };

    const SUFFIX: &str = "aarch64-apple-darwin.zip";
    const SHA256_ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    const OLD_NIGHTLY: &str = "nightly-2026-09-06-1788695183-02b00a5848c7";
    const NEW_NIGHTLY: &str = "nightly-2026-09-06-1788708123-b81b0727ffb3";

    fn release_json(tag: &str, suffix: &str) -> Value {
        let name = format!("nexus-agent-{tag}-{suffix}");
        json!({
            "tag_name": tag,
            "draft": false,
            "prerelease": tag.contains('-'),
            "body": "## Changes\n\n- Fix application updates.",
            "assets": [{
                "name": name,
                "browser_download_url": format!("{DOWNLOADS_URL}/{tag}/{name}"),
                "size": 3,
                "digest": format!("sha256:{SHA256_ABC}"),
            }],
        })
    }

    fn release(tag: &str) -> Release {
        serde_json::from_value(release_json(tag, SUFFIX)).unwrap()
    }

    #[test]
    fn release_updates_use_semver_and_never_mix_nightly_or_drafts() {
        let mut draft = release("v9.0.0");
        draft.draft = true;
        let selected = select_release(
            [
                release(NEW_NIGHTLY),
                release("v1.0.0-alpha.10"),
                draft,
                release("v1.0.0-alpha.2"),
            ],
            "v1.0.0-alpha.1",
            UpdateChannel::Release,
        )
        .unwrap()
        .unwrap();
        assert_eq!(selected.tag_name, "v1.0.0-alpha.10");
        assert!(
            select_release(
                [
                    release(NEW_NIGHTLY),
                    release("v1.0.0+other-build"),
                    release("v1.0.0-rc.1"),
                    release("invalid")
                ],
                "v1.0.0+this-build",
                UpdateChannel::Release,
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn nightly_updates_compare_commit_timestamps_and_skip_same_or_older_builds() {
        let selected = select_release(
            [
                release("v99.0.0"),
                release(NEW_NIGHTLY),
                release(OLD_NIGHTLY),
            ],
            OLD_NIGHTLY,
            UpdateChannel::Nightly,
        )
        .unwrap()
        .unwrap();
        assert_eq!(selected.tag_name, NEW_NIGHTLY);
        assert!(
            select_release(
                [
                    release(OLD_NIGHTLY),
                    release(NEW_NIGHTLY),
                    release("nightly-invalid")
                ],
                NEW_NIGHTLY,
                UpdateChannel::Nightly,
            )
            .unwrap()
            .is_none()
        );
        assert!(ReleaseVersion::parse("nightly-2026-09-07-1788708123-b81b0727ffb3").is_none());
    }

    #[test]
    fn explicit_channel_switch_offers_the_latest_of_the_chosen_channel() {
        assert_eq!(
            select_release(
                [release(NEW_NIGHTLY), release("v0.1.0-alpha.1")],
                NEW_NIGHTLY,
                UpdateChannel::Release,
            )
            .unwrap()
            .unwrap()
            .tag_name,
            "v0.1.0-alpha.1"
        );
        assert_eq!(
            select_release(
                [
                    release(OLD_NIGHTLY),
                    release(NEW_NIGHTLY),
                    release("v9.0.0")
                ],
                "v1.0.0",
                UpdateChannel::Nightly,
            )
            .unwrap()
            .unwrap()
            .tag_name,
            NEW_NIGHTLY
        );
    }

    #[test]
    fn packages_match_all_published_platforms_exactly() {
        for (os, arch, abi, suffix) in [
            ("macos", "aarch64", "", SUFFIX),
            ("macos", "x86_64", "", "x86_64-apple-darwin.zip"),
            ("linux", "x86_64", "gnu", "x86_64-unknown-linux-gnu.tar.gz"),
            ("windows", "x86_64", "msvc", "x86_64-pc-windows-msvc.zip"),
        ] {
            assert_eq!(platform_asset_suffix(os, arch, abi).unwrap(), suffix);
            let release = serde_json::from_value(release_json("v1.0.0", suffix)).unwrap();
            assert!(
                select_package(release, suffix)
                    .unwrap()
                    .asset
                    .name
                    .ends_with(suffix)
            );
        }
        assert!(platform_asset_suffix("linux", "aarch64", "gnu").is_err());
        assert!(platform_asset_suffix("linux", "x86_64", "musl").is_err());
        assert!(platform_asset_suffix("windows", "x86_64", "gnu").is_err());
        assert!(select_package(release("v1.0.0"), "x86_64-apple-darwin.zip").is_err());
        for (field, value) in [
            ("name", json!("../../update.zip")),
            (
                "browser_download_url",
                json!("https://example.com/update.zip"),
            ),
            ("size", json!(0)),
            ("digest", Value::Null),
            ("digest", json!("sha256:invalid")),
        ] {
            let mut data = release_json("v1.0.0", SUFFIX);
            data["assets"][0][field] = value;
            assert!(select_package(serde_json::from_value(data).unwrap(), SUFFIX).is_err());
        }
    }

    #[test]
    fn follows_pagination_and_finds_versioned_prereleases_after_nightlies() {
        let requests = Arc::new(AtomicUsize::new(0));
        let count = requests.clone();
        let http = FakeHttpClient::create(move |request| {
            count.fetch_add(1, AtomicOrdering::SeqCst);
            async move {
                assert!(
                    request.headers()["User-Agent"]
                        .to_str()
                        .unwrap()
                        .starts_with("Nexus-Agent/")
                );
                assert_eq!(
                    request.extensions().get::<RequestTimeout>().unwrap().0,
                    Duration::from_secs(30)
                );
                let first_page = request.uri().query().unwrap().ends_with("page=1");
                let mut response = Response::builder();
                if first_page {
                    response = response.header(
                        "link",
                        format!("<{RELEASES_URL}?per_page=100&page=2>; rel=\"next\""),
                    );
                }
                let body = if first_page {
                    json!([release_json(NEW_NIGHTLY, SUFFIX)])
                } else {
                    json!([release_json("v1.0.0-alpha.2", SUFFIX)])
                };
                Ok(response.body(body.to_string().into())?)
            }
        });
        let package = smol::block_on(find_package(
            http.as_ref(),
            "v1.0.0-alpha.1",
            UpdateChannel::Release,
            SUFFIX,
        ))
        .unwrap()
        .unwrap();
        assert_eq!(package.tag, "v1.0.0-alpha.2");
        assert_eq!(requests.load(AtomicOrdering::SeqCst), 2);
    }

    #[test]
    fn github_errors_and_missing_platform_assets_are_not_reported_as_up_to_date() {
        for status in [403, 429, 500] {
            let http = FakeHttpClient::create(move |_| async move {
                Ok(Response::builder()
                    .status(status)
                    .body("unavailable".into())?)
            });
            let error = smol::block_on(find_package(
                http.as_ref(),
                "v1.0.0",
                UpdateChannel::Release,
                SUFFIX,
            ))
            .unwrap_err();
            assert!(error.to_string().contains(&status.to_string()));
        }
        let http = FakeHttpClient::create(|_| async move {
            Ok(Response::builder().body(
                json!([
                    release_json("v1.0.2", "x86_64-apple-darwin.zip"),
                    release_json("v1.0.1", SUFFIX),
                ])
                .to_string()
                .into(),
            )?)
        });
        assert!(
            smol::block_on(find_package(
                http.as_ref(),
                "v1.0.0",
                UpdateChannel::Release,
                SUFFIX
            ))
            .is_err()
        );
    }

    #[test]
    fn checks_without_downloading_then_downloads_verifies_and_repairs_cache() {
        let directory = tempfile::tempdir().unwrap();
        let downloads = Arc::new(AtomicUsize::new(0));
        let count = downloads.clone();
        let http = FakeHttpClient::create(move |request| {
            let listing = request.uri().host() == Some("api.github.com");
            if !listing {
                count.fetch_add(1, AtomicOrdering::SeqCst);
            }
            async move {
                let body = if listing {
                    json!([release_json("v1.0.1", SUFFIX)]).to_string()
                } else {
                    "abc".into()
                };
                Ok(Response::builder().body(body.into())?)
            }
        });
        let state = smol::block_on(check(
            http.as_ref(),
            "v1.0.0",
            UpdateChannel::Release,
            SUFFIX,
        ))
        .unwrap();
        let UpdateState::Available(package) = state else {
            panic!("expected an available update")
        };
        assert_eq!(downloads.load(AtomicOrdering::SeqCst), 0);
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
        assert_eq!(package.notes, "## Changes\n\n- Fix application updates.");
        assert_eq!(
            package.release_url(),
            "https://github.com/ji233-Sun/nexus-agent/releases/tag/v1.0.1"
        );
        let mut progress = Vec::new();
        let state = smol::block_on(download(
            http.as_ref(),
            package,
            directory.path(),
            |state| {
                progress.push(state);
                Ok(())
            },
        ))
        .unwrap();
        let UpdateState::Ready { package, path } = state else {
            panic!("expected verified package")
        };
        assert_eq!(package.tag, "v1.0.1");
        assert_eq!(package.notes, "## Changes\n\n- Fix application updates.");
        assert_eq!(fs::read(&path).unwrap(), b"abc");
        assert!(matches!(
            progress.last(),
            Some(UpdateState::Downloading { received: 3, .. })
        ));
        let package = select_package(release("v1.0.1"), SUFFIX).unwrap();
        assert_eq!(
            smol::block_on(download_package(
                http.as_ref(),
                &package,
                directory.path(),
                |_| Ok(())
            ))
            .unwrap(),
            path
        );
        assert_eq!(downloads.load(AtomicOrdering::SeqCst), 1);
        fs::write(&path, "bad").unwrap();
        smol::block_on(download_package(
            http.as_ref(),
            &package,
            directory.path(),
            |_| Ok(()),
        ))
        .unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"abc");
        assert_eq!(downloads.load(AtomicOrdering::SeqCst), 2);
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn rejects_truncated_oversized_and_corrupt_downloads_and_cleans_partial_files() {
        for payload in ["ab", "abcd", "bad"] {
            let directory = tempfile::tempdir().unwrap();
            let package = select_package(release("v1.0.1"), SUFFIX).unwrap();
            let http = FakeHttpClient::create(move |_| async move {
                Ok(Response::builder().body(payload.into())?)
            });
            assert!(
                smol::block_on(download_package(
                    http.as_ref(),
                    &package,
                    directory.path(),
                    |_| Ok(())
                ))
                .is_err()
            );
            assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
        }
    }

    #[test]
    fn interrupted_stream_leaves_no_package_and_can_be_retried() {
        struct BrokenReader;
        impl smol::io::AsyncRead for BrokenReader {
            fn poll_read(
                self: std::pin::Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
                _: &mut [u8],
            ) -> std::task::Poll<std::io::Result<usize>> {
                std::task::Poll::Ready(Err(std::io::Error::other("connection interrupted")))
            }
        }
        let http = FakeHttpClient::create(|_| async move {
            Ok(Response::builder().body(AsyncBody::from_reader(
                smol::io::Cursor::new(b"ab").chain(BrokenReader),
            ))?)
        });
        let directory = tempfile::tempdir().unwrap();
        let package = select_package(release("v1.0.1"), SUFFIX).unwrap();
        assert!(
            smol::block_on(download_package(
                http.as_ref(),
                &package,
                directory.path(),
                |_| Ok(())
            ))
            .is_err()
        );
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
        let http =
            FakeHttpClient::create(|_| async move { Ok(Response::builder().body("abc".into())?) });
        let path = smol::block_on(download_package(
            http.as_ref(),
            &package,
            directory.path(),
            |_| Ok(()),
        ))
        .unwrap();
        assert_eq!(fs::read(path).unwrap(), b"abc");
    }
}
