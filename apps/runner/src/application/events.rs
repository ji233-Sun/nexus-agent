use nexus_domain::RunStatus;
use nexus_harness_core::DecodedEvent;
use nexus_protocol::{Event, EventEnvelope, PROTOCOL_VERSION};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use tokio::sync::mpsc;
use uuid::Uuid;

#[derive(Clone)]
pub(crate) struct Emitter {
    tx: mpsc::Sender<EventEnvelope>,
    sequence: Arc<AtomicU64>,
}

impl Emitter {
    pub(crate) fn channel() -> (Self, mpsc::Receiver<EventEnvelope>) {
        let (tx, rx) = mpsc::channel(256);
        (
            Self {
                tx,
                sequence: Arc::new(AtomicU64::new(1)),
            },
            rx,
        )
    }

    pub(crate) async fn send(&self, event: Event) {
        let envelope = EventEnvelope {
            protocol_version: PROTOCOL_VERSION,
            id: Uuid::new_v4(),
            sequence: self.sequence.fetch_add(1, Ordering::Relaxed),
            event,
        };
        let _ = self.tx.send(envelope).await;
    }
}

pub(crate) async fn emit_decoded(run_id: Uuid, decoded: DecodedEvent, emitter: &Emitter) {
    let event = match decoded {
        DecodedEvent::TextDelta(text) => Event::RunOutputDelta { run_id, text },
        DecodedEvent::MessageCompleted(text) => Event::RunMessageCompleted { run_id, text },
        DecodedEvent::ToolStarted { id, name, summary } => Event::RunToolStarted {
            run_id,
            tool_id: id,
            name,
            summary,
        },
        DecodedEvent::ToolCompleted {
            id,
            output,
            is_error,
        } => Event::RunToolCompleted {
            run_id,
            tool_id: id,
            output,
            is_error,
        },
        DecodedEvent::Status(message) => Event::RunStatusChanged {
            run_id,
            status: RunStatus::Running,
            message: Some(message),
        },
        DecodedEvent::Error(message) => Event::RunStatusChanged {
            run_id,
            status: RunStatus::Running,
            message: Some(message),
        },
    };
    emitter.send(event).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn decoded_events_preserve_run_and_protocol_order() {
        let (emitter, mut events) = Emitter::channel();
        let run_id = Uuid::new_v4();
        emit_decoded(run_id, DecodedEvent::TextDelta("hello".into()), &emitter).await;
        emit_decoded(
            run_id,
            DecodedEvent::ToolCompleted {
                id: "tool-1".into(),
                output: "failed".into(),
                is_error: true,
            },
            &emitter,
        )
        .await;

        let first = events.recv().await.unwrap();
        let second = events.recv().await.unwrap();
        assert_eq!(first.protocol_version, PROTOCOL_VERSION);
        assert_eq!((first.sequence, second.sequence), (1, 2));
        assert_ne!(first.id, second.id);
        assert!(
            matches!(first.event, Event::RunOutputDelta { run_id: id, text }
            if id == run_id && text == "hello")
        );
        assert!(
            matches!(second.event, Event::RunToolCompleted { run_id: id, tool_id, output, is_error: true }
            if id == run_id && tool_id == "tool-1" && output == "failed")
        );
    }
}
