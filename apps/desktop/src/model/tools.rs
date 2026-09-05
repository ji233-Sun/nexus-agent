use nexus_domain::{Message, MessageKind};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};
use uuid::Uuid;

pub(crate) enum TimelineItem<'a> {
    Message(&'a Message),
    Tools(Vec<ToolActivity<'a>>),
}

pub(crate) struct ToolActivity<'a> {
    pub(crate) call: &'a Message,
    pub(crate) result: Option<&'a Message>,
}

// Pair by run and invocation, never by completion order. Results can arrive
// after an assistant message; they still belong to their original call.
pub(crate) fn timeline_items(messages: &[Message]) -> Vec<TimelineItem<'_>> {
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
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Command => "执行命令",
            Self::Read => "读取文件",
            Self::Search => "搜索",
            Self::Edit => "编辑文件",
            Self::Create => "写入文件",
            Self::Other => "调用工具",
        }
    }
}

pub(crate) struct ToolDetail {
    pub(crate) title: String,
    pub(crate) text: String,
    pub(crate) language: String,
    pub(crate) diff: bool,
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
            "read" | "read_file" => ToolCategory::Read,
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

    pub(crate) fn preview(&self) -> String {
        let input: Value = serde_json::from_str(self.input()).unwrap_or(Value::Null);
        let value = string_field(
            &input,
            &["command", "cmd", "file_path", "path", "pattern", "query"],
        )
        .or_else(|| input.pointer("/changes/0/path").and_then(Value::as_str))
        .unwrap_or_else(|| {
            if self.input().is_empty() {
                self.result.map_or("", |result| result.content.as_str())
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
            format!("{} · {preview}", self.name())
        } else {
            format!("{} · {preview}", self.category().label())
        }
    }

    pub(crate) fn details(&self) -> Vec<ToolDetail> {
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
        let path = string_field(&input, &["file_path", "path"]).unwrap_or("文件");
        let mut details = Vec::new();
        match self.category() {
            ToolCategory::Command => details.push(detail(
                "命令",
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
                        let path = string_field(change, &["path"]).unwrap_or("文件");
                        if let Some(diff) = string_field(change, &["diff", "patch"]) {
                            details.push(detail(path, language_for_path(path), diff, true));
                        } else if let Some(code) = string_field(change, &["content"]) {
                            details.push(detail(path, language_for_path(path), code, false));
                        } else {
                            details.push(detail(
                                path,
                                "text",
                                "此工具事件未提供文件内容或 diff。",
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
                        append_edit(&mut details, path, edit);
                    }
                } else {
                    append_edit(&mut details, path, &input);
                }
            }
            _ => {}
        }
        if details.is_empty() && !self.input().is_empty() {
            details.push(detail(
                "输入",
                if input.is_null() { "text" } else { "json" },
                &pretty_payload(self.input()),
                false,
            ));
        }
        if let Some(output) = output {
            details.push(detail("输出", "text", &pretty_payload(output), false));
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
    }
}

fn append_edit(details: &mut Vec<ToolDetail>, path: &str, input: &Value) {
    if let (Some(before), Some(after)) = (
        string_field(input, &["old_string", "oldText"]),
        string_field(input, &["new_string", "newText"]),
    ) {
        details.push(detail(
            &format!("{path} · 修改片段"),
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
            id: Uuid::new_v4(),
            task_id: Uuid::nil(),
            run_id,
            sequence: 0,
            role: MessageRole::Tool,
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
        let items = timeline_items(&messages);
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
        assert!(last[1].details()[0].text.contains("Legacy output"));
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
            let details = activity.details();
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
        .details();
        assert_eq!(details[0].text, "cargo test");
        assert_eq!(details[1].text, code);
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
        .details();
        assert_eq!(details[0].text, "-old\n+new");
        assert!(details[0].diff);
        assert_eq!(details[1].text, "done");
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
        .details();
        assert_eq!(details[0].title, "main.rs");
        assert!(details[0].text.contains("未提供"));
        assert!(!details[0].diff);
        assert_eq!(
            replacement_diff("old", "old\n"),
            "@@ -1,1 +1,1 @@\n-old\n\\ No newline at end of file\n+old\n"
        );
    }
}
