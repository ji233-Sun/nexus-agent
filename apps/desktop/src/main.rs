mod runner_client;
mod storage;

use std::{path::Path, process::Command as SystemCommand, str::FromStr as _, time::Duration};

use gpui::{
    App, AppContext as _, Application, Bounds, Context, Entity, Hsla, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Timer, Window, WindowBounds, WindowOptions, div, prelude::FluentBuilder as _, px,
    relative, rgb, rgba, size,
};
use gpui_component::{
    Disableable as _, Root, Sizable as _, Theme, ThemeMode, box_shadow,
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

const BG: u32 = 0xfafafa;
const SURFACE: u32 = 0xffffff;
const RECESSED: u32 = 0xf2f2f2;
const HOVER: u32 = 0xebebeb;
const TEXT: u32 = 0x171717;
const TEXT_SECONDARY: u32 = 0x4d4d4d;
const MUTED: u32 = 0x8f8f8f;
const ACCENT: u32 = 0x0072f5;
const ACCENT_HOVER: u32 = 0x0062d1;
const SUCCESS: u32 = 0x398e4a;
const WARNING: u32 = 0xff990a;
const DANGER: u32 = 0xe5484d;
const TOOL: u32 = 0x7820bc;

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
            .w(px(260.))
            .h_full()
            .flex_none()
            .bg(rgb(BG))
            .shadow(surface_border_shadow())
            .p_4()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .h(px(32.))
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
                                    .size(px(24.))
                                    .rounded(px(6.))
                                    .bg(rgb(TEXT))
                                    .text_color(rgb(SURFACE))
                                    .text_xs()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child("N"),
                            )
                            .child(
                                div()
                                    .text_base()
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .child("Nexus Agent"),
                            ),
                    )
                    .child(
                        Button::new("open-project")
                            .ghost()
                            .small()
                            .child(button_label("新项目", TEXT))
                            .on_click(cx.listener(Self::choose_project)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(section_label("项目"))
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
                                    .h(px(36.))
                                    .px_3()
                                    .rounded(px(6.))
                                    .cursor_pointer()
                                    .flex()
                                    .items_center()
                                    .text_sm()
                                    .text_color(rgb(TEXT_SECONDARY))
                                    .when(selected, |element| {
                                        element
                                            .bg(rgb(SURFACE))
                                            .text_color(rgb(TEXT))
                                            .shadow(surface_border_shadow())
                                    })
                                    .hover(|style| style.bg(rgb(HOVER)).text_color(rgb(TEXT)))
                                    .on_click(cx.listener(move |app, _, _, cx| {
                                        app.select_project(project.clone());
                                        cx.notify();
                                    }))
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
                    .gap_2()
                    .child(section_label("任务"))
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
                                    .p_3()
                                    .rounded(px(6.))
                                    .cursor_pointer()
                                    .text_color(rgb(TEXT_SECONDARY))
                                    .when(selected, |element| {
                                        element
                                            .bg(rgb(SURFACE))
                                            .text_color(rgb(TEXT))
                                            .shadow(surface_border_shadow())
                                    })
                                    .hover(|style| style.bg(rgb(HOVER)).text_color(rgb(TEXT)))
                                    .on_click(cx.listener(move |app, _, _, cx| {
                                        app.select_task(task_id, cx)
                                    }))
                                    .child(div().text_sm().line_clamp(2).child(task.title.clone()))
                                    .child(
                                        div()
                                            .mt_2()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .text_xs()
                                            .text_color(rgb(MUTED))
                                            .child(status_dot(run_status_color(task.status)))
                                            .child(task.status.to_string()),
                                    )
                            })),
                    ),
            )
    }

    fn render_timeline(&self) -> impl IntoElement {
        let empty = self.messages.is_empty() && self.streaming_text.is_empty();
        div()
            .id("timeline")
            .flex_1()
            .h_full()
            .overflow_y_scroll()
            .child(
                div()
                    .w_full()
                    .max_w(px(760.))
                    .min_h_full()
                    .mx_auto()
                    .p_6()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .when(empty, |element| {
                        let (title, description) = if self.selected_project.is_some() {
                            (
                                "准备开始新任务",
                                "在下方描述目标，Claude Code 的执行过程会显示在这里。",
                            )
                        } else {
                            (
                                "选择一个本地项目",
                                "Nexus Agent 会在所选目录中运行 Claude Code。",
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
                    .children(self.messages.iter().map(render_message))
                    .when(!self.streaming_text.is_empty(), |element| {
                        element.child(message_card(
                            "Claude · 正在生成",
                            &self.streaming_text,
                            MessageKind::Text,
                            rgb(ACCENT).into(),
                        ))
                    }),
            )
    }

    fn render_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let harness_color: Hsla = self
            .harness
            .as_ref()
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
            .w(px(288.))
            .h_full()
            .flex_none()
            .overflow_y_scroll()
            .bg(rgb(SURFACE))
            .shadow(surface_border_shadow())
            .p_6()
            .flex()
            .flex_col()
            .gap_6()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_base()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child("运行配置"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child("Claude Code 本地执行参数"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(section_label("HARNESS"))
                    .child(label_value("类型", "Claude Code"))
                    .child(div().text_xs().text_color(rgb(MUTED)).child("可执行文件"))
                    .child(Input::new(&self.executable_input))
                    .child(
                        Button::new("probe")
                            .outline()
                            .small()
                            .w_full()
                            .child(button_label("重新探测", TEXT))
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
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(section_label("MODEL"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child("模型 · 点击切换"),
                    )
                    .child(
                        Button::new("model")
                            .outline()
                            .w_full()
                            .child(button_label(self.model.to_string(), TEXT))
                            .on_click(cx.listener(Self::cycle_model)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child("思考层级 · 点击切换"),
                    )
                    .child(
                        Button::new("effort")
                            .outline()
                            .w_full()
                            .child(button_label(self.effort.to_string(), TEXT))
                            .on_click(cx.listener(Self::cycle_effort)),
                    ),
            )
            .when_some(self.selected_project.as_ref(), |element, project| {
                element
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let can_submit = self.selected_project.is_some()
            && self.active_run.is_none()
            && self
                .harness
                .as_ref()
                .is_some_and(|probe| probe.available && probe.authenticated);
        let header_status_color = if self.active_run.is_some() {
            rgb(ACCENT).into()
        } else {
            self.harness
                .as_ref()
                .map(|probe| {
                    if probe.available && probe.authenticated {
                        rgb(SUCCESS).into()
                    } else {
                        rgb(DANGER).into()
                    }
                })
                .unwrap_or_else(|| rgb(MUTED).into())
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
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .h(px(64.))
                            .flex_none()
                            .bg(rgb(BG))
                            .shadow(composer_shadow())
                            .px_6()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(
                                        div().font_weight(gpui::FontWeight::MEDIUM).child(
                                            self.selected_project
                                                .as_ref()
                                                .map(|project| project.display_name.clone())
                                                .unwrap_or_else(|| "未选择项目".into()),
                                        ),
                                    )
                                    .when_some(self.selected_task, |element, _| {
                                        element.child(
                                            div()
                                                .text_xs()
                                                .text_color(rgb(MUTED))
                                                .child("任务时间线"),
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
                                        div().max_w(px(420.)).truncate().child(self.status.clone()),
                                    ),
                            ),
                    )
                    .child(self.render_timeline())
                    .child(
                        div()
                            .flex_none()
                            .bg(rgb(BG))
                            .shadow(header_shadow())
                            .px_6()
                            .py_4()
                            .child(
                                div()
                                    .w_full()
                                    .max_w(px(760.))
                                    .mx_auto()
                                    .flex()
                                    .gap_3()
                                    .items_end()
                                    .child(
                                        div()
                                            .flex_1()
                                            .child(Input::new(&self.prompt_input).h(px(96.))),
                                    )
                                    .child(
                                        Button::new("submit")
                                            .primary()
                                            .h(px(40.))
                                            .px_5()
                                            .child(button_label(
                                                if self.active_run.is_some() {
                                                    "运行中"
                                                } else {
                                                    "发送"
                                                },
                                                SURFACE,
                                            ))
                                            .when(!can_submit, |button| button.opacity(0.5))
                                            .disabled(!can_submit)
                                            .on_click(cx.listener(Self::submit)),
                                    ),
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
    let indicator = match message.kind {
        MessageKind::Error => rgb(DANGER).into(),
        MessageKind::ToolCall | MessageKind::ToolResult => rgb(TOOL).into(),
        _ => rgb(MUTED).into(),
    };
    message_card(label, &message.content, message.kind, indicator)
}

fn message_card(
    label: &str,
    content: &str,
    kind: MessageKind,
    indicator: Hsla,
) -> impl IntoElement {
    div()
        .w_full()
        .rounded(px(12.))
        .bg(rgb(SURFACE))
        .shadow(surface_card_shadow())
        .p_6()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .text_xs()
                .text_color(rgb(TEXT_SECONDARY))
                .font_weight(gpui::FontWeight::MEDIUM)
                .mb_3()
                .child(status_dot(indicator))
                .child(label.to_owned()),
        )
        .child(
            div()
                .text_sm()
                .text_color(rgb(TEXT))
                .whitespace_normal()
                .line_height(relative(1.5))
                .when(
                    matches!(kind, MessageKind::ToolCall | MessageKind::ToolResult),
                    |element| element.font_family("SF Mono"),
                )
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
        .child(
            div()
                .text_sm()
                .text_color(rgb(TEXT_SECONDARY))
                .whitespace_normal()
                .child(value),
        )
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
    vec![box_shadow(0., 0., 0., 1., rgba(0x00000014).into())]
}

fn surface_card_shadow() -> Vec<gpui::BoxShadow> {
    vec![
        box_shadow(0., 0., 0., 1., rgba(0x00000014).into()),
        box_shadow(0., 2., 2., 0., rgba(0x0000000a).into()),
        box_shadow(0., 8., 8., -8., rgba(0x0000000a).into()),
    ]
}

fn header_shadow() -> Vec<gpui::BoxShadow> {
    vec![box_shadow(0., 1., 0., 0., rgba(0x0000001a).into())]
}

fn composer_shadow() -> Vec<gpui::BoxShadow> {
    vec![box_shadow(0., -1., 0., 0., rgba(0x0000001a).into())]
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
    theme.shadow = true;

    theme.background = rgb(BG).into();
    theme.foreground = rgb(TEXT).into();
    theme.border = rgba(0x00000014).into();
    theme.input = rgba(0x00000024).into();
    theme.caret = rgb(TEXT).into();
    theme.ring = rgb(0x005fcc).into();
    theme.selection = rgba(0x0072f533).into();
    theme.muted = rgb(RECESSED).into();
    theme.muted_foreground = rgb(MUTED).into();
    theme.accent = rgb(HOVER).into();
    theme.accent_foreground = rgb(TEXT).into();
    theme.primary = rgb(ACCENT).into();
    theme.primary_hover = rgb(ACCENT_HOVER).into();
    theme.primary_active = rgb(0x005fcc).into();
    theme.primary_foreground = rgb(SURFACE).into();
    theme.secondary = rgb(SURFACE).into();
    theme.secondary_hover = rgb(HOVER).into();
    theme.secondary_active = rgb(RECESSED).into();
    theme.secondary_foreground = rgb(TEXT_SECONDARY).into();
    theme.link = rgb(ACCENT).into();
    theme.link_hover = rgb(ACCENT_HOVER).into();
    theme.link_active = rgb(0x005fcc).into();
    theme.danger = rgb(DANGER).into();
    theme.danger_hover = rgb(0xc93439).into();
    theme.danger_active = rgb(0xa9272c).into();
    theme.danger_foreground = rgb(SURFACE).into();
    theme.sidebar = rgb(BG).into();
    theme.sidebar_foreground = rgb(TEXT).into();
    theme.sidebar_accent = rgb(HOVER).into();
    theme.sidebar_accent_foreground = rgb(TEXT).into();
    theme.sidebar_border = rgba(0x00000014).into();
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
}
