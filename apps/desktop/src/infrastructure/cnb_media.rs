use super::cnb::{parse_response, repository_from_remote, run_with_temp_dir};
use anyhow::{Context as _, Result, ensure};
use reqwest::Url;
use std::{path::Path, path::PathBuf, sync::Arc, time::Duration};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MediaKind {
    Image,
    Video,
    Audio,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MediaSource {
    pub(crate) url: Url,
    pub(crate) kind: MediaKind,
}

impl MediaSource {
    pub(crate) fn new(raw: &str, label: &str, image: bool, repository: &str) -> Option<Self> {
        let url = if let Some(path) = raw.strip_prefix("undefined/") {
            Url::parse(&format!("https://cnb.cool/{path}")).ok()?
        } else {
            let base = Url::parse(&format!("https://cnb.cool/{repository}/")).ok()?;
            if raw.starts_with("/-/") {
                base.join(raw.trim_start_matches('/')).ok()?
            } else {
                base.join(raw).ok()?
            }
        };
        if !matches!(url.scheme(), "https" | "http")
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return None;
        }
        let kind = if image {
            MediaKind::Image
        } else {
            media_kind(url.path()).or_else(|| media_kind(label))?
        };
        Some(Self { url, kind })
    }

    fn download_args(&self) -> Option<Vec<String>> {
        if !matches!(self.url.host_str()?, "cnb.cool" | "api.cnb.cool") {
            return None;
        }
        let (repository, resource) = self.url.path().trim_start_matches('/').split_once("/-/")?;
        let repository = repository_from_remote(&format!("https://cnb.cool/{repository}"))?;
        let (group, action, flag, path) = if let Some(path) = resource.strip_prefix("files/issues/")
        {
            ("issues", "get-issue-files", "--file-path", path)
        } else if let Some(path) = resource.strip_prefix("imgs/issues/") {
            ("issues", "get-issue-imgs", "--img-path", path)
        } else if let Some(path) = resource.strip_prefix("files/") {
            ("assets", "get-files", "--filePath", path)
        } else {
            (
                "assets",
                "get-imgs",
                "--imgPath",
                resource.strip_prefix("imgs/")?,
            )
        };
        Some(
            [
                group,
                action,
                "--repo",
                &repository,
                flag,
                path,
                "--verbose",
            ]
            .map(str::to_owned)
            .to_vec(),
        )
    }

    pub(crate) async fn load(&self, cli: Option<&Path>) -> Result<LoadedMedia> {
        let Some(args) = self.download_args() else {
            return Ok(LoadedMedia::Remote(self.url.clone()));
        };
        let cli = cli.context("未检测到 CNB CLI，请先安装并运行 cnb login。")?;
        let directory = tempfile::Builder::new()
            .prefix("nexus-cnb-media-")
            .tempdir()?;
        let output =
            run_with_temp_dir(cli, &args, Duration::from_secs(120), Some(directory.path())).await?;
        let (path, _): (PathBuf, _) = parse_response(&output)?;
        let path = path
            .canonicalize()
            .context("CNB CLI 未返回可读取的附件。")?;
        ensure!(
            path.starts_with(directory.path().canonicalize()?) && path.is_file(),
            "CNB CLI 返回的附件路径无效。"
        );
        Ok(LoadedMedia::Local(Arc::new(LocalMedia {
            path,
            _directory: directory,
        })))
    }
}

fn media_kind(filename: &str) -> Option<MediaKind> {
    match filename.rsplit('.').next()?.to_ascii_lowercase().as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" | "avif" => Some(MediaKind::Image),
        "mp4" | "m4v" | "mov" | "webm" | "ogv" => Some(MediaKind::Video),
        "mp3" | "m4a" | "aac" | "wav" | "ogg" | "oga" | "flac" | "opus" => Some(MediaKind::Audio),
        _ => None,
    }
}

#[derive(Clone, Debug)]
pub(crate) enum LoadedMedia {
    Remote(Url),
    Local(Arc<LocalMedia>),
}

#[derive(Debug)]
pub(crate) struct LocalMedia {
    pub(crate) path: PathBuf,
    pub(crate) _directory: tempfile::TempDir,
}

impl LoadedMedia {
    pub(crate) fn url(&self) -> Url {
        match self {
            Self::Remote(url) => url.clone(),
            Self::Local(file) => Url::from_file_path(&file.path).expect("absolute attachment path"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cnb_media_normalizes_attachment_links_and_selects_authenticated_endpoints() {
        for prefix in [
            "undefined/team/repo",
            "https://cnb.cool/team/repo",
            "/team/repo",
            "",
        ] {
            let source = MediaSource::new(
                &format!("{prefix}/-/files/issues/123/id/demo.MP4?download=1#video"),
                "demo.MP4",
                false,
                "team/repo",
            )
            .unwrap();
            assert_eq!(source.kind, MediaKind::Video);
            assert_eq!(
                source.download_args().unwrap(),
                [
                    "issues",
                    "get-issue-files",
                    "--repo",
                    "team/repo",
                    "--file-path",
                    "123/id/demo.MP4",
                    "--verbose",
                ]
            );
        }
        for (path, group, action, flag) in [
            ("imgs/issues", "issues", "get-issue-imgs", "--img-path"),
            ("imgs", "assets", "get-imgs", "--imgPath"),
            ("files", "assets", "get-files", "--filePath"),
        ] {
            let source =
                MediaSource::new(&format!("/-/{path}/id/photo.png"), "", true, "team/repo")
                    .unwrap();
            assert_eq!(
                source.download_args().unwrap(),
                [
                    group,
                    action,
                    "--repo",
                    "team/repo",
                    flag,
                    "id/photo.png",
                    "--verbose"
                ]
            );
        }
        let audio = MediaSource::new(
            "https://cdn.example.test/sound.ogg?sig=abc",
            "",
            false,
            "team/repo",
        )
        .unwrap();
        assert_eq!(audio.kind, MediaKind::Audio);
        assert!(audio.download_args().is_none());
        assert_eq!(audio.url.query(), Some("sig=abc"));
        for unsafe_url in [
            "file:///private/demo.mp4",
            "javascript:demo.mp4",
            "data:text/html,demo.mp4",
            "https://user:secret@cnb.cool/demo.mp4",
        ] {
            assert!(MediaSource::new(unsafe_url, "", false, "team/repo").is_none());
        }
        assert!(
            MediaSource::new(
                "https://example.test/readme.pdf",
                "document",
                false,
                "team/repo"
            )
            .is_none()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cnb_media_downloads_are_isolated_and_removed_when_released() {
        use std::os::unix::fs::PermissionsExt as _;
        let scripts = tempfile::tempdir().unwrap();
        let cli = scripts.path().join("cnb");
        std::fs::write(&cli, "#!/bin/sh\nmkdir -p \"$TMPDIR/cnb-api\"\nprintf 'image bytes' > \"$TMPDIR/cnb-api/photo.png\"\nprintf '{\"status\":200,\"data\":\"%s/cnb-api/photo.png\"}' \"$TMPDIR\"\n").unwrap();
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o700)).unwrap();
        let source = MediaSource::new("/-/imgs/issues/1/photo.png", "", true, "team/repo").unwrap();
        let first = source.load(Some(&cli)).await.unwrap();
        let second = source.load(Some(&cli)).await.unwrap();
        let path = first.url().to_file_path().unwrap();
        assert_ne!(first.url(), second.url());
        assert_eq!(std::fs::read(&path).unwrap(), b"image bytes");
        drop(first);
        assert!(!path.exists());
        assert!(second.url().to_file_path().unwrap().exists());
        std::fs::write(&cli, "#!/bin/sh\nprintf '{\"status\":403,\"data\":{}}'\n").unwrap();
        assert!(
            source
                .load(Some(&cli))
                .await
                .unwrap_err()
                .to_string()
                .contains("HTTP 403")
        );
        std::fs::write(
            &cli,
            format!(
                "#!/bin/sh\nprintf '{{\"status\":200,\"data\":\"{}\"}}'\n",
                cli.display()
            ),
        )
        .unwrap();
        assert!(source.load(Some(&cli)).await.is_err());
    }
}
