use super::*;
use crate::infrastructure::storage::Storage;
use std::path::PathBuf;

impl NexusView {
    pub(super) fn choose_attachments(&mut self, cx: &mut Context<Self>) {
        let conversation = self.presenter.model().conversation.id;
        cx.spawn(async move |view, cx| {
            if let Some(files) = rfd::AsyncFileDialog::new().pick_files().await {
                let paths = files.iter().map(|file| file.path().to_owned()).collect();
                let _ = view.update(cx, |view, cx| {
                    if view.presenter.model().conversation.id == conversation {
                        view.import_attachments(paths, Vec::new(), cx);
                    }
                });
            }
        })
        .detach();
    }

    pub(super) fn paste_attachments(
        &mut self,
        _: &gpui::component::input::Paste,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(clipboard) = cx.read_from_clipboard() else {
            return;
        };
        let mut paths = Vec::new();
        let mut images = Vec::new();
        for entry in clipboard.entries {
            match entry {
                gpui::ClipboardEntry::ExternalPaths(files) => {
                    paths.extend_from_slice(files.paths())
                }
                gpui::ClipboardEntry::Image(image) => images.push(image),
                gpui::ClipboardEntry::String(_) => {}
            }
        }
        if paths.is_empty() && images.is_empty() {
            return;
        }
        // File pastes must not replace selected prompt text with an empty string.
        cx.stop_propagation();
        self.import_attachments(paths, images, cx);
    }

    pub(super) fn import_attachments(
        &mut self,
        paths: Vec<PathBuf>,
        images: Vec<gpui::Image>,
        cx: &mut Context<Self>,
    ) {
        let Some((conversation, directory)) = self
            .presenter
            .begin_attachment_import(paths.len() + images.len())
        else {
            cx.notify();
            return;
        };
        let task = reqwest_client::runtime().spawn_blocking(move || {
            let mut results: Vec<_> = paths
                .iter()
                .map(|path| {
                    Storage::import_attachment(&directory, path)
                        .map_err(|error| format!("{}：{error}", path.display()))
                })
                .collect();
            for image in images {
                let name = format!("clipboard.{}", image.format().extension());
                let result = Storage::save_image_attachment(&directory, &name, image.bytes());
                results.push(result.map_err(|error| error.to_string()));
            }
            results
        });
        cx.spawn(async move |view, cx| {
            let results = task
                .await
                .unwrap_or_else(|error| vec![Err(error.to_string())]);
            let _ = view.update(cx, |view, cx| {
                view.presenter
                    .finish_attachment_import(conversation, results);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}
