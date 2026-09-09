use nexus_domain::{
    HarnessKind, ModelAvailability, ModelDescriptor, ModelReasoningEffort, ModelSource,
    ThinkingEffort, UserAskAnswer,
};
use nexus_harness_core::{
    DecodedEvent, InputFrame, LaunchSpec, LineDecoder, ModelCatalogError, resolve_executable,
    rpc::RpcEventDecoder,
};
use nexus_protocol::{EnvironmentVariable, HarnessProbe, StartRun, TextGenerationConfig};
use serde_json::{Value, json};
use std::{
    io::Write as _,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tempfile::NamedTempFile;
use tokio::{
    io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader},
    process::Command,
    sync::watch,
    time::timeout,
};

const TIMEOUT: Duration = Duration::from_secs(30);

fn model_args(args: &mut Vec<String>, model: Option<&str>, effort: ThinkingEffort) {
    if let Some(model) = model {
        args.extend(["--model".into(), model.into()]);
    }
    if !effort.is_default() {
        args.extend([
            "--thinking".into(),
            if effort == ThinkingEffort::None {
                "off"
            } else {
                effort.as_str()
            }
            .into(),
        ]);
    }
}

pub fn prepare_run(run: &StartRun, cwd: &Path) -> Result<(LaunchSpec, EventDecoder), String> {
    let mut guard = tempfile::Builder::new()
        .prefix("nexus-pi-permissions-")
        .suffix(".mjs")
        .tempfile()
        .map_err(|_| "无法创建 Pi 审批扩展。")?;
    writeln!(
        guard,
        "const nexusPermission = {:?};\n{}",
        run.permission_mode.as_str(),
        include_str!("permissions.js")
    )
    .map_err(|_| "无法写入 Pi 审批扩展。")?;
    let mut args = vec![
        "--mode".into(),
        "rpc".into(),
        "--extension".into(),
        guard.path().to_string_lossy().into(),
    ];
    model_args(&mut args, run.model.as_deref(), run.effort);
    if let Some(session) = &run.session_id {
        args.extend(["--session".into(), session.into()]);
    }
    Ok((
        LaunchSpec {
            executable: run.executable.clone().into(),
            cwd: cwd.into(),
            args,
            stdin: format!("{}\n", json!({"type":"get_state","id":"nexus-session"})),
        },
        EventDecoder {
            inner: RpcEventDecoder::new("Pi"),
            _guard: guard,
            ready: false,
            prompt: Some(run.prompt.clone()),
        },
    ))
}

pub struct EventDecoder {
    inner: RpcEventDecoder,
    _guard: NamedTempFile,
    ready: bool,
    prompt: Option<String>,
}

impl LineDecoder for EventDecoder {
    fn decode_line(&mut self, line: &str) -> Result<Vec<DecodedEvent>, serde_json::Error> {
        let frame: Value = serde_json::from_str(line)?;
        if frame["type"] == "extension_ui_request"
            && frame["method"] == "notify"
            && frame["message"] == "nexus-permissions-ready"
        {
            self.ready = true;
            return Ok(vec![]);
        }
        if frame["type"] == "response" && frame["id"] == "nexus-session" && frame["success"] == true
        {
            if !self.ready {
                return Ok(vec![
                    DecodedEvent::Error("Pi 审批扩展未加载，已阻止运行。".into()),
                    DecodedEvent::TurnCompleted,
                ]);
            }
            let Some(file) = frame["data"]["sessionFile"]
                .as_str()
                .filter(|s| !s.is_empty())
            else {
                return Ok(vec![
                    DecodedEvent::Error("Pi 未返回持久化会话路径。".into()),
                    DecodedEvent::TurnCompleted,
                ]);
            };
            let Some(prompt) = self.prompt.take() else {
                return Ok(vec![]);
            };
            return Ok(vec![
                DecodedEvent::SessionStarted(file.into()),
                DecodedEvent::WriteStdin(InputFrame(
                    json!({"type":"prompt","id":"nexus-prompt","message":prompt}),
                )),
            ]);
        }
        self.inner.decode_line(line)
    }
    fn steer(&mut self, id: &str, prompt: &str) -> Option<InputFrame> {
        self.inner.steer(id, prompt)
    }
    fn answer_user_ask(&mut self, id: &str, answers: &[UserAskAnswer]) -> Option<InputFrame> {
        self.inner.answer_user_ask(id, answers)
    }
}

pub fn prepare_title(
    run: &TextGenerationConfig,
    cwd: &Path,
    prompt: &str,
) -> (LaunchSpec, RpcEventDecoder) {
    let mut args = [
        "--mode",
        "json",
        "--print",
        "--no-session",
        "--no-tools",
        "--no-extensions",
        "--no-skills",
        "--no-prompt-templates",
    ]
    .map(Into::into)
    .to_vec();
    model_args(&mut args, run.model.as_deref(), run.effort);
    (
        LaunchSpec {
            executable: run.executable.clone().into(),
            cwd: cwd.into(),
            args,
            stdin: prompt.into(),
        },
        RpcEventDecoder::new("Pi"),
    )
}

pub async fn discover_models(
    executable: &str,
    cwd: &Path,
    environment: &[EnvironmentVariable],
    mut cancel: watch::Receiver<bool>,
) -> Result<Vec<ModelDescriptor>, ModelCatalogError> {
    if *cancel.borrow() {
        return Err(ModelCatalogError::Cancelled);
    }
    let executable = resolve_executable(executable).ok_or_else(|| {
        ModelCatalogError::Failed("未找到 Pi，请安装官方 pi-coding-agent。".into())
    })?;
    let mut child = Command::new(executable)
        .args([
            "--mode",
            "rpc",
            "--no-session",
            "--no-extensions",
            "--no-skills",
            "--no-prompt-templates",
        ])
        .envs(environment.iter().map(|v| (&v.name, &v.value)))
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| ModelCatalogError::Failed("无法启动 Pi 模型目录。".into()))?;
    let mut stdin = child.stdin.take().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let collect = async {
        stdin
            .write_all(b"{\"type\":\"get_available_models\",\"id\":\"catalog\"}\n")
            .await
            .map_err(|_| "无法请求 Pi 模型目录。".to_owned())?;
        while let Some(line) = lines
            .next_line()
            .await
            .map_err(|_| "无法读取 Pi 模型目录。".to_owned())?
        {
            let Ok(frame) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if frame["type"] == "response" && frame["id"] == "catalog" {
                if frame["success"] != true {
                    return Err("Pi 模型目录加载失败，请检查 CLI 配置。".into());
                }
                return parse_catalog(&frame["data"]);
            }
        }
        Err("Pi 未返回模型目录。".into())
    };
    let result = tokio::select! {result=timeout(TIMEOUT,collect)=>result.unwrap_or_else(|_|Err("Pi 模型目录超时。".into())).map_err(ModelCatalogError::Failed),_=cancel.changed()=>Err(ModelCatalogError::Cancelled)};
    drop(stdin);
    if timeout(Duration::from_secs(2), child.wait()).await.is_err() {
        let _ = child.kill().await;
    }
    result
}

fn parse_catalog(data: &Value) -> Result<Vec<ModelDescriptor>, String> {
    data["models"]
        .as_array()
        .ok_or("Pi 模型目录缺少 models。")?
        .iter()
        .map(|model| {
            let provider = model["provider"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or("Pi 模型缺少 Provider。")?;
            let id = model["id"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or("Pi 模型缺少 ID。")?;
            let efforts = if model["reasoning"] == true {
                vec![
                    ThinkingEffort::Off,
                    ThinkingEffort::Minimal,
                    ThinkingEffort::Low,
                    ThinkingEffort::Medium,
                    ThinkingEffort::High,
                ]
            } else {
                vec![]
            };
            Ok(ModelDescriptor {
                id: format!("{provider}/{id}"),
                display_name: model["name"].as_str().unwrap_or(id).into(),
                source: ModelSource::PiRpc,
                availability: ModelAvailability::Available,
                provider: Some(provider.into()),
                is_default: false,
                supported_reasoning_efforts: efforts
                    .into_iter()
                    .map(|effort| ModelReasoningEffort {
                        effort,
                        description: String::new(),
                    })
                    .collect(),
                default_reasoning_effort: None,
            })
        })
        .collect()
}

pub async fn probe(executable: &str) -> HarnessProbe {
    let mut probe = HarnessProbe {
        harness: HarnessKind::Pi,
        executable: executable.into(),
        available: false,
        authenticated: false,
        version: None,
        message: "未找到 Pi CLI。".into(),
    };
    let Some(path) = resolve_executable(executable) else {
        return probe;
    };
    probe.executable = path.to_string_lossy().into();
    if let Ok(Ok(output)) = timeout(
        TIMEOUT,
        Command::new(path)
            .arg("--version")
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
        && output.status.success()
    {
        probe.available = true;
        probe.version = Some(String::from_utf8_lossy(&output.stdout).trim().into());
        let (_sender, cancel) = watch::channel(false);
        probe.authenticated = discover_models(
            &probe.executable,
            &std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            &[],
            cancel,
        )
        .await
        .is_ok_and(|models| !models.is_empty());
        probe.message = if probe.authenticated {
            "Pi 已配置可用模型，实际调用取决于 Provider 认证。"
        } else {
            "请运行 pi 完成模型与认证配置。"
        }
        .into();
    }
    probe
}

#[cfg(test)]
mod tests {
    use super::*;
    fn run() -> StartRun {
        serde_json::from_value(json!({"run_id":"00000000-0000-0000-0000-000000000001","task_id":"00000000-0000-0000-0000-000000000002","harness":"pi","executable":"pi","cwd":"/project","prompt":"private prompt","effort":"default","permission_mode":"ask"})).unwrap()
    }

    #[test]
    fn permission_extension_must_be_ready_before_prompt_and_session_uses_native_file() {
        let run = run();
        let (spec, mut decoder) = prepare_run(&run, Path::new("/project")).unwrap();
        assert!(!spec.stdin.contains("private prompt"));
        let state=json!({"type":"response","id":"nexus-session","command":"get_state","success":true,"data":{"sessionId":"uuid","sessionFile":"/sessions/thread.jsonl"}}).to_string();
        assert!(matches!(
            decoder.decode_line(&state).unwrap().as_slice(),
            [DecodedEvent::Error(_), DecodedEvent::TurnCompleted]
        ));
        decoder.decode_line(r#"{"type":"extension_ui_request","method":"notify","message":"nexus-permissions-ready"}"#).unwrap();
        let events = decoder.decode_line(&state).unwrap();
        assert!(
            matches!(&events[0],DecodedEvent::SessionStarted(id) if id=="/sessions/thread.jsonl")
        );
        assert!(
            matches!(&events[1],DecodedEvent::WriteStdin(frame) if frame.0["message"]=="private prompt")
        );
        let (_, mut decoder) = prepare_run(&run, Path::new("/project")).unwrap();
        let events=decoder.decode_line(r#"{"type":"extension_ui_request","id":"approve","method":"select","title":"Allow tool: bash","options":["Approve","Deny"]}"#).unwrap();
        assert!(
            matches!(&events[0],DecodedEvent::ApprovalRequested(prompt) if prompt.title=="Pi" && prompt.cancel.0["cancelled"]==true)
        );
    }

    #[test]
    fn resume_and_title_preserve_native_options_without_disabling_normal_session_storage() {
        let mut run = run();
        run.session_id = Some("/sessions/thread.jsonl".into());
        run.model = Some("provider/model".into());
        run.effort = ThinkingEffort::High;
        let (spec, _decoder) = prepare_run(&run, Path::new("/project")).unwrap();
        assert!(
            spec.args
                .windows(2)
                .any(|p| p == ["--session", "/sessions/thread.jsonl"])
        );
        assert!(!spec.args.contains(&"--no-session".into()));
        let (title, _) = prepare_title(
            &TextGenerationConfig {
                harness: run.harness,
                executable: run.executable.clone(),
                model: run.model.clone(),
                effort: run.effort,
                environment: vec![],
            },
            Path::new("/project"),
            "title",
        );
        for flag in [
            "--no-session",
            "--no-tools",
            "--no-extensions",
            "--no-skills",
        ] {
            assert!(title.args.contains(&flag.into()));
        }
        assert!(!title.args.contains(&"--session".into()));
        assert_eq!(title.stdin, "title");
    }

    #[test]
    fn catalog_preserves_provider_identity_without_exposing_connection_credentials() {
        let models=parse_catalog(&json!({"models":[{"id":"org/model","provider":"local","name":"Local","reasoning":true,"apiKey":"secret"}]})).unwrap();
        assert_eq!(models[0].id, "local/org/model");
        assert_eq!(models[0].source, ModelSource::PiRpc);
        assert!(!format!("{models:?}").contains("secret"));
        assert!(parse_catalog(&json!({"models":[{}]})).is_err());
    }
}
