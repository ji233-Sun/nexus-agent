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
        match updates::spawn(self.model.updates.channel) {
            Ok(events) => {
                self.update_events = Some(events);
                self.model.updates.state = UpdateState::Checking;
            }
            Err(error) => self.model.updates.state = updates::failure(error),
        }
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
                    if !self.model.updates.state.is_busy() {
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
