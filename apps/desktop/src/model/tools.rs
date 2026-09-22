use crate::i18n::Language;
use nexus_domain::{Message, MessageKind, MessageRole};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};
use uuid::Uuid;

pub(crate) enum TimelineItem<'a> {
    Message(&'a Message),
    Tools(Vec<ToolActivity<'a>>),
    Process {
        id: Uuid,
        items: Vec<TimelineItem<'a>>,
    },
}

pub(crate) struct ToolActivity<'a> {
    pub(crate) call: &'a Message,
    pub(crate) result: Option<&'a Message>,
}

pub(crate) fn timeline_items<'a>(
    messages: &'a [Message],
    completed_runs: &HashSet<Uuid>,
) -> Vec<TimelineItem<'a>> {
    let mut timeline = Vec::new();
    for run in messages.chunk_by(|a, b| a.task_id == b.task_id && a.run_id == b.run_id) {
        let items = tool_items(run);
        // Completion is a run boundary, not an individual message boundary.
        // A trailing tool call or user message means there is no final answer yet.
        let final_index = completed_runs
            .contains(&run[0].run_id)
            .then(|| {
                items.iter().rposition(|item| {
                    !matches!(item, TimelineItem::Message(message)
                    if message.kind == MessageKind::Status || message.content.trim().is_empty())
                })
            })
            .flatten()
            .filter(|&index| {
                matches!(&items[index], TimelineItem::Message(message)
                if message.role == MessageRole::Assistant && message.kind == MessageKind::Text)
            });
        let Some(final_index) = final_index else {
            timeline.extend(items);
            continue;
        };
        let mut process = Vec::new();
        for (index, item) in items.into_iter().enumerate() {
            // Keep user steering and errors in place, outside any disclosure.
            let visible = index >= final_index
                || matches!(&item, TimelineItem::Message(message)
                    if message.role == MessageRole::User || message.kind == MessageKind::Error);
            if visible {
                if let Some(first) = process.first() {
                    let id = match first {
                        TimelineItem::Message(message) => message.id,
                        TimelineItem::Tools(batch) => batch[0].call.id,
                        TimelineItem::Process { id, .. } => *id,
                    };
                    timeline.push(TimelineItem::Process {
                        id,
                        items: std::mem::take(&mut process),
                    });
                }
                timeline.push(item);
            } else if !matches!(&item, TimelineItem::Message(message) if message.content.trim().is_empty())
            {
                process.push(item);
            }
        }
    }
    timeline
}

// Pair by run and invocation, never by completion order. Results can arrive
// after an assistant message; they still belong to their original call.
fn tool_items(messages: &[Message]) -> Vec<TimelineItem<'_>> {
    let mut pending = HashMap::new();
    let mut results = HashMap::new();
    let mut paired_results = HashSet::new();
    for message in messages {
        let Some(tool) = &message.tool else { continue };
        let key = (message.run_id, tool.id.as_str());
        match message.kind {
            MessageKind::ToolCall => {
                pending.insert(key, message.id);
            }
            MessageKind::ToolResult => {
                if let Some(call_id) = pending.remove(&key) {
                    results.insert(call_id, message);
                    paired_results.insert(message.id);
                }
            }
            _ => {}
        }
    }
    let mut items = Vec::new();
    for message in messages {
        if paired_results.contains(&message.id) {
            continue;
        }
        if matches!(
            message.kind,
            MessageKind::ToolCall | MessageKind::ToolResult
        ) {
            let activity = ToolActivity {
                call: message,
                result: results
                    .get(&message.id)
                    .copied()
                    .or_else(|| (message.kind == MessageKind::ToolResult).then_some(message)),
            };
            if let Some(TimelineItem::Tools(batch)) = items.last_mut()
                && batch[0].call.run_id == message.run_id
            {
                batch.push(activity);
            } else {
                items.push(TimelineItem::Tools(vec![activity]));
            }
        } else {
            items.push(TimelineItem::Message(message));
        }
    }
    items
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolCategory {
    Command,
    Read,
    Search,
    Edit,
    Create,
    Other,
}

impl ToolCategory {
    pub(crate) fn label(self, locale: Language) -> &'static str {
        locale.text(match self {
            Self::Command => "执行命令",
            Self::Read => "读取文件",
            Self::Search => "搜索",
            Self::Edit => "编辑文件",
            Self::Create => "写入文件",
            Self::Other => "调用工具",
        })
    }
}

#[derive(Clone)]
pub(crate) struct ToolDetail {
    pub(crate) title: String,
    pub(crate) text: String,
    pub(crate) language: String,
    pub(crate) diff: bool,
    pub(crate) image: Option<Arc<ToolImage>>,
}

pub(crate) enum ToolImage {
    Base64 { mime_type: String, data: String },
    Path(PathBuf),
}

impl<'a> ToolActivity<'a> {
    pub(crate) fn name(&self) -> &'a str {
        if self.call.kind == MessageKind::ToolResult {
            "工具输出"
        } else {
            self.call
                .content
                .split_once('\n')
                .map_or(self.call.content.as_str(), |(name, _)| name)
        }
    }

    fn input(&self) -> &'a str {
        if self.call.kind == MessageKind::ToolResult {
            ""
        } else {
            self.call
                .content
                .split_once('\n')
                .map_or("", |(_, input)| input)
        }
    }

    pub(crate) fn category(&self) -> ToolCategory {
        match self
            .name()
            .rsplit(['/', '.'])
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "command" | "bash" | "shell" | "exec_command" => ToolCategory::Command,
            "read" | "read_file" | "view_image" | "imageview" => ToolCategory::Read,
            "grep" | "glob" | "search" | "web search" | "websearch" => ToolCategory::Search,
            "edit" | "multiedit" | "file change" | "apply_patch" => ToolCategory::Edit,
            "write" | "write_file" | "create_file" => ToolCategory::Create,
            _ => ToolCategory::Other,
        }
    }

    pub(crate) fn is_error(&self) -> bool {
        self.result.is_some_and(|result| {
            result.tool.as_ref().map_or_else(
                || result.content.starts_with("工具执行失败\n"),
                |tool| tool.is_error,
            )
        })
    }

    pub(crate) fn is_running(&self, active_run: Option<Uuid>) -> bool {
        self.result.is_none() && self.call.tool.is_some() && active_run == Some(self.call.run_id)
    }

    pub(crate) fn preview(&self, locale: Language) -> String {
        let input: Value = serde_json::from_str(self.input()).unwrap_or(Value::Null);
        let media_output = self.input().is_empty()
            && self.result.is_some_and(|result| {
                serde_json::from_str::<Value>(&result.content)
                    .is_ok_and(|value| output_blocks(&value).any(is_media_block))
            });
        let value = string_field(
            &input,
            &["command", "cmd", "file_path", "path", "pattern", "query"],
        )
        .or_else(|| input.pointer("/changes/0/path").and_then(Value::as_str))
        .unwrap_or_else(|| {
            if self.input().is_empty() {
                if media_output {
                    locale.text("媒体预览")
                } else {
                    self.result.map_or("", |result| result.content.as_str())
                }
            } else {
                self.input()
            }
        });
        let preview = value
            .lines()
            .next()
            .unwrap_or_default()
            .chars()
            .take(160)
            .collect::<String>();
        if self.category() == ToolCategory::Other {
            let name = if self.call.kind == MessageKind::ToolResult {
                locale.text("工具输出")
            } else {
                self.name()
            };
            format!("{name} · {preview}")
        } else {
            format!("{} · {preview}", self.category().label(locale))
        }
    }

    pub(crate) fn details(&self, locale: Language, directory: Option<&Path>) -> Vec<ToolDetail> {
        let input: Value = serde_json::from_str(self.input()).unwrap_or(Value::Null);
        let output = self.result.map(|result| {
            if self.is_error() {
                result
                    .content
                    .strip_prefix("工具执行失败\n")
                    .unwrap_or(&result.content)
            } else {
                result.content.as_str()
            }
        });
        let result: Value = output
            .and_then(|text| serde_json::from_str(text).ok())
            .unwrap_or(Value::Null);
        let path = string_field(&input, &["file_path", "path"]).unwrap_or(locale.text("文件"));
        let mut details = Vec::new();
        match self.category() {
            ToolCategory::Command => details.push(detail(
                locale.text("命令"),
                "sh",
                string_field(&input, &["command", "cmd"]).unwrap_or(self.input()),
                false,
            )),
            ToolCategory::Create => {
                if let Some(code) = input.get("content").and_then(Value::as_str) {
                    details.push(detail(path, language_for_path(path), code, false));
                }
            }
            ToolCategory::Edit => {
                if let Some(changes) = input.get("changes").and_then(Value::as_array) {
                    for change in changes {
                        let path = string_field(change, &["path"]).unwrap_or(locale.text("文件"));
                        if let Some(diff) = string_field(change, &["diff", "patch"]) {
                            details.push(detail(path, language_for_path(path), diff, true));
                        } else if let Some(code) = string_field(change, &["content"]) {
                            details.push(detail(path, language_for_path(path), code, false));
                        } else {
                            details.push(detail(
                                path,
                                "text",
                                locale.text("此工具事件未提供文件内容或 diff。"),
                                false,
                            ));
                        }
                    }
                } else if let Some(diff) = result
                    .pointer("/details/diff")
                    .and_then(Value::as_str)
                    .or_else(|| string_field(&input, &["patch", "diff"]))
                {
                    details.push(detail(path, language_for_path(path), diff, true));
                } else if let Some(edits) = input.get("edits").and_then(Value::as_array) {
                    for edit in edits {
                        append_edit(&mut details, path, edit, locale);
                    }
                } else {
                    append_edit(&mut details, path, &input, locale);
                }
            }
            _ => {}
        }
        if details.is_empty() && !self.input().is_empty() {
            details.push(detail(
                locale.text("输入"),
                if input.is_null() { "text" } else { "json" },
                &pretty_payload(self.input()),
                false,
            ));
        }
        if let Some(output) = output {
            if let Some(blocks) = media_output_details(&result, locale) {
                details.extend(blocks);
            } else if (self.category() == ToolCategory::Read || result["type"] == "imageView")
                && !self.is_error()
                && let Some(path) = string_field(&input, &["file_path", "path"])
                    .or_else(|| string_field(&result, &["path"]))
                && (is_image_path(path) || result["type"] == "imageView")
            {
                let path = Path::new(path);
                let resolved = if path.is_absolute() {
                    Some(path.to_path_buf())
                } else {
                    directory.map(|directory| directory.join(path))
                };
                details.push(image_detail(resolved.map(ToolImage::Path), locale));
            } else {
                details.push(detail(
                    locale.text("输出"),
                    "text",
                    &pretty_payload(output),
                    false,
                ));
            }
        }
        details
    }
}

fn string_field<'a>(value: &'a Value, fields: &[&str]) -> Option<&'a str> {
    fields
        .iter()
        .find_map(|field| value.get(*field).and_then(Value::as_str))
}

fn language_for_path(path: &str) -> &str {
    Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("text")
}

fn detail(title: &str, language: &str, text: &str, diff: bool) -> ToolDetail {
    ToolDetail {
        title: title.into(),
        language: language.into(),
        text: text.into(),
        diff,
        image: None,
    }
}

fn is_image_path(path: &str) -> bool {
    matches!(
        language_for_path(path).to_ascii_lowercase().as_str(),
        "png"
            | "jpg"
            | "jpeg"
            | "gif"
            | "webp"
            | "svg"
            | "bmp"
            | "tif"
            | "tiff"
            | "ico"
            | "pnm"
            | "ppm"
            | "pgm"
            | "pbm"
    )
}

fn image_detail(image: Option<ToolImage>, locale: Language) -> ToolDetail {
    let mut detail = detail(locale.text("图片预览"), "text", "", false);
    detail.image = image.map(Arc::new);
    if detail.image.is_none() {
        detail.text = locale.text("无法预览此图片。").into();
    }
    detail
}

fn output_blocks(value: &Value) -> impl Iterator<Item = &Value> + Clone {
    let blocks = value
        .as_array()
        .or_else(|| value.get("content").and_then(Value::as_array))
        .map(Vec::as_slice)
        .unwrap_or_else(|| std::slice::from_ref(value));
    // ACP wraps a standard content block in a tool-call content item.
    blocks.iter().map(|block| {
        if block["type"] == "content" {
            &block["content"]
        } else {
            block
        }
    })
}

fn is_media_block(block: &Value) -> bool {
    matches!(
        block["type"].as_str(),
        Some("image" | "audio" | "video" | "document")
    )
}

fn media_output_details(value: &Value, locale: Language) -> Option<Vec<ToolDetail>> {
    let blocks = output_blocks(value);
    if !blocks.clone().any(is_media_block) {
        return None;
    }
    Some(
        blocks
            .map(|block| match block["type"].as_str() {
                Some("image") => {
                    let source = block.get("source").unwrap_or(block);
                    let image = string_field(source, &["media_type", "mimeType"])
                        .zip(string_field(source, &["data"]))
                        .map(|(mime_type, data)| ToolImage::Base64 {
                            mime_type: mime_type.into(),
                            data: data.into(),
                        });
                    image_detail(image, locale)
                }
                Some("audio" | "video" | "document") => detail(
                    locale.text("输出"),
                    "text",
                    locale.text("暂不支持预览此媒体。"),
                    false,
                ),
                Some("text") => detail(
                    locale.text("输出"),
                    "text",
                    block["text"].as_str().unwrap_or_default(),
                    false,
                ),
                _ => detail(
                    locale.text("输出"),
                    "text",
                    &pretty_payload(&block.to_string()),
                    false,
                ),
            })
            .collect(),
    )
}

fn append_edit(details: &mut Vec<ToolDetail>, path: &str, input: &Value, locale: Language) {
    if let (Some(before), Some(after)) = (
        string_field(input, &["old_string", "oldText"]),
        string_field(input, &["new_string", "newText"]),
    ) {
        details.push(detail(
            &locale.format("{path} · 修改片段", &[("path", path.to_owned())]),
            language_for_path(path),
            &replacement_diff(before, after),
            true,
        ));
    }
}

// These are replacement snippets supplied by the tool, not full-file snapshots.
// Keep line numbers relative to the snippet and preserve final-newline changes.
fn replacement_diff(before: &str, after: &str) -> String {
    let old_count = before.lines().count();
    let new_count = after.lines().count();
    let mut diff = format!(
        "@@ -{},{} +{},{} @@\n",
        usize::from(old_count > 0),
        old_count,
        usize::from(new_count > 0),
        new_count
    );
    for (prefix, text) in [('-', before), ('+', after)] {
        for line in text.split_inclusive('\n') {
            diff.push(prefix);
            diff.push_str(line);
            if !line.ends_with('\n') {
                diff.push_str("\n\\ No newline at end of file\n");
            }
        }
    }
    diff
}

fn pretty_payload(raw: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return raw.into();
    };
    let blocks = value
        .as_array()
        .or_else(|| value.get("content").and_then(Value::as_array));
    if let Some(blocks) = blocks
        && blocks
            .iter()
            .all(|block| block.get("type").and_then(Value::as_str) == Some("text"))
    {
        return blocks
            .iter()
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n");
    }
    value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| serde_json::to_string_pretty(&value).unwrap_or_else(|_| raw.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use nexus_domain::{MessageRole, ToolMetadata};

    fn message(run_id: Uuid, kind: MessageKind, id: Option<&str>, content: &str) -> Message {
        Message {
            attachments: Vec::new(),
            id: Uuid::new_v4(),
            task_id: Uuid::nil(),
            run_id,
            sequence: 0,
            role: if kind == MessageKind::Text {
                MessageRole::Assistant
            } else {
                MessageRole::Tool
            },
            kind,
            content: content.into(),
            created_at: Utc::now(),
            tool: id.map(|id| ToolMetadata {
                id: id.into(),
                is_error: false,
            }),
        }
    }

    #[test]
    fn batches_keep_text_and_run_boundaries_and_preserve_unpaired_legacy_output() {
        let run = Uuid::new_v4();
        let other = Uuid::new_v4();
        let messages = vec![
            message(run, MessageKind::ToolCall, Some("a"), "Command\necho one"),
            message(run, MessageKind::Text, None, "Assistant explanation"),
            message(run, MessageKind::ToolResult, Some("a"), "one"),
            message(run, MessageKind::ToolCall, Some("b"), "Read\n{}"),
            message(other, MessageKind::ToolCall, Some("a"), "Command\necho two"),
            message(other, MessageKind::ToolResult, Some("a"), "two"),
            message(
                other,
                MessageKind::ToolResult,
                None,
                "Legacy output without an id",
            ),
        ];
        let items = timeline_items(&messages, &HashSet::new());
        assert_eq!(items.len(), 4);
        let TimelineItem::Tools(first) = &items[0] else {
            panic!("tools")
        };
        assert_eq!(first[0].result.unwrap().content, "one");
        assert!(
            matches!(&items[1], TimelineItem::Message(message) if message.content == "Assistant explanation")
        );
        let TimelineItem::Tools(pending) = &items[2] else {
            panic!("tools")
        };
        assert!(pending[0].is_running(Some(run)));
        assert!(!pending[0].is_running(None));
        let TimelineItem::Tools(last) = &items[3] else {
            panic!("tools")
        };
        assert_eq!(last.len(), 2);
        assert_eq!(last[0].result.unwrap().content, "two");
        assert!(
            last[1].details(Language::Chinese, None)[0]
                .text
                .contains("Legacy output")
        );
    }

    #[test]
    fn completed_turns_group_process_and_keep_each_request_and_answer_visible() {
        let run = Uuid::new_v4();
        let next = Uuid::new_v4();
        let messages = vec![
            Message {
                role: MessageRole::User,
                ..message(run, MessageKind::Text, None, "request")
            },
            message(run, MessageKind::Text, None, "plan"),
            message(run, MessageKind::ToolCall, Some("a"), "Read\n{}"),
            message(run, MessageKind::Text, None, "progress"),
            message(run, MessageKind::ToolResult, Some("a"), "contents"),
            message(run, MessageKind::Text, None, "answer"),
            message(run, MessageKind::Status, None, "finished"),
            Message {
                role: MessageRole::User,
                ..message(next, MessageKind::Text, None, "follow-up")
            },
            message(next, MessageKind::Text, None, "follow-up progress"),
            message(next, MessageKind::Text, None, "follow-up answer"),
        ];
        let items = timeline_items(&messages, &HashSet::from([run, next]));
        assert_eq!(items.len(), 7);
        for (index, message_index) in [(0, 0), (2, 5), (3, 6), (4, 7), (6, 9)] {
            assert!(
                matches!(&items[index], TimelineItem::Message(m) if m.id == messages[message_index].id)
            );
        }
        let TimelineItem::Process { id, items: process } = &items[1] else {
            panic!("process")
        };
        assert_eq!(*id, messages[1].id);
        assert_eq!(process.len(), 3);
        assert!(matches!(&process[0], TimelineItem::Message(m) if m.content == "plan"));
        let TimelineItem::Tools(batch) = &process[1] else {
            panic!("tools")
        };
        assert_eq!(batch[0].result.unwrap().content, "contents");
        assert!(matches!(&process[2], TimelineItem::Message(m) if m.content == "progress"));
        assert!(
            matches!(&items[5], TimelineItem::Process { id, items } if *id == messages[8].id && items.len() == 1)
        );

        let items = timeline_items(&messages, &HashSet::from([run]));
        assert!(matches!(&items[5], TimelineItem::Message(m) if m.content == "follow-up progress"));
    }

    #[test]
    fn unfinished_runs_missing_answers_and_direct_answers_have_no_process_disclosure() {
        let run = Uuid::new_v4();
        let request = Message {
            role: MessageRole::User,
            ..message(run, MessageKind::Text, None, "request")
        };
        let progress = message(run, MessageKind::Text, None, "progress");
        let answer = message(run, MessageKind::Text, None, "answer");
        let completed = HashSet::from([run]);
        for (messages, runs) in [
            (
                vec![request.clone(), progress.clone(), answer.clone()],
                HashSet::new(),
            ),
            (vec![request.clone(), answer.clone()], completed.clone()),
            (
                vec![
                    request.clone(),
                    message(run, MessageKind::Text, None, " \n"),
                    answer,
                ],
                completed.clone(),
            ),
            (
                vec![
                    request.clone(),
                    progress.clone(),
                    message(run, MessageKind::ToolCall, Some("a"), "Read\n{}"),
                ],
                completed.clone(),
            ),
            (
                vec![
                    request.clone(),
                    progress.clone(),
                    message(run, MessageKind::Error, None, "failed"),
                ],
                completed.clone(),
            ),
            (
                vec![request, message(run, MessageKind::Text, None, "  \n")],
                completed,
            ),
        ] {
            assert!(
                timeline_items(&messages, &runs)
                    .iter()
                    .all(|item| !matches!(item, TimelineItem::Process { .. }))
            );
        }
    }

    #[test]
    fn process_disclosures_preserve_steering_messages_and_errors_in_place() {
        let run = Uuid::new_v4();
        let messages = vec![
            message(run, MessageKind::Text, None, "initial progress"),
            Message {
                role: MessageRole::User,
                ..message(run, MessageKind::Text, None, "steering")
            },
            message(run, MessageKind::Text, None, "updated progress"),
            message(run, MessageKind::Error, None, "diagnostic"),
            message(run, MessageKind::Text, None, "answer"),
        ];
        let items = timeline_items(&messages, &HashSet::from([run]));
        assert_eq!(items.len(), 5);
        for index in [1, 3, 4] {
            assert!(
                matches!(&items[index], TimelineItem::Message(m) if m.id == messages[index].id)
            );
        }
        assert!(matches!(&items[0], TimelineItem::Process { id, .. } if *id == messages[0].id));
        assert!(matches!(&items[2], TimelineItem::Process { id, .. } if *id == messages[2].id));
    }

    #[test]
    fn details_render_full_commands_created_code_and_edit_diffs() {
        let run = Uuid::new_v4();
        let code = "fn main() { println!(\"你好\"); }\n".repeat(40);
        for (name, input, diff) in [
            (
                "Write",
                serde_json::json!({"file_path": "main.rs", "content": code}),
                false,
            ),
            (
                "Edit",
                serde_json::json!({"file_path": "main.rs", "old_string": "old\n", "new_string": code}),
                true,
            ),
            (
                "MultiEdit",
                serde_json::json!({"file_path": "main.rs", "edits": [{"old_string": "old\n", "new_string": code}]}),
                true,
            ),
        ] {
            let call = message(
                run,
                MessageKind::ToolCall,
                Some("t"),
                &format!("{name}\n{input}"),
            );
            let activity = ToolActivity {
                call: &call,
                result: None,
            };
            let details = activity.details(Language::Chinese, None);
            assert_eq!(details.len(), 1);
            assert_eq!(details[0].language, "rs");
            assert_eq!(details[0].diff, diff);
            assert!(details[0].text.contains("你好"));
            assert!(details[0].text.len() > 400);
            if diff {
                assert!(details[0].text.contains("-old\n+fn main()"));
            } else {
                assert_eq!(details[0].text, code);
            }
            let english = activity.details(Language::English, None);
            assert_eq!(english[0].text, details[0].text);
            assert_eq!(english[0].language, "rs");
            assert_eq!(
                english[0].title,
                if diff {
                    "main.rs · Changed snippet"
                } else {
                    "main.rs"
                }
            );
            assert!(activity.preview(Language::English).ends_with("main.rs"));
            assert!(!activity.preview(Language::English).contains("文件"));
        }
        let call = message(
            run,
            MessageKind::ToolCall,
            Some("t"),
            "Bash\n{\"command\":\"cargo test\"}",
        );
        let output = message(
            run,
            MessageKind::ToolResult,
            Some("t"),
            &serde_json::json!([
                {"type": "text", "text": code}
            ])
            .to_string(),
        );
        let details = ToolActivity {
            call: &call,
            result: Some(&output),
        }
        .details(Language::Chinese, None);
        assert_eq!(details[0].text, "cargo test");
        assert_eq!(details[1].text, code);
        let english = ToolActivity {
            call: &call,
            result: Some(&output),
        }
        .details(Language::English, None);
        assert_eq!(english[0].title, "Command");
        assert_eq!(english[1].title, "Output");
        assert_eq!(english[1].text, code);
        let call = message(
            run,
            MessageKind::ToolCall,
            Some("t"),
            "edit\n{\"path\":\"main.rs\"}",
        );
        let output = message(
            run,
            MessageKind::ToolResult,
            Some("t"),
            r#"{"content":[{"type":"text","text":"done"}],"details":{"diff":"-old\n+new"}}"#,
        );
        let details = ToolActivity {
            call: &call,
            result: Some(&output),
        }
        .details(Language::Chinese, None);
        assert_eq!(details[0].text, "-old\n+new");
        assert!(details[0].diff);
        assert_eq!(details[1].text, "done");
    }

    #[test]
    fn image_results_preserve_text_order_and_preview_each_image_without_dumping_data() {
        use serde_json::json;
        let run = Uuid::new_v4();
        let call = message(
            run,
            MessageKind::ToolCall,
            Some("read"),
            "Read\n{\"file_path\":\"missing.png\"}",
        );
        let image = json!({"type": "image", "mimeType": "image/png", "data": "aW1hZ2U="});
        let claude = json!({"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "aW1hZ2U="}});
        let blocks = json!([
            {"type": "text", "text": "before"}, image,
            {"type": "text", "text": "after"}, claude
        ]);
        let acp: Vec<_> = blocks
            .as_array()
            .unwrap()
            .iter()
            .map(|block| json!({"type": "content", "content": block}))
            .collect();
        for payload in [blocks.clone(), json!({"content": blocks}), json!(acp)] {
            let result = message(
                run,
                MessageKind::ToolResult,
                Some("read"),
                &payload.to_string(),
            );
            let details = ToolActivity {
                call: &call,
                result: Some(&result),
            }
            .details(Language::English, None);
            assert_eq!(details.len(), 5);
            assert_eq!(details[1].text, "before");
            assert_eq!(details[3].text, "after");
            for index in [2, 4] {
                assert_eq!(details[index].title, "Image preview");
                assert!(details[index].text.is_empty());
                assert!(
                    matches!(details[index].image.as_deref(), Some(ToolImage::Base64 { mime_type, data })
                    if mime_type == "image/png" && data == "aW1hZ2U=")
                );
            }
            assert!(
                details
                    .iter()
                    .all(|detail| !detail.text.contains("aW1hZ2U="))
            );
            assert_eq!(
                result.content,
                payload.to_string(),
                "stored payload must stay intact"
            );
        }
        let result = message(run, MessageKind::ToolResult, None, &image.to_string());
        let details = ToolActivity {
            call: &result,
            result: Some(&result),
        }
        .details(Language::English, None);
        assert_eq!(details.len(), 1);
        assert!(
            details[0].image.is_some(),
            "unpaired legacy results also show previews"
        );
        assert_eq!(
            ToolActivity {
                call: &result,
                result: Some(&result)
            }
            .preview(Language::English),
            "Tool output · Media preview"
        );
    }

    #[test]
    fn image_paths_use_the_task_directory_and_failed_reads_keep_the_diagnostic() {
        let run = Uuid::new_v4();
        let directory = tempfile::tempdir().unwrap();
        let mut result = message(
            run,
            MessageKind::ToolResult,
            Some("read"),
            "Read image file",
        );
        for name in ["Read", "read_file", "functions.view_image", "imageView"] {
            let call = message(
                run,
                MessageKind::ToolCall,
                Some("read"),
                &format!("{name}\n{{\"path\":\"assets/IMAGE.PNG\"}}"),
            );
            let activity = ToolActivity {
                call: &call,
                result: Some(&result),
            };
            let details = activity.details(Language::English, Some(directory.path()));
            assert!(
                matches!(details[1].image.as_deref(), Some(ToolImage::Path(path))
                if path == &directory.path().join("assets/IMAGE.PNG"))
            );
            assert!(details[1].text.is_empty());
            assert!(
                activity.details(Language::English, None)[1].image.is_none(),
                "never resolve relative paths against the desktop process directory"
            );
            assert_eq!(
                ToolActivity {
                    call: &call,
                    result: None
                }
                .details(Language::English, Some(directory.path()))
                .len(),
                1
            );
        }
        let path = directory.path().join("image-without-extension");
        let native = message(
            run,
            MessageKind::ToolResult,
            None,
            &serde_json::json!({"type": "imageView", "path": path, "status": "completed"})
                .to_string(),
        );
        let details = ToolActivity {
            call: &native,
            result: Some(&native),
        }
        .details(Language::English, None);
        assert_eq!(details.len(), 1);
        assert!(
            matches!(details[0].image.as_deref(), Some(ToolImage::Path(resolved)) if resolved == &path)
        );
        let call = message(
            run,
            MessageKind::ToolCall,
            Some("read"),
            "Read\n{\"file_path\":\"image.png\"}",
        );
        result.content = "permission denied".into();
        result.tool.as_mut().unwrap().is_error = true;
        let details = ToolActivity {
            call: &call,
            result: Some(&result),
        }
        .details(Language::English, Some(directory.path()));
        assert!(details[1].image.is_none());
        assert_eq!(details[1].text, "permission denied");
        let call = message(
            run,
            MessageKind::ToolCall,
            Some("read"),
            "read_file\n{\"path\":\"main.rs\"}",
        );
        result.content = "fn main() {}".into();
        result.tool.as_mut().unwrap().is_error = false;
        let details = ToolActivity {
            call: &call,
            result: Some(&result),
        }
        .details(Language::English, Some(directory.path()));
        assert!(details[1].image.is_none());
        assert_eq!(details[1].text, "fn main() {}");
    }

    #[test]
    fn malformed_images_and_unsupported_media_do_not_dump_binary_payloads() {
        for payload in [
            serde_json::json!({"type": "image", "data": "opaque binary"}),
            serde_json::json!({"type": "audio", "data": "opaque binary", "mimeType": "audio/wav"}),
        ] {
            let result = message(
                Uuid::new_v4(),
                MessageKind::ToolResult,
                None,
                &payload.to_string(),
            );
            let details = ToolActivity {
                call: &result,
                result: Some(&result),
            }
            .details(Language::English, None);
            assert_eq!(details.len(), 1);
            assert!(details[0].image.is_none());
            assert!(!details[0].text.is_empty());
            assert!(!details[0].text.contains("opaque binary"));
        }
    }

    #[test]
    fn file_events_without_diff_report_unavailable_details() {
        let call = message(
            Uuid::new_v4(),
            MessageKind::ToolCall,
            Some("t"),
            "File Change\n{\"changes\":[{\"path\":\"main.rs\",\"kind\":\"update\"}]}",
        );
        let details = ToolActivity {
            call: &call,
            result: None,
        }
        .details(Language::Chinese, None);
        assert_eq!(details[0].title, "main.rs");
        assert!(details[0].text.contains("未提供"));
        assert!(!details[0].diff);
        assert_eq!(
            replacement_diff("old", "old\n"),
            "@@ -1,1 +1,1 @@\n-old\n\\ No newline at end of file\n+old\n"
        );
    }
}
