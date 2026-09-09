use super::*;
use crate::{
    i18n::LocalizedText,
    infrastructure::cnb_media::{LoadedMedia, MediaKind, MediaSource},
};
use gpui::{App, ImageSource, ObjectFit, StyledImage as _, Task};
use gpui_kit::component::text::{
    MarkdownExtensions, MarkdownNode, MarkdownParseContext, MarkdownPlugin, markdown_ast::Node,
};
use std::path::PathBuf;

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod player;

pub(super) fn extensions(
    repository: &str,
    cli: Option<&Path>,
    locale: Language,
) -> MarkdownExtensions {
    MarkdownExtensions::default().plugin(MediaPlugin {
        repository: repository.to_owned(),
        cli: cli.map(Path::to_path_buf),
        locale,
    })
}

struct MediaPlugin {
    repository: String,
    cli: Option<PathBuf>,
    locale: Language,
}

enum Part {
    Markdown(String),
    Media { source: MediaSource, label: String },
}

struct MediaParagraph {
    offset: usize,
    parts: Vec<Part>,
}

fn media_node(node: &Node, repository: &str) -> Option<(MediaSource, String)> {
    let (url, label, image) = match node {
        Node::Image(image) => (image.url.as_str(), image.alt.clone(), true),
        Node::Link(link) => (
            link.url.as_str(),
            link.children.iter().map(Node::to_string).collect(),
            false,
        ),
        Node::Text(text)
            if text.value.trim().starts_with("https://")
                || text.value.trim().starts_with("http://") =>
        {
            (text.value.trim(), text.value.trim().to_owned(), false)
        }
        _ => return None,
    };
    let source = MediaSource::new(url, &label, image, repository)?;
    Some((source, label))
}

impl MarkdownPlugin for MediaPlugin {
    fn name(&self) -> &str {
        "cnb-media"
    }

    fn is_block(&self) -> bool {
        true
    }

    fn parse(&self, node: &Node, cx: &MarkdownParseContext<'_>) -> Option<MarkdownNode> {
        let Node::Paragraph(paragraph) = node else {
            return None;
        };
        let mut parts = Vec::new();
        let mut text = String::new();
        let mut found = false;
        for child in &paragraph.children {
            if let Some((source, label)) = media_node(child, &self.repository) {
                if !text.trim().is_empty() {
                    parts.push(Part::Markdown(std::mem::take(&mut text)));
                } else {
                    text.clear();
                }
                parts.push(Part::Media { source, label });
                found = true;
            } else if let Some(source) = cx.node_source(child) {
                text.push_str(source);
            }
        }
        if !found {
            return None;
        }
        if !text.trim().is_empty() {
            parts.push(Part::Markdown(text));
        }
        Some(
            MarkdownNode::new(
                self.name().to_owned(),
                MediaParagraph {
                    offset: cx.offset()
                        + node.position().map_or(0, |position| position.start.offset),
                    parts,
                },
            )
            .text(node.to_string())
            .markdown(cx.node_source(node).unwrap_or_default()),
        )
    }

    fn render(&self, node: &MarkdownNode, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let mut body = div().w_full().min_w_0().flex().flex_col().gap_3();
        let paragraph = node.data::<MediaParagraph>().expect("media paragraph");
        for (index, part) in paragraph.parts.iter().enumerate() {
            match part {
                Part::Markdown(text) => {
                    body = body.child(TextView::markdown(
                        SharedString::from(format!("media-text-{}-{index}", paragraph.offset)),
                        text.clone(),
                    ));
                }
                Part::Media { source, label } => {
                    let state = window.use_keyed_state(
                        SharedString::from(format!(
                            "cnb-media-{}-{index}-{}",
                            paragraph.offset, source.url
                        )),
                        cx,
                        |_, cx| {
                            MediaView::new(
                                source.clone(),
                                label.clone(),
                                self.cli.clone(),
                                self.locale,
                                cx,
                            )
                        },
                    );
                    state.update(cx, |state, _| state.locale = self.locale);
                    body = body.child(state);
                }
            }
        }
        body
    }
}

struct MediaView {
    source: MediaSource,
    label: String,
    cli: Option<PathBuf>,
    locale: Language,
    loaded: Option<Result<LoadedMedia, LocalizedText>>,
    task: Task<()>,
    abort: Option<tokio::task::AbortHandle>,
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    player: Option<Result<player::Player, LocalizedText>>,
}

impl MediaView {
    fn new(
        source: MediaSource,
        label: String,
        cli: Option<PathBuf>,
        locale: Language,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.on_release(|view, cx| {
            if view.source.kind == MediaKind::Image
                && let Some(Ok(LoadedMedia::Local(file))) = &view.loaded
            {
                ImageSource::from(file.path.clone()).remove_asset(cx);
            }
        })
        .detach();
        let mut view = Self {
            source,
            label,
            cli,
            locale,
            loaded: None,
            task: Task::ready(()),
            abort: None,
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            player: None,
        };
        view.load(cx);
        view
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        if let Some(abort) = self.abort.take() {
            abort.abort();
        }
        self.loaded = None;
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            self.player = None;
        }
        let source = self.source.clone();
        let cli = self.cli.clone();
        let task =
            reqwest_client::runtime().spawn(async move { source.load(cli.as_deref()).await });
        self.abort = Some(task.abort_handle());
        self.task = cx.spawn(async move |view, cx| {
            let result = match task.await {
                Ok(result) => result,
                Err(error) => Err(error.into()),
            }
            .map_err(|error| {
                LocalizedText::new("素材加载失败：{error}", &[("error", error.to_string())])
            });
            let _ = view.update(cx, |view, cx| {
                view.loaded = Some(result);
                cx.notify();
            });
        });
        cx.notify();
    }

    fn error(&self, error: &LocalizedText, cx: &Context<Self>) -> impl IntoElement {
        div()
            .p_3()
            .flex()
            .flex_col()
            .gap_2()
            .child(error.render(self.locale).to_owned())
            .child(
                Button::new("retry-media")
                    .label(self.locale.text("重试"))
                    .on_click(cx.listener(|view, _, _, cx| view.load(cx))),
            )
    }
}

impl Drop for MediaView {
    fn drop(&mut self) {
        if let Some(abort) = &self.abort {
            abort.abort();
        }
    }
}

impl Render for MediaView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = palette(cx);
        let frame = div()
            .debug_selector({
                let kind = self.source.kind;
                move || {
                    match kind {
                        MediaKind::Image => "cnb-media-image",
                        MediaKind::Video => "cnb-media-video",
                        MediaKind::Audio => "cnb-media-audio",
                    }
                    .into()
                }
            })
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_2()
            .text_size(px(12.))
            .text_color(rgb(colors.muted));
        let content = match &self.loaded {
            None => div()
                .p_4()
                .child(self.locale.text("正在加载素材…"))
                .into_any_element(),
            Some(Err(error)) => self.error(error, cx).into_any_element(),
            Some(Ok(media)) if self.source.kind == MediaKind::Image => {
                let source = match media {
                    LoadedMedia::Local(file) => ImageSource::from(file.path.clone()),
                    LoadedMedia::Remote(url) => ImageSource::from(url.to_string()),
                };
                let locale = self.locale;
                gpui::img(source)
                    .w_full()
                    .max_h(px(480.))
                    .object_fit(ObjectFit::Contain)
                    .with_loading(move || {
                        div()
                            .p_4()
                            .child(locale.text("正在加载素材…"))
                            .into_any_element()
                    })
                    .with_fallback(move || {
                        div()
                            .p_4()
                            .child(locale.text("图片无法加载，请打开原始文件查看。"))
                            .into_any_element()
                    })
                    .into_any_element()
            }
            Some(Ok(media)) => {
                #[cfg(any(target_os = "macos", target_os = "windows"))]
                {
                    if self.player.is_none() {
                        self.player = Some(
                            player::Player::new(
                                media.clone(),
                                self.source.kind,
                                self.locale,
                                window,
                                cx,
                            )
                            .map_err(|error| {
                                LocalizedText::new(
                                    "素材加载失败：{error}",
                                    &[("error", error.to_string())],
                                )
                            }),
                        );
                    }
                    match self.player.as_ref().expect("player initialized") {
                        Ok(player) => player.element(self.source.kind).into_any_element(),
                        Err(error) => self.error(error, cx).into_any_element(),
                    }
                }
                #[cfg(not(any(target_os = "macos", target_os = "windows")))]
                {
                    let _ = (media, window);
                    div()
                        .p_4()
                        .child(
                            self.locale
                                .text("此平台暂不支持内嵌播放，请打开原始文件播放。"),
                        )
                        .into_any_element()
                }
            }
        };
        let original = self
            .loaded
            .as_ref()
            .and_then(|loaded| loaded.as_ref().ok())
            .map(LoadedMedia::url)
            .unwrap_or_else(|| self.source.url.clone())
            .to_string();
        frame.child(content).child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .child(div().min_w_0().truncate().child(self.label.clone()))
                .child(
                    Button::new("open-media")
                        .ghost()
                        .label(self.locale.text("打开原始文件"))
                        .on_click(move |_, _, cx| cx.open_url(&original)),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::component::text::markdown_ast::{Image, Link, Text};

    #[test]
    fn cnb_markdown_media_recognizes_images_links_and_bare_urls() {
        let image = Node::Image(Image {
            url: "/-/imgs/issues/id/asset".into(),
            alt: "截图".into(),
            title: None,
            position: None,
        });
        assert_eq!(
            media_node(&image, "team/repo").unwrap().0.kind,
            MediaKind::Image
        );
        let link = Node::Link(Link {
            url: "undefined/team/repo/-/files/issues/id/clip.mp4".into(),
            children: vec![Node::Text(Text {
                value: "clip.mp4".into(),
                position: None,
            })],
            title: None,
            position: None,
        });
        let (source, label) = media_node(&link, "team/repo").unwrap();
        assert_eq!(source.kind, MediaKind::Video);
        assert_eq!(label, "clip.mp4");
        let audio = Node::Text(Text {
            value: "https://example.test/sound.mp3?signature=abc".into(),
            position: None,
        });
        assert_eq!(
            media_node(&audio, "team/repo").unwrap().0.kind,
            MediaKind::Audio
        );
        let prose = Node::Text(Text {
            value: "这个文件叫 demo.mp4".into(),
            position: None,
        });
        assert!(media_node(&prose, "team/repo").is_none());
    }
}
