use std::{
    collections::HashSet,
    io::{BufRead as _, BufReader, Write as _},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command as ProcessCommand, Stdio},
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::Duration,
};

use anyhow::{Context as _, Result, anyhow};
use nexus_domain::{MessageKind, MessageRole};
use serde_json::{Map, Value, json};

const PAGE_LIMIT: usize = 100;
const MAX_THREADS_PER_STATE: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadSummary {
    pub id: String,
    pub title: String,
    pub cwd: String,
    pub source: String,
    pub updated_at: i64,
    pub archived: bool,
}

impl ThreadSummary {
    pub fn detail(&self) -> String {
        let source = match self.source.as_str() {
            "vscode" => "Desktop",
            "exec" | "cli" => "CLI",
            _ => "Codex",
        };
        let project = Path::new(&self.cwd)
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("未知目录");
        if self.archived {
            format!("{source} · {project} · 已归档")
        } else {
            format!("{source} · {project}")
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryMessage {
    pub role: MessageRole,
    pub kind: MessageKind,
    pub content: String,
}

pub enum Event {
    ThreadsLoaded(Result<Vec<ThreadSummary>, String>),
    ThreadLoaded {
        thread_id: String,
        result: Result<Vec<HistoryMessage>, String>,
    },
}

enum Request {
    List,
    Read(String),
}

pub struct Client {
    requests: Sender<Request>,
    events: Receiver<Event>,
}

impl Client {
    pub fn spawn(executable: PathBuf) -> Self {
        Self::spawn_with_fallbacks(executable, desktop_codex_candidates())
    }

    fn spawn_with_fallbacks(executable: PathBuf, fallback_executables: Vec<PathBuf>) -> Self {
        let (request_tx, request_rx) = mpsc::channel();
        let (event_tx, events) = mpsc::channel();
        thread::spawn(move || run_worker(&executable, &fallback_executables, request_rx, event_tx));
        Self {
            requests: request_tx,
            events,
        }
    }

    pub fn refresh(&self) -> bool {
        self.requests.send(Request::List).is_ok()
    }

    pub fn read_thread(&self, thread_id: String) -> bool {
        self.requests.send(Request::Read(thread_id)).is_ok()
    }

    pub fn drain_events(&self) -> Vec<Event> {
        let mut events = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            events.push(event);
        }
        events
    }
}

fn run_worker(
    executable: &Path,
    fallback_executables: &[PathBuf],
    requests: Receiver<Request>,
    events: Sender<Event>,
) {
    let mut server = match spawn_app_server(executable, fallback_executables) {
        Ok(server) => server,
        Err(error) => {
            let _ = events.send(Event::ThreadsLoaded(Err(format!("{error:#}"))));
            return;
        }
    };

    for request in requests {
        match request {
            Request::List => {
                let result = list_threads(&mut server).map_err(|error| format!("{error:#}"));
                let _ = events.send(Event::ThreadsLoaded(result));
            }
            Request::Read(thread_id) => {
                let result =
                    read_thread_with_fallback(&mut server, fallback_executables, &thread_id)
                        .map_err(|error| format!("{error:#}"));
                let _ = events.send(Event::ThreadLoaded { thread_id, result });
            }
        }
    }
}

fn spawn_app_server(executable: &Path, fallback_executables: &[PathBuf]) -> Result<AppServer> {
    let primary_error = match AppServer::spawn(executable) {
        Ok(server) => return Ok(server),
        Err(error) => error,
    };
    let mut fallback_errors = Vec::new();
    for fallback_executable in fallback_executables {
        match AppServer::spawn(fallback_executable) {
            Ok(server) => return Ok(server),
            Err(error) => {
                fallback_errors.push(format!("{}：{error:#}", fallback_executable.display()))
            }
        }
    }
    if fallback_errors.is_empty() {
        Err(primary_error)
    } else {
        Err(anyhow!(
            "{primary_error:#}；Codex Desktop 启动失败：{}",
            fallback_errors.join("；")
        ))
    }
}

struct AppServer {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl AppServer {
    fn spawn(executable: &Path) -> Result<Self> {
        let mut child = ProcessCommand::new(executable)
            .args(["app-server", "--listen", "stdio://"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("启动 Codex app-server：{}", executable.display()))?;
        let stdin = child.stdin.take().context("获取 Codex app-server stdin")?;
        let stdout = child
            .stdout
            .take()
            .context("获取 Codex app-server stdout")?;
        let mut server = Self {
            child,
            stdin: Some(stdin),
            stdout: BufReader::new(stdout),
            next_id: 0,
        };
        server.request(
            "initialize",
            json!({
                "clientInfo": {
                    "name": "nexus-agent",
                    "title": "Nexus Agent",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
        )?;
        server.notify("initialized", json!({}))?;
        Ok(server)
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        self.write_frame(&json!({ "method": method, "params": params }))
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        self.write_frame(&json!({ "id": id, "method": method, "params": params }))?;

        loop {
            let mut line = String::new();
            let read = self
                .stdout
                .read_line(&mut line)
                .context("读取 Codex app-server 响应")?;
            if read == 0 {
                return Err(anyhow!("Codex app-server 在返回 {method} 前退出"));
            }
            let frame: Value = serde_json::from_str(line.trim())
                .with_context(|| format!("解析 Codex app-server 响应：{line}"))?;
            if frame.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(message) = frame.pointer("/error/message").and_then(Value::as_str) {
                return Err(anyhow!("Codex app-server：{message}"));
            }
            return frame
                .get("result")
                .cloned()
                .ok_or_else(|| anyhow!("Codex app-server 响应缺少 result"));
        }
    }

    fn write_frame(&mut self, frame: &Value) -> Result<()> {
        let stdin = self.stdin.as_mut().context("Codex app-server 已关闭")?;
        serde_json::to_writer(&mut *stdin, frame).context("编码 Codex app-server 请求")?;
        stdin
            .write_all(b"\n")
            .context("写入 Codex app-server 请求")?;
        stdin.flush().context("刷新 Codex app-server 请求")?;
        Ok(())
    }
}

impl Drop for AppServer {
    fn drop(&mut self) {
        self.stdin.take();
        for _ in 0..10 {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn list_threads(server: &mut AppServer) -> Result<Vec<ThreadSummary>> {
    let mut threads = list_threads_by_state(server, false)?;
    threads.extend(list_threads_by_state(server, true)?);
    let mut seen = HashSet::new();
    threads.retain(|thread| seen.insert(thread.id.clone()));
    threads.sort_by_key(|thread| std::cmp::Reverse(thread.updated_at));
    Ok(threads)
}

fn list_threads_by_state(server: &mut AppServer, archived: bool) -> Result<Vec<ThreadSummary>> {
    let mut threads = Vec::new();
    let mut cursor: Option<String> = None;
    while threads.len() < MAX_THREADS_PER_STATE {
        let mut params = Map::new();
        params.insert("archived".into(), Value::Bool(archived));
        params.insert("limit".into(), Value::from(PAGE_LIMIT as u64));
        if let Some(cursor) = &cursor {
            params.insert("cursor".into(), Value::String(cursor.clone()));
        }
        let result = server.request("thread/list", Value::Object(params))?;
        let (mut page, next_cursor) = parse_thread_page(&result, archived)?;
        let page_was_empty = page.is_empty();
        threads.append(&mut page);
        if page_was_empty || next_cursor.is_none() || next_cursor == cursor {
            break;
        }
        cursor = next_cursor;
    }
    threads.truncate(MAX_THREADS_PER_STATE);
    Ok(threads)
}

fn parse_thread_page(
    result: &Value,
    archived: bool,
) -> Result<(Vec<ThreadSummary>, Option<String>)> {
    let data = result
        .get("data")
        .and_then(Value::as_array)
        .context("Codex thread/list 响应缺少 data")?;
    let threads = data
        .iter()
        .filter_map(|thread| {
            let id = thread.get("id")?.as_str()?.to_owned();
            let preview = thread
                .get("preview")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let name = thread
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.trim().is_empty())
                .unwrap_or(preview);
            Some(ThreadSummary {
                id,
                title: compact_title(name),
                cwd: thread
                    .get("cwd")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                source: thread
                    .get("source")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                updated_at: thread
                    .get("updatedAt")
                    .and_then(Value::as_i64)
                    .unwrap_or_default(),
                archived,
            })
        })
        .collect();
    let next_cursor = result
        .get("nextCursor")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Ok((threads, next_cursor))
}

fn read_thread(server: &mut AppServer, thread_id: &str) -> Result<Vec<HistoryMessage>> {
    let result = server.request(
        "thread/read",
        json!({ "threadId": thread_id, "includeTurns": true }),
    )?;
    parse_thread_messages(&result)
}

fn read_thread_with_fallback(
    server: &mut AppServer,
    fallback_executables: &[PathBuf],
    thread_id: &str,
) -> Result<Vec<HistoryMessage>> {
    let primary_error = match read_thread(server, thread_id) {
        Ok(messages) => return Ok(messages),
        Err(error) if is_paginated_history_unsupported(&error) => error,
        Err(error) => return Err(error),
    };

    let mut fallback_errors = Vec::new();
    for executable in fallback_executables {
        let mut fallback = match AppServer::spawn(executable) {
            Ok(server) => server,
            Err(error) => {
                fallback_errors.push(format!("{}：{error:#}", executable.display()));
                continue;
            }
        };
        match read_thread(&mut fallback, thread_id) {
            Ok(messages) => {
                *server = fallback;
                return Ok(messages);
            }
            Err(error) => fallback_errors.push(format!("{}：{error:#}", executable.display())),
        }
    }

    if fallback_errors.is_empty() {
        Err(primary_error)
    } else {
        Err(anyhow!(
            "{primary_error:#}；Codex Desktop 兼容读取失败：{}",
            fallback_errors.join("；")
        ))
    }
}

fn is_paginated_history_unsupported(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .to_string()
            .contains("paginated_threads is not supported yet")
    })
}

#[cfg(target_os = "macos")]
fn desktop_codex_candidates() -> Vec<PathBuf> {
    let relative = [
        "Codex.app/Contents/Resources/codex",
        "ChatGPT.app/Contents/Resources/codex",
    ];
    let mut candidates = relative
        .iter()
        .map(|path| Path::new("/Applications").join(path))
        .collect::<Vec<_>>();
    if let Some(home) = std::env::var_os("HOME") {
        candidates.extend(
            relative
                .iter()
                .map(|path| PathBuf::from(&home).join("Applications").join(path)),
        );
    }
    candidates.retain(|path| path.is_file());
    candidates
}

#[cfg(not(target_os = "macos"))]
fn desktop_codex_candidates() -> Vec<PathBuf> {
    Vec::new()
}

fn parse_thread_messages(result: &Value) -> Result<Vec<HistoryMessage>> {
    let turns = result
        .pointer("/thread/turns")
        .and_then(Value::as_array)
        .context("Codex thread/read 响应缺少 turns")?;
    let mut messages = Vec::new();
    for turn in turns {
        for item in turn
            .get("items")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            match item.get("type").and_then(Value::as_str) {
                Some("userMessage") => {
                    let content = user_message_text(item);
                    if !content.is_empty() {
                        messages.push(HistoryMessage {
                            role: MessageRole::User,
                            kind: MessageKind::Text,
                            content,
                        });
                    }
                }
                Some("agentMessage") => {
                    if let Some(text) = item
                        .get("text")
                        .and_then(Value::as_str)
                        .filter(|text| !text.is_empty())
                    {
                        messages.push(HistoryMessage {
                            role: MessageRole::Assistant,
                            kind: MessageKind::Text,
                            content: text.to_owned(),
                        });
                    }
                }
                _ => {}
            }
        }
        if let Some(message) = turn.pointer("/error/message").and_then(Value::as_str) {
            messages.push(HistoryMessage {
                role: MessageRole::System,
                kind: MessageKind::Error,
                content: message.to_owned(),
            });
        }
    }
    Ok(messages)
}

fn user_message_text(item: &Value) -> String {
    item.get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|content| {
            (content.get("type").and_then(Value::as_str) == Some("text"))
                .then(|| content.get("text").and_then(Value::as_str))
                .flatten()
        })
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn compact_title(value: &str) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return "未命名会话".into();
    }
    if normalized.chars().count() <= 52 {
        normalized
    } else {
        let mut title: String = normalized.chars().take(52).collect();
        title.push('…');
        title
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt as _, time::Duration};

    use super::*;

    #[test]
    fn client_lists_active_and_archived_threads_and_reads_messages() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("codex");
        fs::write(
            &executable,
            r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"id":0,"result":{"codexHome":"/tmp/.codex"}}'
      ;;
    *'"method":"thread/list"'*'"archived":false'*)
      printf '%s\n' '{"id":1,"result":{"data":[{"id":"thread-1","preview":"Existing conversation","cwd":"/tmp/project","source":"vscode","updatedAt":20}],"nextCursor":null}}'
      ;;
    *'"method":"thread/list"'*'"archived":true'*)
      printf '%s\n' '{"id":2,"result":{"data":[{"id":"thread-0","name":"Archived conversation","cwd":"/tmp/old","source":"exec","updatedAt":10}],"nextCursor":null}}'
      ;;
    *'"method":"thread/read"'*)
      printf '%s\n' '{"id":3,"result":{"thread":{"turns":[{"items":[{"type":"userMessage","content":[{"type":"text","text":"hello"}]},{"type":"agentMessage","text":"world"}],"error":null}]}}}'
      ;;
  esac
done
"#,
        )
        .unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&executable, permissions).unwrap();

        let client = Client::spawn(executable);
        assert!(client.refresh());
        let Event::ThreadsLoaded(result) =
            client.events.recv_timeout(Duration::from_secs(2)).unwrap()
        else {
            panic!("expected thread list")
        };
        let threads = result.unwrap();
        assert_eq!(threads.len(), 2);
        assert_eq!(threads[0].title, "Existing conversation");
        assert_eq!(threads[0].detail(), "Desktop · project");
        assert!(threads[1].archived);

        assert!(client.read_thread("thread-1".into()));
        let Event::ThreadLoaded { result, .. } =
            client.events.recv_timeout(Duration::from_secs(2)).unwrap()
        else {
            panic!("expected thread messages")
        };
        let messages = result.unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].content, "hello");
        assert_eq!(messages[1].content, "world");
    }

    #[test]
    fn client_falls_back_to_desktop_codex_for_paginated_history() {
        let directory = tempfile::tempdir().unwrap();
        let primary = directory.path().join("codex-cli");
        let desktop = directory.path().join("codex-desktop");
        fs::write(
            &primary,
            r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"id":0,"result":{"codexHome":"/tmp/.codex"}}'
      ;;
    *'"method":"thread/read"'*)
      printf '%s\n' '{"id":1,"error":{"code":-32601,"message":"paginated_threads is not supported yet"}}'
      ;;
  esac
done
"#,
        )
        .unwrap();
        fs::write(
            &desktop,
            r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"id":0,"result":{"codexHome":"/tmp/.codex"}}'
      ;;
    *'"method":"thread/read"'*)
      printf '%s\n' '{"id":1,"result":{"thread":{"turns":[{"items":[{"type":"userMessage","content":[{"type":"text","text":"from desktop"}]}],"error":null}]}}}'
      ;;
  esac
done
"#,
        )
        .unwrap();
        for executable in [&primary, &desktop] {
            let mut permissions = fs::metadata(executable).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(executable, permissions).unwrap();
        }

        let client = Client::spawn_with_fallbacks(primary, vec![desktop]);
        assert!(client.read_thread("paginated-thread".into()));
        let Event::ThreadLoaded { result, .. } =
            client.events.recv_timeout(Duration::from_secs(2)).unwrap()
        else {
            panic!("expected thread messages")
        };
        let messages = result.unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "from desktop");
    }

    #[test]
    fn client_uses_desktop_codex_when_cli_is_missing() {
        let directory = tempfile::tempdir().unwrap();
        let missing_cli = directory.path().join("missing-codex-cli");
        let desktop = directory.path().join("codex-desktop");
        fs::write(
            &desktop,
            r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"id":0,"result":{"codexHome":"/tmp/.codex"}}'
      ;;
    *'"method":"thread/list"'*'"archived":false'*)
      printf '%s\n' '{"id":1,"result":{"data":[{"id":"desktop-thread","preview":"Desktop only","cwd":"/tmp/project","source":"vscode","updatedAt":20}],"nextCursor":null}}'
      ;;
    *'"method":"thread/list"'*'"archived":true'*)
      printf '%s\n' '{"id":2,"result":{"data":[],"nextCursor":null}}'
      ;;
  esac
done
"#,
        )
        .unwrap();
        let mut permissions = fs::metadata(&desktop).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&desktop, permissions).unwrap();

        let client = Client::spawn_with_fallbacks(missing_cli, vec![desktop]);
        assert!(client.refresh());
        let Event::ThreadsLoaded(result) =
            client.events.recv_timeout(Duration::from_secs(2)).unwrap()
        else {
            panic!("expected thread list")
        };
        let threads = result.unwrap();
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].title, "Desktop only");
    }
}
