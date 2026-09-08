use super::*;
use crate::{
    infrastructure::updates,
    model::updates::{UpdateChannel, UpdateState},
};

impl Presenter {
    pub(crate) fn set_update_channel(&mut self, channel: UpdateChannel) -> bool {
        if self.model.updates.state.is_busy() {
            return false;
        }
        if self.model.updates.channel == channel {
            return true;
        }
        match self.storage.set_setting("update_channel", channel.as_str()) {
            Ok(()) => {
                self.model.updates.channel = channel;
                self.model.updates.state = UpdateState::Idle;
                true
            }
            Err(error) => {
                self.model.updates.state = UpdateState::Failed(LocalizedText::new(
                    "无法保存更新偏好：{error}",
                    &[("error", error.to_string())],
                ));
                false
            }
        }
    }

    pub(crate) fn set_update_check_on_startup(&mut self, enabled: bool) -> bool {
        match self.storage.set_setting(
            "update_check_on_startup",
            if enabled { "true" } else { "false" },
        ) {
            Ok(()) => {
                self.model.updates.check_on_startup = enabled;
                true
            }
            Err(error) => {
                self.model.status = LocalizedText::new(
                    "无法保存更新偏好：{error}",
                    &[("error", error.to_string())],
                );
                false
            }
        }
    }

    pub(crate) fn check_for_updates(&mut self) {
        if self.update_events.is_some() || self.model.updates.state.is_busy() {
            return;
        }
        match updates::spawn_check(self.model.updates.channel) {
            Ok(events) => {
                self.update_events = Some(events);
                self.model.updates.state = UpdateState::Checking;
            }
            Err(error) => self.model.updates.state = updates::failure(error),
        }
    }

    pub(crate) fn download_update(&mut self) {
        let UpdateState::Available(package) = &self.model.updates.state else {
            return;
        };
        if self.update_events.is_some() {
            return;
        }
        let package = package.clone();
        match updates::spawn_download(package.clone(), self.model.updates.channel) {
            Ok(events) => {
                self.update_events = Some(events);
                self.model.updates.state = UpdateState::Downloading {
                    package,
                    received: 0,
                };
            }
            Err(error) => self.model.updates.state = updates::failure(error),
        }
    }

    pub(crate) fn install_update_when_idle(&mut self) -> bool {
        if self.update_events.is_some()
            || self.model.active_run.is_some()
            || self.model.harness_manager.busy
        {
            return false;
        }
        let UpdateState::Ready { package, path } = &self.model.updates.state else {
            return false;
        };
        let package = package.clone();
        match updates::spawn_install(package.clone(), path.clone()) {
            Ok(events) => {
                self.update_events = Some(events);
                self.model.updates.state = UpdateState::Installing(package);
            }
            Err(error) => self.model.updates.state = updates::failure(error),
        }
        true
    }

    pub(crate) fn shutdown_for_update(&mut self) {
        // Release the embedded Runner and server before the installer restarts the app.
        self.runner.take();
        self.remote_control.take();
        self.codex_history_client.take();
        self.installation_worker.take();
    }

    pub(crate) fn report_update_error(&mut self, error: String) {
        self.model.updates.state = updates::failure(anyhow::anyhow!(error));
    }

    pub(crate) fn drain_update_events(&mut self) -> bool {
        let Some(events) = &self.update_events else {
            return false;
        };
        let mut changed = false;
        loop {
            match events.try_recv() {
                Ok(state) => {
                    self.model.updates.state = state;
                    changed = true;
                    if !self.model.updates.state.has_worker() {
                        self.update_events = None;
                        return true;
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => return changed,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.update_events = None;
                    self.model.updates.state =
                        UpdateState::Failed("更新任务已中断，请重试。".into());
                    return true;
                }
            }
        }
    }
}
