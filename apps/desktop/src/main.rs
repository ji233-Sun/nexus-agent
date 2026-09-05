mod codex_history;
mod runner_client;
mod storage;

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command as SystemCommand,
    str::FromStr as _,
    time::Duration,
};

use codex_history::{
    Client as CodexHistoryClient, Event as CodexHistoryEvent, HistoryMessage,
    ThreadSummary as CodexThreadSummary,
};
use gpui::{
    AnyElement, App, AppContext as _, Application, Bounds, Context, Corner, ElementId, Entity,
    Hsla, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Timer, Window, WindowBounds, WindowOptions, div,
    prelude::FluentBuilder as _, px, relative, rgb, rgba, size,
};
use gpui_component::{
    Disableable as _, Root, Sizable as _, Theme, ThemeMode, box_shadow,
    button::{Button, ButtonVariants as _},
    input::{Input, InputState},
    menu::{DropdownMenu as _, PopupMenuItem},
    text::TextView,
};
use nexus_domain::{
    ClaudeModel, HarnessKind, Message, MessageKind, MessageRole, Project, RunStatus, TaskSummary,
    ThinkingEffort,
};
use nexus_protocol::{Command, CommandEnvelope, Event, HarnessProbe, StartRun};
use runner_client::RunnerClient;
use storage::Storage;
use uuid::Uuid;

const BG: u32 = 0xffffff;
const SURFACE: u32 = 0xffffff;
const SIDEBAR: u32 = 0xf7f7f5;
const RECESSED: u32 = 0xf3f3f1;
const HOVER: u32 = 0xededeb;
const SELECTED: u32 = 0xe7e7e4;
const BORDER: u32 = 0xe5e5e2;
const TEXT: u32 = 0x242424;
const TEXT_SECONDARY: u32 = 0x555555;
const MUTED: u32 = 0x858585;
const ACCENT: u32 = 0x202124;
const ACCENT_HOVER: u32 = 0x0f1011;
const LINK: u32 = 0x2f6fca;
const SUCCESS: u32 = 0x398e4a;
const WARNING: u32 = 0xff990a;
const DANGER: u32 = 0xe5484d;
const TOOL: u32 = 0xa35c16;
const RUNNER_MODE_ARG: &str = "--nexus-runner";

struct NexusApp {
    storage: Storage,
    runner: Option<RunnerClient>,
    projects: Vec<Project>,
    selected_project: Option<Project>,
    tasks: Vec<TaskSummary>,
    selected_task: Option<Uuid>,
    messages: Vec<Message>,
    active_run: Option<Uuid>,
    active_task: Option<Uuid>,
    active_harness: Option<HarnessKind>,
    streaming_text: String,
    status: String,
    harnesses: BTreeMap<HarnessKind, HarnessProbe>,
    codex_history_client: Option<CodexHistoryClient>,
    codex_history_executable: Option<String>,
    codex_threads: Vec<CodexThreadSummary>,
    selected_codex_thread: Option<String>,
    codex_history_messages: Vec<HistoryMessage>,
    codex_history_loading: bool,
    codex_thread_loading: bool,
    codex_history_error: Option<String>,
    selected_harness: HarnessKind,
    project_dirty: bool,
    model: ClaudeModel,
    effort: ThinkingEffort,
    prompt_input: Entity<InputState>,
    executable_input: Entity<InputState>,
}

impl NexusApp {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (storage, storage_error) = match Storage::open_default() {
            Ok(storage) => (storage, None),
            Err(error) => (
                Storage::open(Path::new(":memory:")).expect("open fallback database"),
                Some(format!(
                    "无法打开本地数据库，历史记录仅在本次运行有效：{error}"
                )),
            ),
        };
        let projects = storage.projects().unwrap_or_default();
        let selected_harness = storage
            .setting("default_harness")
            .ok()
            .flatten()
            .and_then(|value| HarnessKind::from_str(&value).ok())
            .unwrap_or_default();
        let model = storage
            .setting("claude_model")
            .ok()
            .flatten()
            .and_then(|value| ClaudeModel::from_str(&value).ok())
            .unwrap_or_default();
        let effort = storage
            .setting("thinking_effort")
            .ok()
            .flatten()
            .and_then(|value| ThinkingEffort::from_str(&value).ok())
            .unwrap_or_default();
        let executable = storage
            .setting(executable_setting_key(selected_harness))
            .ok()
            .flatten()
            .unwrap_or_else(|| selected_harness.default_executable().into());
        let prompt_input = cx.new(|cx| {
            InputState::new(window, cx)
                .auto_grow(3, 8)
                .placeholder("描述你希望 Agent 完成的工作…")
        });
        let executable_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(executable.clone())
                .placeholder("命令名或完整路径")
        });
        let has_storage_error = storage_error.is_some();
        let (runner, runner_error) = match RunnerClient::spawn() {
            Ok(runner) => (Some(runner), None),
            Err(error) => (None, Some(error.to_string())),
        };
        let mut app = Self {
            storage,
            runner,
            projects,
            selected_project: None,
            tasks: Vec::new(),
            selected_task: None,
            messages: Vec::new(),
            active_run: None,
            active_task: None,
            active_harness: None,
            streaming_text: String::new(),
            status: storage_error.unwrap_or_else(|| "正在连接本地 Runner…".into()),
            harnesses: BTreeMap::new(),
            codex_history_client: None,
            codex_history_executable: None,
            codex_threads: Vec::new(),
            selected_codex_thread: None,
            codex_history_messages: Vec::new(),
            codex_history_loading: false,
            codex_thread_loading: false,
            codex_history_error: None,
            selected_harness,
            project_dirty: false,
            model,
            effort,
            prompt_input,
            executable_input,
        };
        if let Some(runner) = &app.runner {
            let _ = runner.send(CommandEnvelope::new(Command::RunnerHello));
            for harness in HarnessKind::ALL {
                let executable = app
                    .storage
                    .setting(executable_setting_key(harness))
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| harness.default_executable().into());
                let _ = runner.send(CommandEnvelope::new(Command::HarnessProbe {
                    harness,
                    executable,
                }));
            }
        } else if !has_storage_error {
            app.status = runner_error.unwrap_or_default();
        }
        app.start_event_pump(cx);
        app
    }

    fn selected_probe(&self) -> Option<&HarnessProbe> {
        self.harnesses.get(&self.selected_harness)
    }

    fn start_event_pump(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_millis(33)).await;
                let Some(this) = this.upgrade() else { break };
                if this
                    .update(cx, |app, cx| {
                        app.drain_runner_events(cx);
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    fn drain_runner_events(&mut self, cx: &mut Context<Self>) {
        let runner_events = self
            .runner
            .as_ref()
            .map(RunnerClient::drain_events)
            .unwrap_or_default();
        let history_events = self
            .codex_history_client
            .as_ref()
            .map(CodexHistoryClient::drain_events)
            .unwrap_or_default();
        let changed = !runner_events.is_empty() || !history_events.is_empty();
        for envelope in runner_events {
            self.handle_event(envelope.event);
        }
        for event in history_events {
            self.handle_codex_history_event(event);
        }
        if changed {
            cx.notify();
        }
    }

    fn handle_event(&mut self, event: Event) {
        match event {
            Event::RunnerReady => {
                self.status = format!("Runner 已连接，正在探测 {}…", self.selected_harness)
            }
            Event::HarnessDetected(probe) => {
                let harness = probe.harness;
                let message = probe.message.clone();
                let history_executable =
                    (harness == HarnessKind::Codex).then(|| probe.executable.clone());
                self.harnesses.insert(harness, probe);
                if harness == self.selected_harness {
                    self.status = message;
                }
                if let Some(executable) = history_executable {
                    self.connect_codex_history(executable);
                }
            }
            Event::RunStarted { run_id, .. } => {
                self.active_run = Some(run_id);
                let harness = self.active_harness.unwrap_or(self.selected_harness);
                self.status = format!("{harness} 正在执行…");
                let _ = self.storage.update_run_status(run_id, RunStatus::Running);
            }
            Event::RunOutputDelta { run_id, text } if self.active_run == Some(run_id) => {
                self.streaming_text.push_str(&text);
            }
            Event::RunMessageCompleted { run_id, text } if self.active_run == Some(run_id) => {
                self.streaming_text.clear();
                self.persist_live_message(run_id, MessageRole::Assistant, MessageKind::Text, &text);
            }
            Event::RunToolStarted {
                run_id,
                name,
                summary,
                ..
            } if self.active_run == Some(run_id) => {
                let content = if summary.is_empty() {
                    name
                } else {
                    format!("{name}\n{summary}")
                };
                self.persist_live_message(
                    run_id,
                    MessageRole::Tool,
                    MessageKind::ToolCall,
                    &content,
                );
            }
            Event::RunToolCompleted {
                run_id,
                output,
                is_error,
                ..
            } if self.active_run == Some(run_id) => {
                let content = if is_error {
                    format!("工具执行失败\n{output}")
                } else {
                    output
                };
                self.persist_live_message(
                    run_id,
                    MessageRole::Tool,
                    MessageKind::ToolResult,
                    &content,
                );
            }
            Event::RunStatusChanged {
                run_id,
                status,
                message,
            } if self.active_run == Some(run_id) => {
                let _ = self.storage.update_run_status(run_id, status);
                if let Some(message) = message {
                    self.status = message;
                }
            }
            Event::RunFailed {
                run_id, message, ..
            } if self.active_run == Some(run_id) => {
                self.status = message.clone();
                self.persist_live_message(
                    run_id,
                    MessageRole::System,
                    MessageKind::Error,
                    &message,
                );
            }
            Event::RunExited {
                run_id,
                status,
                exit_code,
            } if self.active_run == Some(run_id) => {
                let _ = self.storage.finish_run(run_id, status, exit_code);
                self.streaming_text.clear();
                self.active_run = None;
                self.active_task = None;
                self.active_harness = None;
                self.status = match status {
                    RunStatus::Completed => "任务已完成".into(),
                    RunStatus::Cancelled => "任务已取消".into(),
                    RunStatus::Failed => "任务执行失败".into(),
                    _ => format!("任务状态：{status}"),
                };
                self.reload_tasks();
            }
            _ => {}
        }
    }

    fn connect_codex_history(&mut self, executable: String) {
        if self.codex_history_executable.as_deref() != Some(&executable) {
            self.codex_history_client = Some(CodexHistoryClient::spawn(PathBuf::from(&executable)));
            self.codex_history_executable = Some(executable);
        }
        self.request_codex_history_refresh();
    }

    fn request_codex_history_refresh(&mut self) {
        self.codex_history_error = None;
        self.codex_history_loading = self
            .codex_history_client
            .as_ref()
            .is_some_and(CodexHistoryClient::refresh);
        if !self.codex_history_loading {
            self.codex_history_error = Some("Codex 历史服务不可用。".into());
        }
    }

    fn handle_codex_history_event(&mut self, event: CodexHistoryEvent) {
        match event {
            CodexHistoryEvent::ThreadsLoaded(result) => {
                self.codex_history_loading = false;
                match result {
                    Ok(threads) => {
                        self.codex_threads = threads;
                        self.codex_history_error = None;
                    }
                    Err(error) => self.codex_history_error = Some(error),
                }
            }
            CodexHistoryEvent::ThreadLoaded { thread_id, result }
                if self.selected_codex_thread.as_deref() == Some(&thread_id) =>
            {
                self.codex_thread_loading = false;
                match result {
                    Ok(messages) => {
                        self.codex_history_messages = messages;
                        self.status = "Codex 历史会话已载入".into();
                    }
                    Err(error) => {
                        self.codex_history_messages = vec![HistoryMessage {
                            role: MessageRole::System,
                            kind: MessageKind::Error,
                            content: error,
                        }];
                        self.status = "无法读取 Codex 历史会话".into();
                    }
                }
            }
            CodexHistoryEvent::ThreadLoaded { .. } => {}
        }
    }

    fn persist_live_message(
        &mut self,
        run_id: Uuid,
        role: MessageRole,
        kind: MessageKind,
        content: &str,
    ) {
        let Some(task_id) = self.active_task else {
            return;
        };
        if let Ok(message) = self
            .storage
            .append_message(task_id, run_id, role, kind, content)
        {
            self.messages.push(message);
        }
    }

    fn choose_project(&mut self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let Some(folder) = rfd::AsyncFileDialog::new().pick_folder().await else {
                return;
            };
            let path = folder.path().to_owned();
            let _ = this.update(cx, |app, cx| {
                match app.storage.open_project(&path) {
                    Ok(project) => {
                        app.select_project(project);
                        app.projects = app.storage.projects().unwrap_or_default();
                    }
                    Err(error) => app.status = format!("无法打开项目：{error}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn select_project(&mut self, project: Project) {
        self.project_dirty = is_git_dirty(Path::new(&project.canonical_path));
        self.selected_project = Some(project);
        self.selected_task = None;
        self.selected_codex_thread = None;
        self.messages.clear();
        self.codex_history_messages.clear();
        self.codex_thread_loading = false;
        self.streaming_text.clear();
        self.reload_tasks();
    }

    fn reload_tasks(&mut self) {
        self.tasks = self
            .selected_project
            .as_ref()
            .and_then(|project| self.storage.tasks(project.id).ok())
            .unwrap_or_default();
    }

    fn select_task(&mut self, task_id: Uuid, cx: &mut Context<Self>) {
        self.selected_task = Some(task_id);
        self.selected_codex_thread = None;
        self.codex_history_messages.clear();
        self.codex_thread_loading = false;
        self.messages = self.storage.messages(task_id).unwrap_or_default();
        cx.notify();
    }

    fn select_codex_thread(&mut self, thread_id: String, cx: &mut Context<Self>) {
        if self.active_run.is_some() {
            self.status = "任务执行期间不能切换历史会话。".into();
            cx.notify();
            return;
        }
        self.selected_task = None;
        self.selected_codex_thread = Some(thread_id.clone());
        self.messages.clear();
        self.streaming_text.clear();
        self.codex_history_messages.clear();
        self.codex_thread_loading = self
            .codex_history_client
            .as_ref()
            .is_some_and(|client| client.read_thread(thread_id));
        self.status = if self.codex_thread_loading {
            "正在读取 Codex 历史会话…".into()
        } else {
            "Codex 历史服务不可用。".into()
        };
        cx.notify();
    }

    fn refresh_codex_history(
        &mut self,
        _: &gpui::ClickEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.request_codex_history_refresh();
        cx.notify();
    }

    fn submit(&mut self, _: &gpui::ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_run.is_some() {
            return;
        }
        let Some(project) = self.selected_project.clone() else {
            self.status = "请先选择项目目录。".into();
            cx.notify();
            return;
        };
        let prompt = self.prompt_input.read(cx).value().trim().to_owned();
        if prompt.is_empty() {
            self.status = "Prompt 不能为空。".into();
            cx.notify();
            return;
        }
        let configured_executable = self.executable_input.read(cx).value().trim().to_owned();
        if configured_executable.is_empty() {
            self.status = format!("{} 可执行文件不能为空。", self.selected_harness);
            cx.notify();
            return;
        }
        let Some(probe) = self
            .selected_probe()
            .filter(|probe| probe.available && probe.authenticated)
        else {
            self.status = format!("{} 尚未就绪，请先完成探测和登录。", self.selected_harness);
            cx.notify();
            return;
        };
        let executable = probe.executable.clone();
        let harness = self.selected_harness;
        let model = match harness {
            HarnessKind::Claude => self.model.cli_value().map(str::to_owned),
            HarnessKind::Codex => None,
        };
        let title: String = prompt.chars().take(48).collect();
        let created = self.storage.create_task_run(
            project.id,
            &title,
            &prompt,
            harness,
            model.as_deref(),
            self.effort,
        );
        let Ok((task_id, run_id)) = created else {
            self.status = "无法保存新任务。".into();
            cx.notify();
            return;
        };
        let command = CommandEnvelope::new(Command::RunStart(StartRun {
            run_id,
            task_id,
            cwd: project.canonical_path,
            prompt: prompt.clone(),
            harness,
            executable: executable.clone(),
            model,
            effort: self.effort,
        }));
        if let Some(runner) = &self.runner
            && runner.send(command).is_ok()
        {
            self.active_run = Some(run_id);
            self.active_task = Some(task_id);
            self.active_harness = Some(harness);
            self.selected_task = Some(task_id);
            self.selected_codex_thread = None;
            self.codex_history_messages.clear();
            self.codex_thread_loading = false;
            self.messages = self.storage.messages(task_id).unwrap_or_default();
            self.status = format!("正在启动 {harness} · {}", self.effort);
            let _ = self
                .storage
                .set_setting(executable_setting_key(harness), &configured_executable);
            self.prompt_input
                .update(cx, |input, cx| input.set_value("", window, cx));
            self.reload_tasks();
        } else {
            let _ = self.storage.finish_run(run_id, RunStatus::Failed, None);
            self.status = "Runner 不可用，任务未启动。".into();
        }
        cx.notify();
    }

    fn cancel(&mut self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(run_id) = self.active_run else {
            return;
        };
        if let Some(runner) = &self.runner {
            let _ = runner.send(CommandEnvelope::new(Command::RunCancel { run_id }));
            let _ = self
                .storage
                .update_run_status(run_id, RunStatus::Cancelling);
            let harness = self.active_harness.unwrap_or(self.selected_harness);
            self.status = format!("正在停止 {harness}…");
            cx.notify();
        }
    }

    fn probe(&mut self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        let executable = self.executable_input.read(cx).value().trim().to_owned();
        let harness = self.selected_harness;
        if let Some(runner) = &self.runner {
            let _ = runner.send(CommandEnvelope::new(Command::HarnessProbe {
                harness,
                executable: executable.clone(),
            }));
            let _ = self
                .storage
                .set_setting(executable_setting_key(harness), &executable);
            self.status = format!("正在探测 {harness}…");
            cx.notify();
        }
    }

    fn select_harness(
        &mut self,
        harness: HarnessKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active_run.is_some() || self.selected_harness == harness {
            return;
        }
        let current_executable = self.executable_input.read(cx).value().trim().to_owned();
        if !current_executable.is_empty() {
            let _ = self.storage.set_setting(
                executable_setting_key(self.selected_harness),
                &current_executable,
            );
        }

        self.selected_harness = harness;
        let _ = self
            .storage
            .set_setting("default_harness", self.selected_harness.as_str());
        let executable = self
            .storage
            .setting(executable_setting_key(self.selected_harness))
            .ok()
            .flatten()
            .unwrap_or_else(|| self.selected_harness.default_executable().into());
        self.executable_input
            .update(cx, |input, cx| input.set_value(&executable, window, cx));
        if let Some(runner) = &self.runner {
            let _ = runner.send(CommandEnvelope::new(Command::HarnessProbe {
                harness: self.selected_harness,
                executable,
            }));
            self.status = format!("正在探测 {}…", self.selected_harness);
        }
        cx.notify();
    }

    fn select_model(&mut self, model: ClaudeModel, cx: &mut Context<Self>) {
        if self.active_run.is_some()
            || self.selected_harness != HarnessKind::Claude
            || self.model == model
        {
            return;
        }
        self.model = model;
        let _ = self
            .storage
            .set_setting("claude_model", self.model.as_str());
        cx.notify();
    }

    fn select_effort(&mut self, effort: ThinkingEffort, cx: &mut Context<Self>) {
        if self.active_run.is_some() || self.effort == effort {
            return;
        }
        self.effort = effort;
        let _ = self
            .storage
            .set_setting("thinking_effort", self.effort.as_str());
        cx.notify();
    }

    fn harness_selector(
        &self,
        id: &'static str,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let selected = self.selected_harness;
        let app = cx.entity().clone();
        Button::new(id)
            .label(format!("{selected}  ⌄"))
            .disabled(self.active_run.is_some())
            .when(compact, |button| button.ghost().small())
            .when(!compact, |button| button.outline().w_full())
            .dropdown_menu_with_anchor(Corner::TopLeft, move |menu, _, _| {
                HarnessKind::ALL.into_iter().fold(
                    menu.min_w(if compact { px(160.) } else { px(220.) }),
                    |menu, harness| {
                        let app = app.clone();
                        menu.item(
                            PopupMenuItem::new(harness.to_string())
                                .checked(harness == selected)
                                .on_click(move |_, window, cx| {
                                    app.update(cx, |app, cx| {
                                        app.select_harness(harness, window, cx)
                                    });
                                }),
                        )
                    },
                )
            })
    }

    fn model_selector(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.model;
        let app = cx.entity().clone();
        let label = if self.selected_harness == HarnessKind::Claude {
            selected.to_string()
        } else {
            "CLI 默认模型".into()
        };
        Button::new("composer-model")
            .ghost()
            .small()
            .label(format!("{label}  ⌄"))
            .disabled(self.selected_harness == HarnessKind::Codex || self.active_run.is_some())
            .dropdown_menu_with_anchor(Corner::TopLeft, move |menu, _, _| {
                ClaudeModel::ALL
                    .into_iter()
                    .fold(menu.min_w(px(160.)), |menu, model| {
                        let app = app.clone();
                        menu.item(
                            PopupMenuItem::new(model.to_string())
                                .checked(model == selected)
                                .on_click(move |_, _, cx| {
                                    app.update(cx, |app, cx| app.select_model(model, cx));
                                }),
                        )
                    })
            })
    }

    fn effort_selector(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.effort;
        let app = cx.entity().clone();
        Button::new("composer-effort")
            .ghost()
            .small()
            .label(format!("{selected}  ⌄"))
            .disabled(self.active_run.is_some())
            .dropdown_menu_with_anchor(Corner::TopLeft, move |menu, _, _| {
                ThinkingEffort::ALL
                    .into_iter()
                    .fold(menu.min_w(px(140.)), |menu, effort| {
                        let app = app.clone();
                        menu.item(
                            PopupMenuItem::new(effort.to_string())
                                .checked(effort == selected)
                                .on_click(move |_, _, cx| {
                                    app.update(cx, |app, cx| app.select_effort(effort, cx));
                                }),
                        )
                    })
            })
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected_project_id = self.selected_project.as_ref().map(|project| project.id);
        let selected_codex_thread = self.selected_codex_thread.as_deref();
        let history_status = if self.codex_history_loading {
            "正在读取…".to_owned()
        } else if let Some(error) = &self.codex_history_error {
            format!("不可用：{error}")
        } else if self.codex_history_client.is_some() {
            format!("{} 条本机会话", self.codex_threads.len())
        } else {
            "等待检测 Codex CLI".to_owned()
        };
        div()
            .w(px(248.))
            .h_full()
            .flex_none()
            .bg(rgb(SIDEBAR))
            .border_r_1()
            .border_color(rgb(BORDER))
            .p_3()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .h(px(40.))
                    .px_2()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_base()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child("Nexus"),
                            )
                            .child(div().text_xs().text_color(rgb(MUTED)).child("⌄")),
                    ),
            )
            .child(
                Button::new("open-project")
                    .ghost()
                    .w_full()
                    .justify_start()
                    .child(button_label("＋  打开项目", TEXT))
                    .on_click(cx.listener(Self::choose_project)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().px_2().py_1().child(section_label("项目")))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .when(self.projects.is_empty(), |element| {
                                element.child(
                                    div()
                                        .px_2()
                                        .py_3()
                                        .text_xs()
                                        .text_color(rgb(MUTED))
                                        .child("尚未打开项目"),
                                )
                            })
                            .children(self.projects.iter().map(|project| {
                                let id = project.id;
                                let selected = selected_project_id == Some(id);
                                let display_name = project.display_name.clone();
                                let project = project.clone();
                                div()
                                    .id(SharedString::from(format!("project-{id}")))
                                    .h(px(34.))
                                    .px_2()
                                    .rounded(px(8.))
                                    .cursor_pointer()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .text_sm()
                                    .text_color(rgb(TEXT_SECONDARY))
                                    .when(selected, |element| {
                                        element.bg(rgb(SELECTED)).text_color(rgb(TEXT))
                                    })
                                    .hover(|style| style.bg(rgb(HOVER)).text_color(rgb(TEXT)))
                                    .on_click(cx.listener(move |app, _, _, cx| {
                                        app.select_project(project.clone());
                                        cx.notify();
                                    }))
                                    .child(div().text_color(rgb(MUTED)).child("▱"))
                                    .child(display_name)
                            })),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().px_2().py_1().child(section_label("Nexus 记录")))
                    .child(
                        div()
                            .id("task-list")
                            .flex_1()
                            .overflow_y_scroll()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .when(
                                self.selected_project.is_some() && self.tasks.is_empty(),
                                |element| {
                                    element.child(
                                        div()
                                            .px_2()
                                            .py_3()
                                            .text_xs()
                                            .text_color(rgb(MUTED))
                                            .child("发送 Prompt 后，任务会显示在这里"),
                                    )
                                },
                            )
                            .children(self.tasks.iter().map(|task| {
                                let task_id = task.id;
                                let selected = self.selected_task == Some(task_id);
                                div()
                                    .id(SharedString::from(format!("task-{task_id}")))
                                    .p_2()
                                    .rounded(px(8.))
                                    .cursor_pointer()
                                    .text_color(rgb(TEXT_SECONDARY))
                                    .when(selected, |element| {
                                        element.bg(rgb(SELECTED)).text_color(rgb(TEXT))
                                    })
                                    .hover(|style| style.bg(rgb(HOVER)).text_color(rgb(TEXT)))
                                    .on_click(cx.listener(move |app, _, _, cx| {
                                        app.select_task(task_id, cx)
                                    }))
                                    .child(div().text_sm().line_clamp(2).child(task.title.clone()))
                                    .child(
                                        div()
                                            .mt_1()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .text_xs()
                                            .text_color(rgb(MUTED))
                                            .child(status_dot(run_status_color(task.status)))
                                            .child(task.status.to_string()),
                                    )
                            }))
                            .child(
                                div()
                                    .mt_3()
                                    .px_2()
                                    .py_1()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(section_label("Codex 历史"))
                                    .child(
                                        Button::new("refresh-codex-history")
                                            .ghost()
                                            .small()
                                            .child(button_label("刷新", TEXT))
                                            .disabled(
                                                self.codex_history_client.is_none()
                                                    || self.codex_history_loading,
                                            )
                                            .on_click(cx.listener(Self::refresh_codex_history)),
                                    ),
                            )
                            .child(
                                div()
                                    .px_2()
                                    .py_1()
                                    .text_xs()
                                    .text_color(rgb(MUTED))
                                    .line_clamp(3)
                                    .child(history_status),
                            )
                            .children(self.codex_threads.iter().map(|thread| {
                                let thread_id = thread.id.clone();
                                let selected = selected_codex_thread == Some(thread.id.as_str());
                                div()
                                    .id(SharedString::from(format!("codex-thread-{}", thread.id)))
                                    .p_2()
                                    .rounded(px(8.))
                                    .cursor_pointer()
                                    .text_color(rgb(TEXT_SECONDARY))
                                    .when(selected, |element| {
                                        element.bg(rgb(SELECTED)).text_color(rgb(TEXT))
                                    })
                                    .hover(|style| style.bg(rgb(HOVER)).text_color(rgb(TEXT)))
                                    .on_click(cx.listener(move |app, _, _, cx| {
                                        app.select_codex_thread(thread_id.clone(), cx)
                                    }))
                                    .child(
                                        div().text_sm().line_clamp(2).child(thread.title.clone()),
                                    )
                                    .child(
                                        div()
                                            .mt_1()
                                            .text_xs()
                                            .text_color(rgb(MUTED))
                                            .child(thread.detail()),
                                    )
                            })),
                    ),
            )
    }

    fn render_timeline(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let showing_codex_history = self.selected_codex_thread.is_some();
        let empty = if showing_codex_history {
            self.codex_history_messages.is_empty()
        } else {
            self.messages.is_empty() && self.streaming_text.is_empty()
        };
        div()
            .id("timeline")
            .flex_1()
            .h_full()
            .overflow_y_scroll()
            .bg(rgb(SURFACE))
            .child(
                div()
                    .w_full()
                    .max_w(px(720.))
                    .min_h_full()
                    .mx_auto()
                    .px_8()
                    .pt_8()
                    .pb_12()
                    .flex()
                    .flex_col()
                    .gap_6()
                    .when(empty, |element| {
                        let (title, description) = if self.codex_thread_loading {
                            ("正在读取 Codex 历史", "正在从本机 Codex 会话中加载消息。")
                        } else if showing_codex_history {
                            ("此会话没有消息", "没有可显示的用户或助手消息。")
                        } else if self.selected_project.is_some() {
                            (
                                "准备开始新任务",
                                "在下方描述目标，Agent 的执行过程会显示在这里。",
                            )
                        } else {
                            (
                                "选择一个本地项目",
                                "选择项目开始任务，或从左侧浏览 Codex 历史。",
                            )
                        };
                        element.child(
                            div()
                                .flex_1()
                                .flex()
                                .flex_col()
                                .items_center()
                                .justify_center()
                                .gap_2()
                                .text_center()
                                .child(
                                    div()
                                        .text_xl()
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .child(title),
                                )
                                .child(
                                    div()
                                        .max_w(px(420.))
                                        .text_sm()
                                        .text_color(rgb(MUTED))
                                        .line_height(relative(1.5))
                                        .child(description),
                                ),
                        )
                    })
                    .when(!showing_codex_history, |element| {
                        element.children(
                            self.messages
                                .iter()
                                .map(|message| render_message(message, window, cx)),
                        )
                    })
                    .when(showing_codex_history, |element| {
                        element.children(self.codex_history_messages.iter().enumerate().map(
                            |(index, message)| render_history_message(index, message, window, cx),
                        ))
                    })
                    .when(
                        !showing_codex_history && !self.streaming_text.is_empty(),
                        |element| {
                            element.child(message_card(
                                "streaming-message",
                                MessageRole::Assistant,
                                "Agent · 正在生成",
                                &self.streaming_text,
                                MessageKind::Text,
                                window,
                                cx,
                            ))
                        },
                    ),
            )
    }

    fn render_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let probe = self.selected_probe();
        let harness_color: Hsla = probe
            .map(|probe| {
                if probe.available && probe.authenticated {
                    rgb(SUCCESS).into()
                } else {
                    rgb(DANGER).into()
                }
            })
            .unwrap_or_else(|| rgb(MUTED).into());
        div()
            .id("settings-panel")
            .w(px(264.))
            .h_full()
            .flex_none()
            .overflow_y_scroll()
            .bg(rgb(SIDEBAR))
            .border_l_1()
            .border_color(rgb(BORDER))
            .p_4()
            .flex()
            .flex_col()
            .gap_5()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_base()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("运行设置"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child("当前项目的本地执行环境"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .rounded(px(10.))
                    .bg(rgb(SURFACE))
                    .shadow(surface_border_shadow())
                    .p_3()
                    .child(section_label("AGENT"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child("类型 · 点击选择"),
                    )
                    .child(self.harness_selector("settings-harness", false, cx))
                    .child(div().text_xs().text_color(rgb(MUTED)).child("可执行文件"))
                    .child(Input::new(&self.executable_input))
                    .child(
                        Button::new("probe")
                            .outline()
                            .small()
                            .w_full()
                            .child(button_label("重新探测", TEXT))
                            .disabled(self.active_run.is_some())
                            .on_click(cx.listener(Self::probe)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .gap_2()
                            .text_xs()
                            .text_color(rgb(TEXT_SECONDARY))
                            .child(status_dot(harness_color))
                            .child(
                                probe
                                    .map(|probe| probe.message.clone())
                                    .unwrap_or_else(|| "尚未探测".into()),
                            ),
                    )
                    .when_some(
                        probe.and_then(|probe| probe.version.clone()),
                        |element, version| element.child(label_value("版本", version)),
                    ),
            )
            .when_some(self.selected_project.as_ref(), |element, project| {
                element
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .rounded(px(10.))
                            .bg(rgb(SURFACE))
                            .shadow(surface_border_shadow())
                            .p_3()
                            .child(section_label("WORKSPACE"))
                            .child(label_value("工作目录", project.canonical_path.clone())),
                    )
                    .when(self.project_dirty, |element| {
                        element.child(
                            div()
                                .flex()
                                .items_start()
                                .gap_2()
                                .text_xs()
                                .text_color(rgb(TEXT_SECONDARY))
                                .child(status_dot(rgb(WARNING).into()))
                                .child("目录存在未提交修改；Nexus 不会自动还原或提交。"),
                        )
                    })
            })
            .child(div().flex_1())
            .when(self.active_run.is_some(), |element| {
                element.child(
                    Button::new("cancel")
                        .danger()
                        .outline()
                        .w_full()
                        .child(button_label("取消运行", DANGER))
                        .on_click(cx.listener(Self::cancel)),
                )
            })
    }
}

impl Render for NexusApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let probe = self.selected_probe();
        let can_submit = self.selected_project.is_some()
            && self.active_run.is_none()
            && probe.is_some_and(|probe| probe.available && probe.authenticated);
        let header_status_color = if self.active_run.is_some() {
            rgb(ACCENT).into()
        } else {
            probe
                .map(|probe| {
                    if probe.available && probe.authenticated {
                        rgb(SUCCESS).into()
                    } else {
                        rgb(DANGER).into()
                    }
                })
                .unwrap_or_else(|| rgb(MUTED).into())
        };
        let header_title = self
            .selected_codex_thread
            .as_ref()
            .and_then(|thread_id| {
                self.codex_threads
                    .iter()
                    .find(|thread| &thread.id == thread_id)
            })
            .map(|thread| format!("Codex 历史 · {}", thread.title))
            .or_else(|| {
                self.selected_project
                    .as_ref()
                    .map(|project| project.display_name.clone())
            })
            .unwrap_or_else(|| "未选择项目".into());
        let header_context = if self.selected_codex_thread.is_some() {
            Some("Codex 原有会话")
        } else if self.selected_task.is_some() {
            Some("任务时间线")
        } else {
            None
        };
        div()
            .size_full()
            .bg(rgb(BG))
            .text_color(rgb(TEXT))
            .text_sm()
            .flex()
            .child(self.render_sidebar(cx))
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .min_w_0()
                    .bg(rgb(SURFACE))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .h(px(50.))
                            .flex_none()
                            .bg(rgb(SURFACE))
                            .border_b_1()
                            .border_color(rgb(BORDER))
                            .px_5()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .min_w_0()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(div().text_color(rgb(MUTED)).child("▱"))
                                    .child(
                                        div()
                                            .truncate()
                                            .font_weight(gpui::FontWeight::MEDIUM)
                                            .child(header_title),
                                    )
                                    .when_some(header_context, |element, context| {
                                        element.child(
                                            div()
                                                .flex_none()
                                                .text_xs()
                                                .text_color(rgb(MUTED))
                                                .child(format!("· {context}")),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .text_xs()
                                    .text_color(rgb(MUTED))
                                    .child(status_dot(header_status_color))
                                    .child(
                                        div().max_w(px(260.)).truncate().child(self.status.clone()),
                                    ),
                            ),
                    )
                    .child(self.render_timeline(window, cx))
                    .child(
                        div()
                            .flex_none()
                            .bg(rgb(SURFACE))
                            .px_6()
                            .pt_2()
                            .pb_5()
                            .child(
                                div()
                                    .w_full()
                                    .max_w(px(720.))
                                    .mx_auto()
                                    .rounded(px(18.))
                                    .bg(rgb(SURFACE))
                                    .shadow(surface_card_shadow())
                                    .p_2()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        Input::new(&self.prompt_input)
                                            .h(px(72.))
                                            .appearance(false)
                                            .focus_bordered(false),
                                    )
                                    .child(
                                        div()
                                            .h(px(38.))
                                            .px_1()
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap_1()
                                                    .child(self.harness_selector(
                                                        "composer-harness",
                                                        true,
                                                        cx,
                                                    ))
                                                    .child(self.model_selector(cx))
                                                    .child(self.effort_selector(cx)),
                                            )
                                            .child(
                                                Button::new("submit")
                                                    .primary()
                                                    .rounded(px(18.))
                                                    .size(px(36.))
                                                    .p_0()
                                                    .child(button_label(
                                                        if self.active_run.is_some() {
                                                            "…"
                                                        } else {
                                                            "↑"
                                                        },
                                                        SURFACE,
                                                    ))
                                                    .when(!can_submit, |button| {
                                                        button.opacity(0.42)
                                                    })
                                                    .disabled(!can_submit)
                                                    .on_click(cx.listener(Self::submit)),
                                            ),
                                    ),
                            ),
                    ),
            )
            .child(self.render_settings(cx))
    }
}

fn render_message(
    message: &Message,
    window: &mut Window,
    cx: &mut Context<NexusApp>,
) -> AnyElement {
    let label = match message.role {
        MessageRole::User => "You",
        MessageRole::Assistant => "Agent",
        MessageRole::Tool => "Tool",
        MessageRole::System => "System",
    };
    message_card(
        message.id,
        message.role,
        label,
        &message.content,
        message.kind,
        window,
        cx,
    )
}

fn render_history_message(
    index: usize,
    message: &HistoryMessage,
    window: &mut Window,
    cx: &mut Context<NexusApp>,
) -> AnyElement {
    let label = match message.role {
        MessageRole::User => "You · Codex 历史",
        MessageRole::Assistant => "Codex · 历史",
        MessageRole::Tool => "Tool · Codex 历史",
        MessageRole::System => "System · Codex 历史",
    };
    message_card(
        ("history-message", index),
        message.role,
        label,
        &message.content,
        message.kind,
        window,
        cx,
    )
}

fn message_indicator(kind: MessageKind) -> Hsla {
    match kind {
        MessageKind::Error => rgb(DANGER).into(),
        MessageKind::ToolCall | MessageKind::ToolResult => rgb(TOOL).into(),
        _ => rgb(MUTED).into(),
    }
}

fn message_card(
    id: impl Into<ElementId>,
    role: MessageRole,
    label: &str,
    content: &str,
    kind: MessageKind,
    window: &mut Window,
    cx: &mut Context<NexusApp>,
) -> AnyElement {
    let is_user = role == MessageRole::User;
    let is_panel = matches!(
        kind,
        MessageKind::ToolCall | MessageKind::ToolResult | MessageKind::Error
    );
    let show_label = !is_user && (role != MessageRole::Assistant || is_panel);
    div()
        .w_full()
        .flex()
        .when(is_user, |element| element.justify_end())
        .child(
            div()
                .when(is_user, |element| {
                    element
                        .max_w(px(600.))
                        .rounded(px(18.))
                        .bg(rgb(RECESSED))
                        .px_4()
                        .py_3()
                })
                .when(!is_user, |element| element.w_full())
                .when(!is_user && is_panel, |element| {
                    element
                        .rounded(px(10.))
                        .bg(rgb(SIDEBAR))
                        .shadow(surface_border_shadow())
                        .p_4()
                })
                .when(!is_user && !is_panel, |element| element.px_1().py_2())
                .when(show_label, |element| {
                    element.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .mb_2()
                            .child(status_dot(message_indicator(kind)))
                            .child(label.to_owned()),
                    )
                })
                .child(if kind == MessageKind::Text {
                    TextView::markdown(id, content.to_owned(), window, cx)
                        .selectable(true)
                        .into_any_element()
                } else {
                    div()
                        .text_sm()
                        .text_color(if kind == MessageKind::Status {
                            rgb(TEXT_SECONDARY)
                        } else {
                            rgb(TEXT)
                        })
                        .whitespace_normal()
                        .line_height(relative(1.55))
                        .when(
                            matches!(kind, MessageKind::ToolCall | MessageKind::ToolResult),
                            |element| element.font_family("SF Mono"),
                        )
                        .child(content.to_owned())
                        .into_any_element()
                }),
        )
        .into_any_element()
}

fn label_value(label: impl Into<SharedString>, value: impl Into<SharedString>) -> impl IntoElement {
    let label: SharedString = label.into();
    let value: SharedString = value.into();
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_xs().text_color(rgb(MUTED)).child(label))
        .child(
            div()
                .text_sm()
                .text_color(rgb(TEXT_SECONDARY))
                .whitespace_normal()
                .child(value),
        )
}

fn executable_setting_key(harness: HarnessKind) -> &'static str {
    match harness {
        HarnessKind::Claude => "claude_executable",
        HarnessKind::Codex => "codex_executable",
    }
}

fn section_label(label: impl Into<SharedString>) -> impl IntoElement {
    div()
        .text_xs()
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(rgb(MUTED))
        .child(label.into())
}

fn button_label(label: impl Into<SharedString>, color: u32) -> impl IntoElement {
    div().text_color(rgb(color)).child(label.into())
}

fn status_dot(color: Hsla) -> impl IntoElement {
    div()
        .size(px(8.))
        .mt(px(3.))
        .flex_none()
        .rounded_full()
        .bg(color)
}

fn run_status_color(status: RunStatus) -> Hsla {
    match status {
        RunStatus::Completed => rgb(SUCCESS).into(),
        RunStatus::Failed => rgb(DANGER).into(),
        RunStatus::Running | RunStatus::Starting | RunStatus::Cancelling => rgb(ACCENT).into(),
        _ => rgb(MUTED).into(),
    }
}

fn surface_border_shadow() -> Vec<gpui::BoxShadow> {
    vec![box_shadow(0., 0., 0., 1., rgba(0x00000012).into())]
}

fn surface_card_shadow() -> Vec<gpui::BoxShadow> {
    vec![
        box_shadow(0., 0., 0., 1., rgba(0x00000018).into()),
        box_shadow(0., 2., 6., -2., rgba(0x00000014).into()),
        box_shadow(0., 14., 32., -12., rgba(0x00000024).into()),
    ]
}

fn configure_theme(cx: &mut App) {
    Theme::change(ThemeMode::Light, None, cx);
    let theme = Theme::global_mut(cx);
    theme.font_family = ".SystemUIFont".into();
    theme.font_size = px(14.);
    theme.mono_font_family = "SF Mono".into();
    theme.mono_font_size = px(13.);
    theme.radius = px(6.);
    theme.radius_lg = px(12.);
    theme.shadow = false;

    theme.background = rgb(BG).into();
    theme.foreground = rgb(TEXT).into();
    theme.border = rgb(BORDER).into();
    theme.input = rgba(0x00000024).into();
    theme.caret = rgb(TEXT).into();
    theme.ring = rgb(TEXT_SECONDARY).into();
    theme.selection = rgba(0x20212424).into();
    theme.muted = rgb(RECESSED).into();
    theme.muted_foreground = rgb(MUTED).into();
    theme.accent = rgb(HOVER).into();
    theme.accent_foreground = rgb(TEXT).into();
    theme.primary = rgb(ACCENT).into();
    theme.primary_hover = rgb(ACCENT_HOVER).into();
    theme.primary_active = rgb(0x000000).into();
    theme.primary_foreground = rgb(SURFACE).into();
    theme.secondary = rgb(SURFACE).into();
    theme.secondary_hover = rgb(HOVER).into();
    theme.secondary_active = rgb(RECESSED).into();
    theme.secondary_foreground = rgb(TEXT_SECONDARY).into();
    theme.link = rgb(LINK).into();
    theme.link_hover = rgb(0x245aa6).into();
    theme.link_active = rgb(0x1e4c8e).into();
    theme.danger = rgb(DANGER).into();
    theme.danger_hover = rgb(0xc93439).into();
    theme.danger_active = rgb(0xa9272c).into();
    theme.danger_foreground = rgb(SURFACE).into();
    theme.sidebar = rgb(SIDEBAR).into();
    theme.sidebar_foreground = rgb(TEXT).into();
    theme.sidebar_accent = rgb(HOVER).into();
    theme.sidebar_accent_foreground = rgb(TEXT).into();
    theme.sidebar_border = rgb(BORDER).into();
}

fn is_git_dirty(path: &Path) -> bool {
    SystemCommand::new("git")
        .args(["status", "--porcelain"])
        .current_dir(path)
        .output()
        .map(|output| output.status.success() && !output.stdout.is_empty())
        .unwrap_or(false)
}

fn main() -> anyhow::Result<()> {
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == RUNNER_MODE_ARG)
    {
        return nexus_runner::run();
    }

    Application::new().run(|cx: &mut App| {
        gpui_component::init(cx);
        configure_theme(cx);
        let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
        cx.spawn(async move |cx| {
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            };
            cx.open_window(options, |window, cx| {
                let view = cx.new(|cx| NexusApp::new(window, cx));
                cx.new(|cx| Root::new(view, window, cx))
            })?;
            Ok::<_, anyhow::Error>(())
        })
        .detach();
    });
    Ok(())
}
