use anyhow::{Context as _, Result, ensure};
use gpui_kit::TextSystem;
use sha2::{Digest as _, Sha256};
use std::{
    borrow::Cow,
    fs,
    io::Write as _,
    path::{Path, PathBuf},
};

pub(crate) fn directory() -> Result<PathBuf> {
    Ok(super::paths::data_directory()?.join("fonts"))
}

fn read_font(source: &Path) -> Result<Vec<u8>> {
    let extension = source
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    ensure!(
        matches!(extension.as_str(), "ttf" | "otf"),
        "请选择 TTF 或 OTF 字体文件"
    );
    let bytes = fs::read(source).context("无法读取字体文件")?;
    // Reject collections and non-font files even when their extension was renamed.
    ensure!(
        bytes.starts_with(&[0, 1, 0, 0]) || bytes.starts_with(b"OTTO"),
        "不是有效的 TTF 或 OTF 字体"
    );
    Ok(bytes)
}

pub(crate) fn import(directory: &Path, source: &Path, text_system: &TextSystem) -> Result<()> {
    let bytes = read_font(source)?;
    let extension = if bytes.starts_with(b"OTTO") {
        "otf"
    } else {
        "ttf"
    };
    let name = format!("{:x}.{extension}", Sha256::digest(&bytes));
    fs::create_dir_all(directory).context("无法创建字体目录")?;
    // Keep an app-owned copy, deduplicated by content, with no partial files on restart.
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    file.write_all(&bytes)?;
    text_system
        .add_fonts(vec![Cow::Owned(bytes)])
        .context("无法加载字体文件")?;
    match file.persist_noclobber(directory.join(name)) {
        Ok(_) => Ok(()),
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error.error).context("无法保存字体文件"),
    }
}

pub(crate) fn load(directory: &Path, text_system: &TextSystem) -> Result<Vec<String>> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).context("无法读取字体目录"),
    };
    let mut errors = Vec::new();
    for entry in entries {
        let result = (|| -> Result<()> {
            let path = entry?.path();
            if matches!(
                path.extension().and_then(|value| value.to_str()),
                Some("ttf" | "otf")
            ) {
                let bytes = read_font(&path)?;
                text_system
                    .add_fonts(vec![Cow::Owned(bytes)])
                    .with_context(|| format!("无法加载 {}", path.display()))?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            errors.push(format!("{error:#}"));
        }
    }
    Ok(errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui_kit::test]
    fn imported_files_survive_source_removal_and_duplicates_share_one_copy(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let fonts = directory.path().join("fonts");
        assert!(load(&fonts, cx.text_system()).unwrap().is_empty());
        let source_directory = tempfile::tempdir().unwrap();
        let source = source_directory.path().join("用户字体.TTF");
        // GPUI's test text system skips decoding; this test covers the file lifecycle.
        let bytes = b"\x00\x01\x00\x00font bytes";
        fs::write(&source, bytes).unwrap();
        import(&fonts, &source, cx.text_system()).unwrap();
        import(&fonts, &source, cx.text_system()).unwrap();
        let copies = fs::read_dir(&fonts).unwrap().collect::<Vec<_>>();
        assert_eq!(copies.len(), 1);
        let copy = copies[0].as_ref().unwrap().path();
        assert_eq!(fs::read(copy).unwrap(), bytes);
        drop(source_directory);
        assert!(load(&fonts, cx.new_app().text_system()).unwrap().is_empty());

        fs::write(fonts.join("corrupt.ttf"), b"invalid").unwrap();
        fs::write(fonts.join("unfinished.tmp"), b"incomplete").unwrap();
        assert_eq!(load(&fonts, cx.text_system()).unwrap().len(), 1);
    }

    #[gpui_kit::test]
    fn invalid_imports_do_not_save_files(cx: &mut gpui_kit::TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let fonts = directory.path().join("fonts");
        for (name, bytes) in [
            ("bad.ttf", b"not a font".as_slice()),
            ("collection.ttf", b"ttcf".as_slice()),
            ("font.woff", b"OTTO".as_slice()),
        ] {
            let source = directory.path().join(name);
            fs::write(&source, bytes).unwrap();
            assert!(import(&fonts, &source, cx.text_system()).is_err());
        }
        assert!(!fonts.exists());
    }
}
