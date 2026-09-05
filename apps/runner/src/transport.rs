use crate::application::{Runner, events::Emitter};
use anyhow::{Context as _, Result};
use nexus_protocol::{CommandEnvelope, EventEnvelope, PROTOCOL_VERSION};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt as _, AsyncWrite, AsyncWriteExt as _},
    sync::mpsc,
};

pub(crate) async fn serve(
    input: impl AsyncBufRead + Unpin,
    output: impl AsyncWrite + Unpin,
) -> Result<()> {
    let (emitter, events) = Emitter::channel();
    let commands = async move {
        let mut runner = Runner::new(emitter);
        let result = read_commands(input, &mut runner).await;
        runner.shutdown().await;
        result
    };
    let (commands, writer) = tokio::join!(commands, write_events(output, events));
    commands?;
    writer
}

async fn read_commands(input: impl AsyncBufRead + Unpin, runner: &mut Runner) -> Result<()> {
    let mut lines = input.lines();
    while let Some(line) = lines.next_line().await.context("read command")? {
        let command: CommandEnvelope = match serde_json::from_str(&line) {
            Ok(command) => command,
            Err(error) => {
                eprintln!("invalid command frame: {error}");
                continue;
            }
        };
        if command.protocol_version != PROTOCOL_VERSION {
            eprintln!(
                "protocol version mismatch: expected {PROTOCOL_VERSION}, got {}",
                command.protocol_version
            );
            continue;
        }
        if !runner.handle(command.command).await {
            break;
        }
    }
    Ok(())
}

async fn write_events(
    mut output: impl AsyncWrite + Unpin,
    mut events: mpsc::Receiver<EventEnvelope>,
) -> Result<()> {
    while let Some(event) = events.recv().await {
        let mut frame = serde_json::to_vec(&event).context("serialize runner event")?;
        frame.push(b'\n');
        output
            .write_all(&frame)
            .await
            .context("write runner event")?;
        output.flush().await.context("flush runner event")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_protocol::{Command, Event};

    fn frame(command: Command) -> String {
        serde_json::to_string(&CommandEnvelope::new(command)).unwrap() + "\n"
    }

    #[tokio::test]
    async fn skips_invalid_frames_and_stops_at_shutdown() {
        let mut wrong_version = CommandEnvelope::new(Command::RunnerHello);
        wrong_version.protocol_version = PROTOCOL_VERSION + 1;
        let input = format!(
            "invalid json\n{}\n{}{}{}",
            serde_json::to_string(&wrong_version).unwrap(),
            frame(Command::RunnerHello),
            frame(Command::RunnerShutdown),
            frame(Command::RunnerHello)
        );
        let mut output = Vec::new();

        serve(input.as_bytes(), &mut output).await.unwrap();
        let output = String::from_utf8(output).unwrap();
        assert_eq!(output.lines().count(), 1);
        let event: EventEnvelope = serde_json::from_str(output.trim()).unwrap();
        assert_eq!(event.protocol_version, PROTOCOL_VERSION);
        assert_eq!(event.sequence, 1);
        assert!(matches!(event.event, Event::RunnerReady));
    }

    #[tokio::test]
    async fn eof_flushes_pending_events_and_exits() {
        let input = frame(Command::RunnerHello) + &frame(Command::RunnerHello);
        let mut output = Vec::new();
        serve(input.as_bytes(), &mut output).await.unwrap();
        let output = String::from_utf8(output).unwrap();
        let events: Vec<EventEnvelope> = output
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].sequence, 2);
        assert!(
            events
                .iter()
                .all(|event| matches!(event.event, Event::RunnerReady))
        );
    }
}
