//! Immutable attachment snapshots, independent of the history database.

use anyhow::{Context as _, Result};
use std::{fs, path::Path};
use uuid::Uuid;

// Called on a worker thread. Snapshot the contents so queues and history do
// not depend on the lifetime or subsequent edits of the original file.
pub(crate) fn import_attachment(
    directory: &Path,
    source: &Path,
) -> Result<nexus_domain::Attachment> {
    use nexus_domain::{Attachment, AttachmentKind};
    use std::io::{Read as _, Seek as _};
    let metadata = source.metadata().context("无法读取附件")?;
    anyhow::ensure!(metadata.is_file(), "只能添加普通文件，不能添加文件夹。");
    anyhow::ensure!(
        metadata.len() <= Attachment::MAX_FILE_BYTES as u64,
        "单个文件不能超过 100 MiB。"
    );
    let name = source
        .file_name()
        .and_then(|name| name.to_str())
        .context("附件文件名无效")?;
    let mut input = fs::File::open(source)?;
    let mut header = [0; 33];
    let length = input.read(&mut header)?;
    let is_image = nexus_harness_core::image_media_type(&header[..length]).is_some();
    // A broken image should not silently become a generic file.
    let image_extension = source
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            ["png", "jpg", "jpeg", "gif", "webp"]
                .iter()
                .any(|image| ext.eq_ignore_ascii_case(image))
        });
    anyhow::ensure!(
        is_image || !image_extension,
        "图片格式无效，请使用 PNG、JPEG、GIF 或 WebP。"
    );
    input.rewind()?;
    if is_image {
        let mut bytes = Vec::new();
        input
            .take(Attachment::MAX_IMAGE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        save_image_attachment(directory, name, &bytes)
    } else {
        persist_attachment(
            directory,
            name,
            source.extension(),
            AttachmentKind::File,
            input,
        )
    }
}

pub(crate) fn save_image_attachment(
    directory: &Path,
    name: &str,
    bytes: &[u8],
) -> Result<nexus_domain::Attachment> {
    nexus_harness_core::validate_image(bytes).map_err(anyhow::Error::msg)?;
    let extension = match nexus_harness_core::image_media_type(bytes) {
        Some("image/png") => "png",
        Some("image/jpeg") => "jpg",
        Some("image/gif") => "gif",
        Some("image/webp") => "webp",
        _ => unreachable!("validated image"),
    };
    persist_attachment(
        directory,
        name,
        Some(std::ffi::OsStr::new(extension)),
        nexus_domain::AttachmentKind::Image,
        bytes,
    )
}

fn persist_attachment(
    directory: &Path,
    name: &str,
    extension: Option<&std::ffi::OsStr>,
    kind: nexus_domain::AttachmentKind,
    input: impl std::io::Read,
) -> Result<nexus_domain::Attachment> {
    use nexus_domain::{Attachment, AttachmentKind};
    let limit = match kind {
        AttachmentKind::Image => Attachment::MAX_IMAGE_BYTES,
        AttachmentKind::File => Attachment::MAX_FILE_BYTES,
    };
    fs::create_dir_all(directory)?;
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    let copied = std::io::copy(&mut input.take(limit as u64 + 1), &mut file)?;
    anyhow::ensure!(copied <= limit as u64, "附件大小超出限制。");
    // Preserve the extension for tools that infer the file type from its path.
    let mut path = directory.join(Uuid::new_v4().to_string());
    if let Some(extension) = extension {
        path.set_extension(extension);
    }
    file.persist_noclobber(&path)?;
    Ok(Attachment {
        path: path.canonicalize()?.to_string_lossy().into_owned(),
        source_name: name.to_owned(),
        page: None,
        kind,
    })
}

pub(crate) fn save_pdf_capture(
    directory: &Path,
    name: &str,
    page: u32,
    bytes: &[u8],
) -> Result<nexus_domain::Attachment> {
    nexus_harness_core::validate_capture(bytes).map_err(anyhow::Error::msg)?;
    anyhow::ensure!(page > 0 && !name.trim().is_empty(), "截图来源无效");
    let name: String = name.chars().take(255).collect();
    let mut image = save_image_attachment(directory, &name, bytes)?;
    image.page = Some(page);
    Ok(image)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_images_by_content_and_rejects_invalid_or_oversized_files() {
        use base64::Engine as _;
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("attachments");
        let source = directory.path().join("misnamed.txt");
        let gif = base64::engine::general_purpose::STANDARD
            .decode("R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7")
            .unwrap();
        fs::write(&source, &gif).unwrap();
        let image = import_attachment(&root, &source).unwrap();
        assert!(image.is_image());
        assert_eq!(image.page, None);
        assert!(image.path.ends_with(".gif"));
        assert_eq!(
            nexus_harness_core::read_image_attachment(&image).unwrap(),
            gif
        );
        let invalid = directory.path().join("broken.PNG");
        fs::write(&invalid, b"not an image").unwrap();
        assert!(import_attachment(&root, &invalid).is_err());
        assert!(import_attachment(&root, directory.path()).is_err());
        let large = directory.path().join("large.txt");
        fs::File::create(&large)
            .unwrap()
            .set_len(nexus_domain::Attachment::MAX_FILE_BYTES as u64 + 1)
            .unwrap();
        assert!(import_attachment(&root, &large).is_err());
        let large_image = directory.path().join("large.gif");
        fs::write(&large_image, &gif).unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(&large_image)
            .unwrap()
            .set_len(nexus_domain::Attachment::MAX_IMAGE_BYTES as u64 + 1)
            .unwrap();
        assert!(import_attachment(&root, &large_image).is_err());
        assert_eq!(fs::read_dir(root).unwrap().count(), 1);
    }
}
