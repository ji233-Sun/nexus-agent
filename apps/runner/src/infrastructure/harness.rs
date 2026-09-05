use nexus_domain::HarnessKind;
use nexus_harness_claude as claude;
use nexus_harness_codex as codex;
use nexus_harness_core::{LaunchSpec, LineDecoder};
use nexus_protocol::{HarnessProbe, StartRun};
use std::path::Path;

pub(crate) async fn probe(harness: HarnessKind, executable: &str) -> HarnessProbe {
    match harness {
        HarnessKind::Claude => claude::probe(executable).await,
        HarnessKind::Codex => codex::probe(executable).await,
    }
}

pub(crate) fn prepare(request: &StartRun, cwd: &Path) -> (LaunchSpec, Box<dyn LineDecoder>) {
    match request.harness {
        HarnessKind::Claude => (
            claude::build_launch_spec(
                &request.executable,
                cwd,
                &request.prompt,
                request.model.as_deref(),
                request.effort,
            ),
            Box::new(claude::EventDecoder),
        ),
        HarnessKind::Codex => (
            codex::build_launch_spec(
                &request.executable,
                cwd,
                &request.prompt,
                request.model.as_deref(),
                request.effort,
            ),
            Box::new(codex::EventDecoder),
        ),
    }
}
