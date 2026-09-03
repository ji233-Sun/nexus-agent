mod runner_client;
mod storage;

use std::{path::Path, process::Command as SystemCommand, str::FromStr as _, time::Duration};

use gpui::{
    App, AppContext as _, Application, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Timer,
    Window, WindowOptions, div, prelude::FluentBuilder as _, px, rgb,
};
use gpui_component::{
    Disableable as _, Root, Sizable as _,
    button::{Button, ButtonVariants as _},
    input::{Input, InputState},
};
use nexus_domain::{
    ClaudeModel, Message, MessageKind, MessageRole, Project, RunStatus, TaskSummary, ThinkingEffort,
};
use nexus_protocol::{Command, CommandEnvelope, Event, HarnessProbe, StartRun};
use runner_client::RunnerClient;
use storage::Storage;
use uuid::Uuid;

const BG: u32 = 0x0b0d12;
const PANEL: u32 = 0x121620;
const PANEL_ALT: u32 = 0x181d29;
const BORDER: u32 = 0x252c3a;
const TEXT: u32 = 0xe7eaf0;
const MUTED: u32 = 0x8d96a8;
const ACCENT: u32 = 0x8b7cf6;
const SUCCESS: u32 = 0x52c79a;
const DANGER: u32 = 0xf07178;

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
    streaming_text: String,
    status: String,
    harness: Option<HarnessProbe>,
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
            .setting("claude_executable")
            .ok()
            .flatten()
            .unwrap_or_else(|| "claude".into());
        let prompt_input = cx.new(|cx| {
            InputState::new(window, cx)
                .auto_grow(3, 8)
                .placeholder("描述你希望 Claude Code 完成的工作…")
        });
        let executable_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(executable.clone())
                .placeholder("claude 或完整路径")
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
            streaming_text: String::new(),
            status: storage_error.unwrap_or_else(|| "正在连接本地 Runner…".into()),
            harness: None,
            project_dirty: false,
            model,
            effort,
            prompt_input,
            executable_input,
        };
        if let Some(runner) = &app.runner {
            let _ = runner.send(CommandEnvelope::new(Command::RunnerHello));
            let _ = runner.send(CommandEnvelope::new(Command::HarnessProbe { executable }));
        } else if !has_storage_error {
            app.status = runner_error.unwrap_or_default();
        }
        app.start_event_pump(cx);
        app
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
        let events = self
            .runner
            .as_ref()
            .map(RunnerClient::drain_events)
            .unwrap_or_default();
        if events.is_empty() {
            return;
        }
        for envelope in events {
            self.handle_event(envelope.event);
        }
        cx.notify();
    }

    fn handle_event(&mut self, event: Event) {
        match event {
            Event::RunnerReady => self.status = "Runner 已连接，正在探测 Claude Code…".into(),
            Event::HarnessDetected(probe) => {
                self.status = probe.message.clone();
                self.harness = Some(probe);
            }
            Event::RunStarted { run_id, .. } => {
                self.active_run = Some(run_id);
                self.status = "Claude Code 正在执行…".into();
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
        self.messages.clear();
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
        self.messages = self.storage.messages(task_id).unwrap_or_default();
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
        let executable = self.executable_input.read(cx).value().trim().to_owned();
        if executable.is_empty() {
            self.status = "Claude Code 可执行文件不能为空。".into();
            cx.notify();
            return;
        }
        if !self
            .harness
            .as_ref()
            .is_some_and(|probe| probe.available && probe.authenticated)
        {
            self.status = "Claude Code 尚未就绪，请先完成探测和登录。".into();
            cx.notify();
            return;
        }
        let title: String = prompt.chars().take(48).collect();
        let created =
            self.storage
                .create_task_run(project.id, &title, &prompt, self.model, self.effort);
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
            executable: executable.clone(),
            model: self.model,
            effort: self.effort,
        }));
        if let Some(runner) = &self.runner
            && runner.send(command).is_ok()
        {
            self.active_run = Some(run_id);
            self.active_task = Some(task_id);
            self.selected_task = Some(task_id);
            self.messages = self.storage.messages(task_id).unwrap_or_default();
            self.status = format!("正在启动 Claude Code · {} · {}", self.model, self.effort);
            let _ = self.storage.set_setting("claude_executable", &executable);
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
            self.status = "正在停止 Claude Code…".into();
            cx.notify();
        }
    }

    fn probe(&mut self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        let executable = self.executable_input.read(cx).value().trim().to_owned();
        if let Some(runner) = &self.runner {
            let _ = runner.send(CommandEnvelope::new(Command::HarnessProbe {
                executable: executable.clone(),
            }));
            let _ = self.storage.set_setting("claude_executable", &executable);
            self.status = "正在探测 Claude Code…".into();
            cx.notify();
        }
    }

    fn cycle_model(&mut self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.model = self.model.next();
        let _ = self
            .storage
            .set_setting("claude_model", self.model.as_str());
        cx.notify();
    }

    fn cycle_effort(&mut self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.effort = self.effort.next();
        let _ = self
            .storage
            .set_setting("thinking_effort", self.effort.as_str());
        cx.notify();
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected_project_id = self.selected_project.as_ref().map(|project| project.id);
        div()
            .w(px(244.))
            .h_full()
            .flex_none()
            .bg(rgb(PANEL))
            .border_r_1()
            .border_color(rgb(BORDER))
            .p_3()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("Nexus Agent"),
                    )
                    .child(
                        Button::new("open-project")
                            .label("＋ 项目")
                            .small()
                            .on_click(cx.listener(Self::choose_project)),
                    ),
            )
            .child(div().text_xs().text_color(rgb(MUTED)).child("最近项目"))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .children(self.projects.iter().map(|project| {
                        let id = project.id;
                        let selected = selected_project_id == Some(id);
                        let display_name = project.display_name.clone();
                        let project = project.clone();
                        div()
                            .id(SharedString::from(format!("project-{id}")))
                            .px_2()
                            .py_2()
                            .rounded_md()
                            .cursor_pointer()
                            .when(selected, |element| element.bg(rgb(PANEL_ALT)))
                            .hover(|style| style.bg(rgb(PANEL_ALT)))
                            .on_click(cx.listener(move |app, _, _, cx| {
                                app.select_project(project.clone());
                                cx.notify();
                            }))
                            .child(display_name)
                    })),
            )
            .child(div().mt_2().text_xs().text_color(rgb(MUTED)).child("任务"))
            .child(
                div()
                    .id("task-list")
                    .flex_1()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .children(self.tasks.iter().map(|task| {
                        let task_id = task.id;
                        let selected = self.selected_task == Some(task_id);
                        div()
                            .id(SharedString::from(format!("task-{task_id}")))
                            .p_2()
                            .rounded_md()
                            .cursor_pointer()
                            .when(selected, |element| element.bg(rgb(PANEL_ALT)))
                            .hover(|style| style.bg(rgb(PANEL_ALT)))
                            .on_click(
                                cx.listener(move |app, _, _, cx| app.select_task(task_id, cx)),
                            )
                            .child(div().text_sm().line_clamp(2).child(task.title.clone()))
                            .child(
                                div()
                                    .mt_1()
                                    .text_xs()
                                    .text_color(status_color(task.status))
                                    .child(task.status.to_string()),
                            )
                    })),
            )
    }

    fn render_timeline(&self) -> impl IntoElement {
        let empty = self.messages.is_empty() && self.streaming_text.is_empty();
        div()
            .id("timeline")
            .flex_1()
            .h_full()
            .overflow_y_scroll()
            .p_5()
            .flex()
            .flex_col()
            .gap_3()
            .when(empty, |element| {
                element.child(
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(rgb(MUTED))
                        .child(if self.selected_project.is_some() {
                            "输入 Prompt，开始一个 Claude Code 任务"
                        } else {
                            "选择一个本地项目开始"
                        }),
                )
            })
            .children(self.messages.iter().map(render_message))
            .when(!self.streaming_text.is_empty(), |element| {
                element.child(message_card(
                    "Claude · streaming",
                    &self.streaming_text,
                    MessageKind::Text,
                ))
            })
    }

    fn render_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let harness_color = self
            .harness
            .as_ref()
            .map(|probe| {
                if probe.available && probe.authenticated {
                    rgb(SUCCESS)
                } else {
                    rgb(DANGER)
                }
            })
            .unwrap_or_else(|| rgb(MUTED));
        div()
            .w(px(272.))
            .h_full()
            .flex_none()
            .bg(rgb(PANEL))
            .border_l_1()
            .border_color(rgb(BORDER))
            .p_4()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .text_base()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("运行配置"),
            )
            .child(label_value("Harness", "Claude Code"))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().text_xs().text_color(rgb(MUTED)).child("可执行文件"))
                    .child(Input::new(&self.executable_input))
                    .child(
                        Button::new("probe")
                            .label("重新探测")
                            .small()
                            .on_click(cx.listener(Self::probe)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child("模型（点击切换）"),
                    )
                    .child(
                        Button::new("model")
                            .label(self.model.to_string())
                            .on_click(cx.listener(Self::cycle_model)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child("思考层级（点击切换）"),
                    )
                    .child(
                        Button::new("effort")
                            .label(self.effort.to_string())
                            .on_click(cx.listener(Self::cycle_effort)),
                    ),
            )
            .child(
                div().text_sm().text_color(harness_color).child(
                    self.harness
                        .as_ref()
                        .map(|probe| probe.message.clone())
                        .unwrap_or_else(|| "尚未探测".into()),
                ),
            )
            .when_some(
                self.harness
                    .as_ref()
                    .and_then(|probe| probe.version.clone()),
                |element, version| element.child(label_value("版本", version)),
            )
            .when_some(self.selected_project.as_ref(), |element, project| {
                element
                    .child(label_value("工作目录", project.canonical_path.clone()))
                    .when(self.project_dirty, |element| {
                        element.child(
                            div()
                                .text_xs()
                                .text_color(rgb(0xe6b450))
                                .child("此目录存在未提交修改；Nexus 不会自动还原或提交。"),
                        )
                    })
            })
            .child(div().flex_1())
            .when(self.active_run.is_some(), |element| {
                element.child(
                    Button::new("cancel")
                        .label("取消运行")
                        .danger()
                        .on_click(cx.listener(Self::cancel)),
                )
            })
    }
}

impl Render for NexusApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let can_submit = self.selected_project.is_some()
            && self.active_run.is_none()
            && self
                .harness
                .as_ref()
                .is_some_and(|probe| probe.available && probe.authenticated);
        div()
            .size_full()
            .bg(rgb(BG))
            .text_color(rgb(TEXT))
            .flex()
            .child(self.render_sidebar(cx))
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .h(px(54.))
                            .flex_none()
                            .border_b_1()
                            .border_color(rgb(BORDER))
                            .px_5()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                self.selected_project
                                    .as_ref()
                                    .map(|project| project.display_name.clone())
                                    .unwrap_or_else(|| "未选择项目".into()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(MUTED))
                                    .child(self.status.clone()),
                            ),
                    )
                    .child(self.render_timeline())
                    .child(
                        div()
                            .flex_none()
                            .border_t_1()
                            .border_color(rgb(BORDER))
                            .p_4()
                            .flex()
                            .gap_3()
                            .items_end()
                            .child(
                                div()
                                    .flex_1()
                                    .child(Input::new(&self.prompt_input).h(px(94.))),
                            )
                            .child(
                                Button::new("submit")
                                    .label(if self.active_run.is_some() {
                                        "运行中"
                                    } else {
                                        "发送"
                                    })
                                    .primary()
                                    .disabled(!can_submit)
                                    .on_click(cx.listener(Self::submit)),
                            ),
                    ),
            )
            .child(self.render_settings(cx))
    }
}

fn render_message(message: &Message) -> impl IntoElement {
    let label = match message.role {
        MessageRole::User => "You",
        MessageRole::Assistant => "Claude",
        MessageRole::Tool => "Tool",
        MessageRole::System => "System",
    };
    message_card(label, &message.content, message.kind)
}

fn message_card(label: &str, content: &str, kind: MessageKind) -> impl IntoElement {
    let border = match kind {
        MessageKind::Error => rgb(DANGER),
        MessageKind::ToolCall | MessageKind::ToolResult => rgb(0x4d7caa),
        _ => rgb(BORDER),
    };
    div()
        .w_full()
        .rounded_lg()
        .border_1()
        .border_color(border)
        .bg(rgb(PANEL))
        .p_4()
        .child(
            div()
                .text_xs()
                .text_color(rgb(MUTED))
                .mb_2()
                .child(label.to_owned()),
        )
        .child(
            div()
                .text_sm()
                .whitespace_normal()
                .child(content.to_owned()),
        )
}

fn label_value(label: impl Into<SharedString>, value: impl Into<SharedString>) -> impl IntoElement {
    let label: SharedString = label.into();
    let value: SharedString = value.into();
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_xs().text_color(rgb(MUTED)).child(label))
        .child(div().text_sm().child(value))
}

fn status_color(status: RunStatus) -> gpui::Hsla {
    match status {
        RunStatus::Completed => rgb(SUCCESS).into(),
        RunStatus::Failed => rgb(DANGER).into(),
        RunStatus::Running | RunStatus::Starting | RunStatus::Cancelling => rgb(ACCENT).into(),
        _ => rgb(MUTED).into(),
    }
}

fn is_git_dirty(path: &Path) -> bool {
    SystemCommand::new("git")
        .args(["status", "--porcelain"])
        .current_dir(path)
        .output()
        .map(|output| output.status.success() && !output.stdout.is_empty())
        .unwrap_or(false)
}

fn main() {
    Application::new().run(|cx: &mut App| {
        gpui_component::init(cx);
        cx.spawn(async move |cx| {
            cx.open_window(WindowOptions::default(), |window, cx| {
                let view = cx.new(|cx| NexusApp::new(window, cx));
                cx.new(|cx| Root::new(view, window, cx))
            })?;
            Ok::<_, anyhow::Error>(())
        })
        .detach();
    });
}
