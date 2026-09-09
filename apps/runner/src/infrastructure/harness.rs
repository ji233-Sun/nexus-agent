mod additional;
use nexus_domain::{HarnessKind, ModelDescriptor};
use nexus_harness_claude as claude;
use nexus_harness_codex as codex;
use nexus_harness_core::{LaunchSpec, LineDecoder, ModelCatalogError};
use nexus_harness_omp as omp;
use nexus_protocol::{EnvironmentVariable, HarnessProbe, StartRun};
use std::path::Path;
use tokio::sync::watch;

pub(crate) async fn probe(harness: HarnessKind, executable: &str) -> HarnessProbe {
    match harness {
        HarnessKind::Claude => claude::probe(executable).await,
        HarnessKind::Codex => codex::probe(executable).await,
        HarnessKind::Omp => omp::probe(executable).await,
        _ => additional::probe(harness, executable).await,
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
        HarnessKind::Claude => claude::discover_models(executable, cwd, environment, cancel).await,
        _ => {
            if *cancel.borrow() {
                Err(ModelCatalogError::Cancelled)
            } else {
                Ok(Vec::new())
            }
        }
    }
}

pub(crate) fn prepare(
    request: &StartRun,
    cwd: &Path,
) -> anyhow::Result<(LaunchSpec, Box<dyn LineDecoder>)> {
    Ok(match request.harness {
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
            Box::new(claude::EventDecoder) as Box<dyn LineDecoder>,
        ),
        HarnessKind::Codex => {
            let (spec, decoder) = codex::prepare_run(request, cwd);
            (spec, Box::new(decoder) as Box<dyn LineDecoder>)
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
            Box::new(omp::EventDecoder) as Box<dyn LineDecoder>,
        ),
        _ => additional::prepare(request, cwd, None)?,
    })
}

pub(crate) fn prepare_title(
    request: &StartRun,
    cwd: &Path,
    prompt: &str,
) -> anyhow::Result<(LaunchSpec, Box<dyn LineDecoder>)> {
    Ok(match request.harness {
        HarnessKind::Claude => (
            claude::build_title_launch_spec(
                &request.executable,
                cwd,
                prompt,
                request.model.as_deref(),
                request.effort,
            ),
            Box::new(claude::EventDecoder) as Box<dyn LineDecoder>,
        ),
        HarnessKind::Codex => (
            codex::build_title_launch_spec(
                &request.executable,
                cwd,
                prompt,
                request.model.as_deref(),
                request.effort,
            ),
            Box::new(codex::TitleEventDecoder) as Box<dyn LineDecoder>,
        ),
        HarnessKind::Omp => (
            omp::build_title_launch_spec(
                &request.executable,
                cwd,
                prompt,
                request.model.as_deref(),
                request.effort,
            ),
            Box::new(omp::EventDecoder) as Box<dyn LineDecoder>,
        ),
        _ => additional::prepare(request, cwd, Some(prompt))?,
    })
}
