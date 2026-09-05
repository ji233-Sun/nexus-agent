use super::{CodexHistoryClient, Presenter};
use crate::{
    infrastructure::codex_history::Event as CodexHistoryEvent, model::history::HistoryMessage,
};
use nexus_domain::{MessageKind, MessageRole};
use std::path::PathBuf;

impl Presenter {
    pub(crate) fn history_available(&self) -> bool {
        self.codex_history_client.is_some()
    }

    pub(super) fn connect_codex_history(&mut self, executable: String) {
        if self.codex_history_executable.as_deref() != Some(&executable) {
            self.codex_history_client = Some(CodexHistoryClient::spawn(PathBuf::from(&executable)));
            self.codex_history_executable = Some(executable);
        }
        self.request_codex_history_refresh();
    }

    pub(crate) fn request_codex_history_refresh(&mut self) {
        self.model.codex_history_error = None;
        self.model.codex_history_loading = self
            .codex_history_client
            .as_ref()
            .is_some_and(CodexHistoryClient::refresh);
        if !self.model.codex_history_loading {
            self.model.codex_history_error = Some("Codex 历史服务不可用。".into());
        }
    }

    pub(super) fn handle_codex_history_event(&mut self, event: CodexHistoryEvent) {
        match event {
            CodexHistoryEvent::ThreadsLoaded(result) => {
                self.model.codex_history_loading = false;
                match result {
                    Ok(threads) => {
                        self.model.codex_threads = threads;
                        self.model.codex_history_error = None;
                    }
                    Err(error) => self.model.codex_history_error = Some(error),
                }
            }
            CodexHistoryEvent::ThreadLoaded { thread_id, result }
                if self.model.selected_codex_thread.as_deref() == Some(&thread_id) =>
            {
                self.model.codex_thread_loading = false;
                match result {
                    Ok(messages) => {
                        self.model.codex_history_messages = messages;
                        self.model.status = "Codex 历史会话已载入".into();
                    }
                    Err(error) => {
                        self.model.codex_history_messages = vec![HistoryMessage {
                            role: MessageRole::System,
                            kind: MessageKind::Error,
                            content: error,
                        }];
                        self.model.status = "无法读取 Codex 历史会话".into();
                    }
                }
            }
            CodexHistoryEvent::ThreadLoaded { .. } => {}
        }
    }

    pub(crate) fn select_codex_thread(&mut self, thread_id: String) {
        if self.model.active_run.is_some() {
            self.model.status = "任务执行期间不能切换历史会话。".into();
            return;
        }
        self.model.selected_task = None;
        self.model.selected_codex_thread = Some(thread_id.clone());
        self.model.messages.clear();
        self.model.streaming_text.clear();
        self.model.codex_history_messages.clear();
        self.model.codex_thread_loading = self
            .codex_history_client
            .as_ref()
            .is_some_and(|client| client.read_thread(thread_id));
        self.model.status = if self.model.codex_thread_loading {
            "正在读取 Codex 历史会话…".into()
        } else {
            "Codex 历史服务不可用。".into()
        };
    }
}
