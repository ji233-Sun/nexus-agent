use super::*;
use crate::model::{tools::ToolDetail, workspace::WorkspaceReview};

pub(super) struct ReviewPage {
    workspace_id: Uuid,
    selected: Option<(ReviewSection, String)>,
    target: Entity<InputState>,
    merge_open: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReviewSection {
    Unstaged,
    Untracked,
    Staged,
    Committed,
    Merge,
    Resolution,
    ResolutionStaged,
}

impl ReviewSection {
    fn label(self, locale: Language) -> &'static str {
        locale.text(match self {
            Self::Unstaged => "未暂存",
            Self::Untracked => "新文件",
            Self::Staged => "已暂存",
            Self::Committed => "已提交",
            Self::Merge => "合入预览",
            Self::Resolution => "冲突解决",
            Self::ResolutionStaged => "已暂存的冲突解决内容",
        })
    }
}

pub(super) struct ReviewFile {
    pub(super) path: String,
    pub(super) patch: String,
    section: ReviewSection,
    status: &'static str,
    pub(super) additions: usize,
    pub(super) deletions: usize,
}

impl ReviewFile {
    fn key(&self) -> (ReviewSection, String) {
        (self.section, self.path.clone())
    }

    pub(super) fn detail(&self, locale: Language) -> ToolDetail {
        // File headers belong in navigation, not in the selectable code document.
        // Keep hunk headers: they locate omitted context and anchor line numbers.
        let body = self
            .patch
            .find("\n@@")
            .map(|index| &self.patch[index + 1..]);
        let (text, diff) = if self.section == ReviewSection::Untracked {
            if self.patch.contains('\0') || self.patch.starts_with("二进制文件（") {
                (
                    locale.text("二进制文件，无法显示文本差异。").to_owned(),
                    false,
                )
            } else if self.patch.is_empty() {
                (locale.text("空文件").to_owned(), false)
            } else {
                let mut text = format!("@@ -0,0 +1,{} @@\n", self.patch.lines().count());
                for line in self.patch.split_inclusive('\n') {
                    text.push('+');
                    text.push_str(line);
                }
                (text, true)
            }
        } else if let Some(body) = body {
            (body.to_owned(), true)
        } else if self.patch.contains("GIT binary patch") || self.patch.contains("Binary files ") {
            (
                locale.text("二进制文件，无法显示文本差异。").to_owned(),
                false,
            )
        } else {
            let metadata = self
                .patch
                .lines()
                .filter(|line| {
                    line.starts_with("old mode ")
                        || line.starts_with("new mode ")
                        || line.starts_with("new file mode ")
                        || line.starts_with("deleted file mode ")
                })
                .collect::<Vec<_>>()
                .join("\n");
            (
                if metadata.is_empty() {
                    self.patch.clone()
                } else {
                    metadata
                },
                false,
            )
        };
        ToolDetail {
            title: self.path.clone(),
            text,
            language: Path::new(&self.path)
                .extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or("text")
                .into(),
            diff,
        }
    }
}

// Review patches are generated with --no-renames and fixed a/ and b/ prefixes.
// Git quotes non-ASCII bytes and control characters using C-style escapes.
fn patch_path(header: &str) -> String {
    let Some(paths) = header.strip_prefix("diff --git ") else {
        return header
            .strip_prefix("diff --cc ")
            .or_else(|| header.strip_prefix("diff --combined "))
            .map(decode_path)
            .unwrap_or_else(|| header.to_owned());
    };
    let source = if paths.starts_with('"') {
        decode_path(paths)
    } else {
        let middle = paths.len().saturating_sub(1) / 2;
        let first = paths.get(..middle).unwrap_or(paths);
        if first.strip_prefix("a/")
            == paths
                .get(middle + 1..)
                .and_then(|path| path.strip_prefix("b/"))
        {
            first.to_owned()
        } else {
            // Preserve unfamiliar metadata instead of silently dropping a patch.
            paths.to_owned()
        }
    };
    source.strip_prefix("a/").unwrap_or(&source).to_owned()
}

fn decode_path(text: &str) -> String {
    let Some(quoted) = text.strip_prefix('"') else {
        return text.to_owned();
    };
    let mut bytes = quoted.bytes().peekable();
    let mut decoded = Vec::new();
    while let Some(byte) = bytes.next() {
        match byte {
            b'"' => break,
            b'\\' => match bytes.next() {
                Some(first @ b'0'..=b'7') => {
                    let mut value = (first - b'0') as u16;
                    for _ in 0..2 {
                        if let Some(digit @ b'0'..=b'7') = bytes.peek().copied() {
                            bytes.next();
                            value = value * 8 + (digit - b'0') as u16;
                        } else {
                            break;
                        }
                    }
                    decoded.push(value as u8);
                }
                Some(escape) => decoded.push(match escape {
                    b't' => b'\t',
                    b'n' => b'\n',
                    b'r' => b'\r',
                    b'a' => 7,
                    b'b' => 8,
                    b'f' => 12,
                    b'v' => 11,
                    other => other,
                }),
                None => decoded.push(b'\\'),
            },
            other => decoded.push(other),
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

pub(super) fn patch_files(patch: &str, section: ReviewSection) -> Vec<ReviewFile> {
    let mut files: Vec<ReviewFile> = Vec::new();
    let mut in_hunk = false;
    for line in patch.split_inclusive('\n') {
        if line.starts_with("diff --") {
            files.push(ReviewFile {
                path: patch_path(line.trim_end_matches('\n')),
                patch: String::new(),
                section,
                status: "M",
                additions: 0,
                deletions: 0,
            });
            in_hunk = false;
        }
        if let Some(file) = files.last_mut() {
            if line.starts_with("@@") {
                in_hunk = true;
            }
            if in_hunk {
                file.additions += usize::from(line.starts_with('+'));
                file.deletions += usize::from(line.starts_with('-'));
            } else if line.starts_with("new file mode ") {
                file.status = "A";
            } else if line.starts_with("deleted file mode ") {
                file.status = "D";
            }
            file.patch.push_str(line);
        }
    }
    if files.is_empty() && !patch.is_empty() {
        files.push(ReviewFile {
            path: "HEAD".into(),
            patch: patch.into(),
            section,
            status: "M",
            additions: 0,
            deletions: 0,
        });
    }
    files
}

fn review_files(
    review: &WorkspaceReview,
    plan: Option<&crate::model::workspace::MergePlan>,
) -> Vec<ReviewFile> {
    let mut files = Vec::new();
    for (section, patch) in [
        (
            ReviewSection::Merge,
            plan.map_or("", |plan| plan.diff.as_str()),
        ),
        (ReviewSection::Resolution, review.resolution_diff.as_str()),
        (
            ReviewSection::ResolutionStaged,
            review.resolution_staged.as_str(),
        ),
        (ReviewSection::Unstaged, review.unstaged.as_str()),
    ] {
        files.extend(patch_files(patch, section));
    }
    files.extend(review.untracked.iter().map(|(path, text)| ReviewFile {
        path: path.clone(),
        patch: text.clone(),
        section: ReviewSection::Untracked,
        status: "A",
        additions: if text.contains('\0') || text.starts_with("二进制文件（") {
            0
        } else {
            text.lines().count()
        },
        deletions: 0,
    }));
    files.extend(patch_files(&review.staged, ReviewSection::Staged));
    files.extend(patch_files(&review.committed, ReviewSection::Committed));
    files
}

impl NexusView {
    pub(super) fn close_workspace_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.review_pages
            .remove(&self.presenter.model().conversation.id);
        self.prompt_input
            .update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    pub(super) fn open_workspace_review(
        &mut self,
        id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .presenter
            .model()
            .workspace_review
            .as_ref()
            .is_none_or(|review| review.workspace_id != id)
            && !self.presenter.review_workspace(id)
        {
            return;
        }
        let target = self
            .presenter
            .model()
            .selected_project
            .as_ref()
            .and_then(|project| {
                crate::infrastructure::git::current_branch(Path::new(&project.canonical_path))
            })
            .unwrap_or_default();
        let target = cx.new(|cx| InputState::new(window, cx).default_value(target));
        self.review_pages.insert(
            self.presenter.model().conversation.id,
            ReviewPage {
                workspace_id: id,
                selected: None,
                target,
                merge_open: false,
            },
        );
        self.focus_handle.focus(window, cx);
        cx.notify();
    }

    pub(super) fn render_workspace_review(
        &self,
        page: &ReviewPage,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let model = self.presenter.model();
        let locale = model.language;
        let colors = palette(cx);
        let owner = model.conversation.id;
        let id = page.workspace_id;
        let review = model
            .workspace_review
            .as_ref()
            .filter(|review| review.workspace_id == id);
        let plan = model
            .merge_plan
            .as_ref()
            .filter(|plan| plan.workspace_id == id);
        let workspace = model.workspaces.iter().find(|workspace| workspace.id == id);
        let files = review
            .map(|review| review_files(review, plan))
            .unwrap_or_default();
        let selected = page
            .selected
            .as_ref()
            .and_then(|key| files.iter().find(|file| file.key() == *key))
            .or_else(|| files.first());
        let branch = review
            .and_then(|review| review.branch.as_deref())
            .unwrap_or("HEAD");
        let head = review
            .map(|review| review.head.chars().take(8).collect::<String>())
            .unwrap_or_default();
        let mut navigation = div()
            .id("review-file-list")
            .debug_selector(|| "review-file-list".into())
            .w(px(220.))
            .flex_none()
            .h_full()
            .min_h_0()
            .overflow_y_scroll()
            .border_r_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.canvas))
            .p_2();
        let mut previous = None;
        for file in &files {
            if previous != Some(file.section) {
                let count = files
                    .iter()
                    .filter(|other| other.section == file.section)
                    .count();
                navigation = navigation.child(
                    div()
                        .h(px(32.))
                        .px_2()
                        .flex()
                        .items_center()
                        .justify_between()
                        .text_size(px(11.))
                        .text_color(rgb(colors.muted))
                        .child(file.section.label(locale))
                        .child(count.to_string()),
                );
                previous = Some(file.section);
            }
            let key = file.key();
            let active = selected.is_some_and(|selected| selected.key() == key);
            let filename = file
                .path
                .rsplit('/')
                .next()
                .unwrap_or(&file.path)
                .to_owned();
            let directory = file
                .path
                .rsplit_once('/')
                .map(|(directory, _)| directory.to_owned());
            navigation = navigation.child(
                Button::new(SharedString::from(format!(
                    "review-file-{:?}-{}",
                    file.section, file.path
                )))
                .debug_selector({
                    let path = file.path.clone();
                    let section = file.section;
                    move || format!("review-nav-{section:?}-{path}")
                })
                .ghost()
                .small()
                .w_full()
                .h(px(if directory.is_some() { 46. } else { 34. }))
                .px_2()
                .justify_start()
                .gap_2()
                .selected(active)
                .tooltip(file.path.clone())
                .child(
                    div()
                        .flex_none()
                        .text_size(px(11.))
                        .font_family(MONO_FONT)
                        .text_color(rgb(match file.status {
                            "A" => colors.success,
                            "D" => colors.danger,
                            _ => colors.muted,
                        }))
                        .child(file.status),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .items_start()
                        .gap(px(2.))
                        .child(div().w_full().truncate().text_size(px(12.)).child(filename))
                        .when_some(directory, |element, directory| {
                            element.child(
                                div()
                                    .w_full()
                                    .truncate()
                                    .text_size(px(10.))
                                    .text_color(rgb(colors.muted))
                                    .child(directory),
                            )
                        }),
                )
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .gap_1()
                        .text_size(px(10.))
                        .font_family(MONO_FONT)
                        .when(file.additions > 0, |element| {
                            element.child(
                                div()
                                    .text_color(rgb(colors.success))
                                    .child(format!("+{}", file.additions)),
                            )
                        })
                        .when(file.deletions > 0, |element| {
                            element.child(
                                div()
                                    .text_color(rgb(colors.danger))
                                    .child(format!("−{}", file.deletions)),
                            )
                        }),
                )
                .on_click(cx.listener(move |app, _, _, cx| {
                    if let Some(page) = app.review_pages.get_mut(&owner) {
                        page.selected = Some(key.clone());
                    }
                    cx.notify();
                })),
            );
        }
        let diff = selected
            .map(|file| {
                tools::render_review_detail(
                    format!("review-diff-{id}-{:?}-{}", file.section, file.path).into(),
                    file.detail(locale),
                    file.patch.clone(),
                    locale,
                )
                .into_any_element()
            })
            .unwrap_or_else(|| {
                div()
                    .flex_1()
                    .size_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_3()
                    .text_color(rgb(colors.muted))
                    .text_size(px(13.))
                    .child(
                        Icon::new(if model.workspace_busy {
                            IconName::LoaderCircle
                        } else {
                            IconName::CircleCheck
                        })
                        .size(px(28.)),
                    )
                    .child(locale.text(if model.workspace_busy {
                        "正在读取变更…"
                    } else if review.is_none() && model.changes_status.is_some() {
                        "无法显示差异，请刷新重试。"
                    } else {
                        "没有可审查的变更"
                    }))
                    .into_any_element()
            });
        div()
            .debug_selector(|| "workspace-review-page".into())
            .flex_1()
            .min_w_0()
            .min_h_0()
            .my_2()
            .mr_2()
            .rounded(px(16.))
            .border_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.surface))
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(52.))
                    .flex_none()
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_3()
                    .border_b_1()
                    .border_color(rgb(colors.border))
                    .bg(rgb(colors.canvas))
                    .child(
                        Button::new("close-workspace-review")
                            .debug_selector(|| "close-workspace-review".into())
                            .ghost()
                            .small()
                            .size(px(28.))
                            .icon(IconName::ArrowLeft)
                            .tooltip(locale.text("返回对话"))
                            .accessibility_label(locale.text("返回对话"))
                            .on_click(cx.listener(move |app, _, window, cx| {
                                app.close_workspace_review(window, cx)
                            })),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(14.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(locale.text("变更审查")),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(12.))
                            .text_color(rgb(colors.muted))
                            .child(format!(
                                "{}  /  {branch}",
                                model
                                    .selected_project
                                    .as_ref()
                                    .map(|project| project.display_name.as_str())
                                    .unwrap_or_default()
                            )),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(11.))
                            .font_family(MONO_FONT)
                            .text_color(rgb(colors.muted))
                            .child(head),
                    )
                    .when(
                        workspace.is_some_and(|workspace| workspace.managed),
                        |element| {
                            element.child(
                                Button::new("review-merge-options")
                                    .debug_selector(|| "review-merge-options".into())
                                    .ghost()
                                    .small()
                                    .label(locale.text("合入…"))
                                    .selected(page.merge_open)
                                    .on_click(cx.listener(move |app, _, _, cx| {
                                        if let Some(page) = app.review_pages.get_mut(&owner) {
                                            page.merge_open = !page.merge_open;
                                        }
                                        cx.notify();
                                    })),
                            )
                        },
                    )
                    .child(
                        Button::new("refresh-workspace-review")
                            .debug_selector(|| "refresh-workspace-review".into())
                            .ghost()
                            .small()
                            .size(px(28.))
                            .icon(IconName::RotateCw)
                            .loading(model.workspace_busy)
                            .tooltip(locale.text("刷新变更"))
                            .accessibility_label(locale.text("刷新变更"))
                            .disabled(model.workspace_busy)
                            .on_click(cx.listener(move |app, _, _, cx| {
                                app.presenter.review_workspace(id);
                                cx.notify();
                            })),
                    ),
            )
            .when_some(review, |element, review| {
                if page.merge_open || workspace.is_some_and(|workspace| workspace.merge.is_some()) {
                    element.child(self.render_review_merge(page, review, cx))
                } else {
                    element
                }
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .flex()
                    .overflow_hidden()
                    .when(!files.is_empty(), |element| element.child(navigation))
                    .child(
                        div()
                            .debug_selector(|| "review-diff-pane".into())
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .flex()
                            .flex_col()
                            .child(diff),
                    ),
            )
            .when_some(model.changes_status.as_ref(), |element, status| {
                element.child(
                    div()
                        .id("review-status")
                        .debug_selector(|| "review-status".into())
                        .flex_none()
                        .max_h(px(96.))
                        .overflow_y_scroll()
                        .px_4()
                        .py_2()
                        .border_t_1()
                        .border_color(rgb(colors.border))
                        .text_size(px(12.))
                        .text_color(rgb(colors.text_secondary))
                        .child(status.render(locale).to_owned()),
                )
            })
            .into_any_element()
    }

    fn render_review_merge(
        &self,
        page: &ReviewPage,
        review: &WorkspaceReview,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let model = self.presenter.model();
        let locale = model.language;
        let colors = palette(cx);
        let id = page.workspace_id;
        let owner = model.conversation.id;
        let workspace = model.workspaces.iter().find(|workspace| workspace.id == id);
        let mut panel = div()
            .id("review-merge-panel")
            .debug_selector(|| "review-merge-panel".into())
            .flex_none()
            .max_h(px(180.))
            .overflow_y_scroll()
            .px_4()
            .py_3()
            .border_b_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.canvas))
            .flex()
            .flex_col()
            .gap_2()
            .text_size(px(12.));
        if let Some(state) = workspace.and_then(|workspace| workspace.merge.as_ref()) {
            let mut actions = div().flex().items_center().gap_2().child(
                div().flex_1().min_w_0().truncate().child(format!(
                    "{} → {}",
                    review.branch.as_deref().unwrap_or("HEAD"),
                    state.target_branch
                )),
            );
            for (abort, label) in [(false, "继续合并"), (true, "中止合并")] {
                let expected = review.clone();
                actions = actions.child(Button::new(label).small().label(locale.text(label))
                    .disabled(model.workspace_busy || (!abort && !review.conflicts.is_empty()))
                    .on_click(cx.listener(move |_, _, window, cx| {
                        let answer = window.prompt(PromptLevel::Critical, locale.text(label),
                            Some(locale.text("继续会提交当前冲突解决内容；中止会恢复到本次合并开始前的状态。")),
                            &[PromptButton::ok(locale.text(label)), PromptButton::cancel(locale.text("取消"))], cx);
                        let review = expected.clone();
                        cx.spawn(async move |app, cx| {
                            if answer.await.ok() == Some(0) {
                                let _ = app.update(cx, |app, cx| {
                                    if app.presenter.model().conversation.id == owner {
                                        app.presenter.finish_workspace_merge(review, abort); cx.notify();
                                    }
                                });
                            }
                        }).detach();
                    })));
            }
            let path = state.target_path.clone();
            panel =
                panel
                    .child(actions)
                    .child(div().text_color(rgb(colors.muted)).child(locale.text(
                        "请在目标目录编辑冲突文件并 git add，再刷新此处继续；也可以中止合并。",
                    )))
                    .when(!review.conflicts.is_empty(), |element| {
                        element.child(div().text_color(rgb(colors.warning)).child(format!(
                            "{}: {}",
                            locale.text("冲突文件"),
                            review.conflicts.join(", ")
                        )))
                    })
                    .child(
                        Button::new("review-merge-directory")
                            .ghost()
                            .small()
                            .icon(IconName::Folder)
                            .label(path.clone())
                            .on_click(move |_, _, cx| cx.reveal_path(Path::new(&path))),
                    );
        } else if let Some(plan) = model
            .merge_plan
            .as_ref()
            .filter(|plan| plan.workspace_id == id)
        {
            let expected = plan.clone();
            panel = panel
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(div().flex_1().min_w_0().truncate().child(format!(
                            "{} → {}",
                            plan.source_branch, plan.state.target_branch
                        )))
                        .child(
                            Button::new("confirm-workspace-merge")
                                .debug_selector(|| "confirm-workspace-merge".into())
                                .primary()
                                .small()
                                .label(locale.text("确认合入本地分支"))
                                .disabled(model.workspace_busy)
                                .on_click(cx.listener(move |app, _, _, cx| {
                                    app.presenter.merge_workspace(expected.clone());
                                    cx.notify();
                                })),
                        ),
                )
                .child(
                    div()
                        .text_color(rgb(colors.muted))
                        .child(plan.state.target_path.clone()),
                );
        } else if workspace.is_some_and(|workspace| workspace.managed) {
            let target = page.target.clone();
            panel = panel
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(locale.text("合入本地目标分支"))
                        .child(div().w(px(200.)).child(Input::new(&page.target).small()))
                        .child(
                            Button::new("preview-workspace-merge")
                                .debug_selector(|| "preview-workspace-merge".into())
                                .small()
                                .label(locale.text("预览合入"))
                                .disabled(model.workspace_busy || !review.dirty_paths.is_empty())
                                .on_click(cx.listener(move |app, _, _, cx| {
                                    let branch = target.read(cx).value().to_string();
                                    if app.presenter.preview_workspace_merge(id, branch)
                                        && let Some(page) = app.review_pages.get_mut(&owner)
                                    {
                                        page.selected = None;
                                    }
                                    cx.notify();
                                })),
                        ),
                )
                .when(!review.dirty_paths.is_empty(), |element| {
                    element.child(
                        div()
                            .text_color(rgb(colors.muted))
                            .child(locale.text("请先提交工作区变更，再预览合入。")),
                    )
                });
        }
        panel.into_any_element()
    }
}
