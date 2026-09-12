use nexus_domain::{HarnessKind, HarnessTransport, ModelDescriptor, UserAskAnswer};
use nexus_harness_acp as acp;
use nexus_harness_claude as claude;
use nexus_harness_codex as codex;
use nexus_harness_core::{DecodedEvent, InputFrame, LaunchSpec, LineDecoder, ModelCatalogError};
use nexus_harness_omp as omp;
use nexus_harness_pi as pi;
use nexus_protocol::{EnvironmentVariable, HarnessProbe, StartRun, TextGenerationConfig};
use serde_json::{Value, json};
use std::path::Path;
use tokio::sync::watch;

pub(crate) async fn probe(
    harness: HarnessKind,
    executable: &str,
    environment: &[EnvironmentVariable],
) -> HarnessProbe {
    match harness {
        HarnessKind::Claude => claude::probe(executable).await,
        HarnessKind::Codex => codex::probe(executable).await,
        HarnessKind::Omp => omp::probe(executable).await,
        HarnessKind::Pi => pi::probe(executable).await,
        HarnessKind::Kimi | HarnessKind::Qoder | HarnessKind::QoderCn | HarnessKind::Codebuddy => {
            acp::probe(harness, executable, environment).await
        }
    }
}

pub(crate) async fn discover_models(
    harness: HarnessKind,
    executable: &str,
    cwd: &Path,
    environment: &[EnvironmentVariable],
    cancel: watch::Receiver<bool>,
) -> Result<Vec<ModelDescriptor>, ModelCatalogError> {
    match harness {
        HarnessKind::Codex => codex::discover_models(executable, cwd, environment, cancel).await,
        HarnessKind::Omp => omp::discover_models(executable, cwd, environment, cancel).await,
        HarnessKind::Pi => pi::discover_models(executable, cwd, environment, cancel).await,
        HarnessKind::Kimi | HarnessKind::Qoder | HarnessKind::QoderCn | HarnessKind::Codebuddy => {
            acp::discover_models(harness, executable, cwd, environment, cancel).await
        }
        HarnessKind::Claude => claude::discover_models(executable, cwd, environment, cancel).await,
    }
}

fn configurable(harness: HarnessKind) -> bool {
    matches!(
        harness,
        HarnessKind::Kimi | HarnessKind::Qoder | HarnessKind::QoderCn | HarnessKind::Codebuddy
    )
}

// Existing untagged sessions were created through ACP. New sessions carry their
// transport and explicit region so changing preferences cannot redirect a resume.
pub(crate) fn restore_session_settings(request: &mut StartRun) -> Result<(), String> {
    if !configurable(request.harness) {
        return Ok(());
    }
    let Some(saved) = request.session_id.clone() else {
        return Ok(());
    };
    if let Some(encoded) = saved.strip_prefix("nexus:v1:") {
        let value: Value = serde_json::from_str(encoded).map_err(|_| "无法读取原生会话配置。")?;
        request.transport = serde_json::from_value(value["transport"].clone())
            .map_err(|_| "无法读取会话接入方式。")?;
        request.session_id = Some(
            value["session"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or("缺少原生会话 ID。")?
                .into(),
        );
        if request.harness == HarnessKind::Codebuddy {
            request
                .environment
                .retain(|v| v.name != "CODEBUDDY_INTERNET_ENVIRONMENT");
            if let Some(region) = value["region"]
                .as_str()
                .filter(|v| matches!(*v, "internal" | "external"))
            {
                request.environment.push(EnvironmentVariable {
                    name: "CODEBUDDY_INTERNET_ENVIRONMENT".into(),
                    value: region.into(),
                });
            }
        }
    } else {
        request.transport = HarnessTransport::Acp;
    }
    // Sessions recorded before Kimi Code dropped the Wire protocol are resumed
    // over ACP; the stored native session id may no longer be loadable.
    if request.harness == HarnessKind::Kimi {
        request.transport = HarnessTransport::Acp;
    }
    Ok(())
}

pub(crate) fn prepare(
    request: &StartRun,
    cwd: &Path,
) -> Result<(LaunchSpec, Box<dyn LineDecoder>), String> {
    if !request.attachments.is_empty() {
        if !nexus_domain::ImageAttachment::supported_by(request.harness) {
            return Err("此 Harness 暂不支持截图输入，请选择 Codex 或 Claude Code。".into());
        }
        if request.attachments.len() > nexus_domain::ImageAttachment::MAX_COUNT {
            return Err("每条消息最多包含 8 张截图。".into());
        }
        for image in &request.attachments {
            nexus_harness_core::read_image_attachment(image)?;
        }
    }
    let (spec, decoder) = prepare_native(request, cwd)?;
    if !configurable(request.harness) {
        return Ok((spec, decoder));
    }
    let region = request
        .environment
        .iter()
        .find(|v| v.name == "CODEBUDDY_INTERNET_ENVIRONMENT")
        .map(|v| v.value.clone());
    Ok((
        spec,
        Box::new(SessionDecoder {
            inner: decoder,
            transport: request.transport,
            region,
        }),
    ))
}
struct SessionDecoder {
    inner: Box<dyn LineDecoder>,
    transport: HarnessTransport,
    region: Option<String>,
}
impl LineDecoder for SessionDecoder {
    fn decode_line(&mut self, line: &str) -> Result<Vec<DecodedEvent>, serde_json::Error> {
        let mut events = self.inner.decode_line(line)?;
        for event in &mut events {
            if let DecodedEvent::SessionStarted(session) = event {
                *session = format!(
                    "nexus:v1:{}",
                    json!({"session":session,"transport":self.transport,"region":self.region})
                );
            }
        }
        Ok(events)
    }
    fn steer(&mut self, id: &str, prompt: &str) -> Option<InputFrame> {
        self.inner.steer(id, prompt)
    }
    fn answer_user_ask(&mut self, id: &str, answers: &[UserAskAnswer]) -> Option<InputFrame> {
        self.inner.answer_user_ask(id, answers)
    }
}

fn prepare_native(
    request: &StartRun,
    cwd: &Path,
) -> Result<(LaunchSpec, Box<dyn LineDecoder>), String> {
    Ok(match request.harness {
        // Kimi Code no longer ships the Wire protocol; ACP is its only transport.
        HarnessKind::Kimi => {
            let (spec, decoder) = acp::prepare_run(request, cwd);
            (spec, Box::new(decoder) as Box<dyn LineDecoder>)
        }
        HarnessKind::Qoder | HarnessKind::QoderCn | HarnessKind::Codebuddy => {
            if request.transport == HarnessTransport::Cli {
                nexus_harness_cli::prepare_run(request, cwd)
            } else {
                let (spec, decoder) = acp::prepare_run(request, cwd);
                (spec, Box::new(decoder) as Box<dyn LineDecoder>)
            }
        }
        HarnessKind::Pi => {
            let (spec, decoder) = pi::prepare_run(request, cwd)?;
            (spec, Box::new(decoder) as Box<dyn LineDecoder>)
        }
        HarnessKind::Claude => (
            claude::prepare_run(request, cwd)?,
            Box::new(claude::EventDecoder::default()),
        ),
        HarnessKind::Codex => {
            let (spec, decoder) = codex::prepare_run(request, cwd);
            (spec, Box::new(decoder))
        }
        HarnessKind::Omp => (
            omp::build_launch_spec(
                &request.executable,
                cwd,
                &request.prompt,
                request.model.as_deref(),
                request.effort,
                request.session_id.as_deref(),
                request.permission_mode,
            ),
            Box::new(omp::EventDecoder::default()),
        ),
    })
}

pub(crate) fn prepare_text_generation(
    request: &TextGenerationConfig,
    cwd: &Path,
    prompt: &str,
) -> Result<(LaunchSpec, Box<dyn LineDecoder>), String> {
    Ok(match request.harness {
        HarnessKind::Kimi => {
            let (spec, decoder) = acp::prepare_kimi_text_generation(request, prompt, cwd)?;
            (spec, Box::new(decoder) as Box<dyn LineDecoder>)
        }
        HarnessKind::Qoder | HarnessKind::QoderCn | HarnessKind::Codebuddy => {
            let mut args: Vec<String> = [
                "--print",
                "--output-format",
                "stream-json",
                "--tools",
                "",
                "--strict-mcp-config",
                "--mcp-config",
                "{\"mcpServers\":{}}",
                "--no-session-persistence",
            ]
            .map(Into::into)
            .to_vec();
            if request.harness == HarnessKind::Codebuddy {
                args.push("--verbose".into());
            }
            if let Some(model) = &request.model {
                args.extend(["--model".into(), model.into()]);
            }
            nexus_harness_cli::append_effort_args(&mut args, request.harness, request.effort);
            (
                LaunchSpec {
                    executable: request.executable.clone().into(),
                    cwd: cwd.into(),
                    args,
                    stdin: prompt.into(),
                },
                Box::new(claude::EventDecoder::for_harness(request.harness)),
            )
        }
        HarnessKind::Pi => {
            let (spec, decoder) = pi::prepare_title(request, cwd, prompt);
            (spec, Box::new(decoder))
        }
        HarnessKind::Claude => (
            claude::build_title_launch_spec(
                &request.executable,
                cwd,
                prompt,
                request.model.as_deref(),
                request.effort,
            ),
            Box::new(claude::EventDecoder::default()),
        ),
        HarnessKind::Codex => (
            codex::build_title_launch_spec(
                &request.executable,
                cwd,
                prompt,
                request.model.as_deref(),
                request.effort,
            ),
            Box::new(codex::TitleEventDecoder),
        ),
        HarnessKind::Omp => (
            omp::build_title_launch_spec(
                &request.executable,
                cwd,
                prompt,
                request.model.as_deref(),
                request.effort,
            ),
            Box::new(omp::EventDecoder::default()),
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn run(harness: HarnessKind) -> StartRun {
        serde_json::from_value(json!({"run_id":uuid::Uuid::new_v4(),"task_id":uuid::Uuid::new_v4(),
            "cwd":"/tmp","prompt":"hello","permission_mode":"ask","effort":"default","harness":harness,"executable":harness.default_executable()})).unwrap()
    }
    #[test]
    fn captured_images_reach_claude_and_unsupported_harnesses_fail_explicitly() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("capture.png");
        let png = [
            137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1,
            8, 4, 0, 0, 0, 181, 28, 12, 2, 0, 0, 0, 11, 73, 68, 65, 84, 120, 218, 99, 100, 248, 15,
            0, 1, 5, 1, 1, 39, 24, 227, 102, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
        ];
        std::fs::write(&path, png).unwrap();
        let mut request = run(HarnessKind::Claude);
        request.attachments = vec![nexus_domain::ImageAttachment {
            path: path.canonicalize().unwrap().to_string_lossy().into_owned(),
            source_name: "report.pdf".into(),
            page: 2,
        }];
        for session in [None, Some("existing-session".into())] {
            request.session_id = session;
            let (spec, _) = prepare(&request, directory.path()).unwrap();
            let frame: serde_json::Value = serde_json::from_str(&spec.stdin).unwrap();
            assert_eq!(
                frame["message"]["content"][2]["source"]["media_type"],
                "image/png"
            );
            assert_eq!(frame["message"]["content"][2]["source"]["type"], "base64");
            assert!(
                frame["message"]["content"][2]["source"]["data"]
                    .as_str()
                    .unwrap()
                    .starts_with("iVBORw0KGgo")
            );
        }
        request.harness = HarnessKind::Omp;
        assert!(prepare(&request, directory.path()).is_err());
        request.harness = HarnessKind::Codex;
        std::fs::write(path, b"not a screenshot").unwrap();
        assert!(prepare(&request, directory.path()).is_err());
    }

    #[test]
    fn resume_pins_transport_and_region_and_preserves_legacy_acp() {
        for transport in [HarnessTransport::Cli, HarnessTransport::Acp] {
            let mut request = run(HarnessKind::Codebuddy);
            request.environment = vec![EnvironmentVariable {
                name: "CODEBUDDY_INTERNET_ENVIRONMENT".into(),
                value: "external".into(),
            }];
            request.session_id = Some(format!(
                "nexus:v1:{}",
                json!({"session":"saved","transport":transport,"region":"internal"})
            ));
            restore_session_settings(&mut request).unwrap();
            assert_eq!(request.transport, transport);
            assert_eq!(request.session_id.as_deref(), Some("saved"));
            assert_eq!(request.environment[0].value, "internal");
        }
        for harness in [
            HarnessKind::Kimi,
            HarnessKind::Qoder,
            HarnessKind::QoderCn,
            HarnessKind::Codebuddy,
        ] {
            let mut request = run(harness);
            assert_eq!(request.transport, HarnessTransport::Cli);
            request.session_id = Some("legacy".into());
            restore_session_settings(&mut request).unwrap();
            assert_eq!(request.transport, HarnessTransport::Acp);
            assert_eq!(request.session_id.as_deref(), Some("legacy"));
            request.session_id = Some("nexus:v1:broken".into());
            assert!(restore_session_settings(&mut request).is_err());
        }
    }
}
