use nexus_domain::{HarnessKind, ModelDescriptor};
use nexus_harness_acp as acp;
use nexus_harness_claude as claude;
use nexus_harness_codex as codex;
use nexus_harness_core::{LaunchSpec, LineDecoder, ModelCatalogError};
use nexus_harness_omp as omp;
use nexus_harness_pi as pi;
use nexus_protocol::{EnvironmentVariable, HarnessProbe, StartRun, TextGenerationConfig};
use std::path::Path;
use tokio::sync::watch;

pub(crate) async fn probe(harness: HarnessKind, executable: &str) -> HarnessProbe {
    match harness {
        HarnessKind::Claude => claude::probe(executable).await,
        HarnessKind::Codex => codex::probe(executable).await,
        HarnessKind::Omp => omp::probe(executable).await,
        HarnessKind::Pi => pi::probe(executable).await,
        HarnessKind::Kimi | HarnessKind::Qoder | HarnessKind::Codebuddy => {
            acp::probe(harness, executable).await
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
        HarnessKind::Kimi | HarnessKind::Qoder | HarnessKind::Codebuddy => {
            acp::discover_models(harness, executable, cwd, environment, cancel).await
        }
        HarnessKind::Claude => claude::discover_models(executable, cwd, environment, cancel).await,
    }
}

pub(crate) fn prepare(
    request: &StartRun,
    cwd: &Path,
) -> Result<(LaunchSpec, Box<dyn LineDecoder>), String> {
    Ok(match request.harness {
        HarnessKind::Kimi | HarnessKind::Qoder | HarnessKind::Codebuddy => {
            let (spec, decoder) = acp::prepare_run(request, cwd);
            (spec, Box::new(decoder) as Box<dyn LineDecoder>)
        }
        HarnessKind::Pi => {
            let (spec, decoder) = pi::prepare_run(request, cwd)?;
            (spec, Box::new(decoder) as Box<dyn LineDecoder>)
        }
        HarnessKind::Claude => (
            claude::build_launch_spec(
                &request.executable,
                cwd,
                &request.prompt,
                request.model.as_deref(),
                request.effort,
                request.session_id.as_deref(),
                request.permission_mode,
            ),
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
        HarnessKind::Qoder | HarnessKind::Codebuddy => {
            let mut args: Vec<String> = [
                "--print",
                "--output-format",
                "stream-json",
                "--verbose",
                "--tools",
                "",
                "--strict-mcp-config",
                "--mcp-config",
                "{\"mcpServers\":{}}",
                "--no-session-persistence",
            ]
            .map(Into::into)
            .to_vec();
            if let Some(model) = &request.model {
                args.extend(["--model".into(), model.into()]);
            }
            if !request.effort.is_default()
                && !matches!(
                    request.effort,
                    nexus_domain::ThinkingEffort::Off | nexus_domain::ThinkingEffort::None
                )
            {
                args.extend([
                    if request.harness == HarnessKind::Qoder {
                        "--reasoning-effort"
                    } else {
                        "--effort"
                    }
                    .into(),
                    request.effort.as_str().into(),
                ]);
            }
            (
                LaunchSpec {
                    executable: request.executable.clone().into(),
                    cwd: cwd.into(),
                    args,
                    stdin: prompt.into(),
                },
                Box::new(claude::EventDecoder::default()),
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
