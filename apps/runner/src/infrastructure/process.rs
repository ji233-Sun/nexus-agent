use super::process_tree;
use nexus_harness_core::LaunchSpec;
use nexus_protocol::EnvironmentVariable;
use tokio::process::Command as ProcessCommand;

pub(crate) fn command(spec: &LaunchSpec, environment: &[EnvironmentVariable]) -> ProcessCommand {
    let mut command = ProcessCommand::new(&spec.executable);
    command
        .args(&spec.args)
        .envs(
            environment
                .iter()
                .map(|variable| (&variable.name, &variable.value)),
        )
        .current_dir(&spec.cwd)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    process_tree::configure(&mut command);
    command
}
