use super::Presenter;
use crate::{
    infrastructure::{
        credentials::MIMO_VOICE_CREDENTIAL,
        voice::{self, Provider, Worker},
    },
    model::voice::{Operation, VoiceSettings},
};
use anyhow::{Result, bail};
use uuid::Uuid;

impl Presenter {
    pub(super) fn load_voice_settings(&mut self) {
        self.model.voice.settings = self
            .storage
            .setting("voice_input")
            .ok()
            .flatten()
            .and_then(|value| serde_json::from_str(&value).ok())
            .unwrap_or_default();
        match self.credentials.api_key(MIMO_VOICE_CREDENTIAL) {
            Ok(key) => {
                self.model.voice.mimo_configured = key.is_some_and(|key| !key.trim().is_empty())
            }
            Err(_) => self.model.voice.status = "无法读取语音凭据库，请在设置中重试保存。".into(),
        }
    }

    pub(crate) fn select_voice_provider(&mut self, provider: Provider) -> Result<()> {
        if !voice::supported_providers().contains(&provider) {
            bail!("当前平台不支持此语音 Provider");
        }
        let settings = VoiceSettings {
            provider: Some(provider),
            ..self.model.voice.settings.clone()
        };
        self.storage
            .set_setting("voice_input", &serde_json::to_string(&settings)?)?;
        self.cancel_voice();
        self.model.voice.settings = settings;
        self.model.voice.status.clear();
        Ok(())
    }

    pub(crate) fn set_voice_locale(&mut self, locale: Option<String>) -> Result<()> {
        if locale
            .as_ref()
            .is_some_and(|locale| !voice::native_locales().contains(locale))
        {
            bail!("系统不支持此识别语言");
        }
        let settings = VoiceSettings {
            locale,
            ..self.model.voice.settings.clone()
        };
        self.storage
            .set_setting("voice_input", &serde_json::to_string(&settings)?)?;
        self.cancel_voice();
        self.model.voice.settings = settings;
        Ok(())
    }

    pub(crate) fn save_voice_key(&mut self, key: &str) -> Result<()> {
        let key = key.trim();
        if key.is_empty() {
            bail!("请输入 MiMo API Key");
        }
        self.credentials
            .set_api_key(MIMO_VOICE_CREDENTIAL, key)
            .map_err(|_| anyhow::anyhow!("无法保存到系统凭据库，请解锁凭据库后重试。"))?;
        self.model.voice.mimo_configured = true;
        self.model.voice.status = "已配置；尚未验证服务请求。".into();
        Ok(())
    }

    pub(crate) fn voice_error(&mut self, error: impl std::fmt::Display) {
        self.model.voice.status = error.to_string();
    }

    pub(crate) fn start_voice(&mut self) -> Result<()> {
        if !self.model.voice.ready() {
            bail!("请先在设置 → 语音输入中选择并配置 Provider。");
        }
        if self.model.voice.operation.is_some() {
            bail!("语音操作正在进行");
        }
        if self.model.selected_codex_thread.is_some() {
            bail!("历史记录不可输入");
        }
        let provider = self.model.voice.settings.provider.unwrap();
        let key = if provider == Provider::Mimo {
            self.credentials
                .api_key(MIMO_VOICE_CREDENTIAL)
                .map_err(|_| anyhow::anyhow!("无法读取语音凭据库"))?
        } else {
            None
        };
        let worker = Worker::start(provider, self.model.voice.settings.locale.clone(), key)?;
        self.model.voice.operation = Some(Operation {
            id: Uuid::new_v4(),
            conversation: self.model.conversation.id,
            provider,
        });
        self.voice_worker = Some(worker);
        self.model.voice.transcribing = false;
        self.model.voice.status = "正在请求麦克风并录音（最多 60 秒）…".into();
        Ok(())
    }

    pub(crate) fn stop_voice(&mut self) {
        if let Some(worker) = &self.voice_worker {
            worker.stop();
            self.model.voice.transcribing = true;
            self.model.voice.status = "正在识别…".into();
        }
    }

    pub(crate) fn cancel_voice(&mut self) {
        self.voice_worker.take(); // Drop cancels recording and network work.
        if self.model.voice.operation.take().is_some() {
            self.model.voice.status = "语音输入已取消。".into();
        }
        self.model.voice.transcribing = false;
    }

    pub(crate) fn poll_voice(&mut self) -> Option<String> {
        let operation = self.model.voice.operation?;
        if operation.conversation != self.model.conversation.id
            || self.model.selected_codex_thread.is_some()
        {
            self.cancel_voice();
            return None;
        }
        let result = self.voice_worker.as_ref()?.try_result()?;
        self.complete_voice(operation, result)
    }

    pub(super) fn complete_voice(
        &mut self,
        operation: Operation,
        result: Result<String, String>,
    ) -> Option<String> {
        if self.model.voice.operation != Some(operation)
            || operation.conversation != self.model.conversation.id
            || self.model.voice.settings.provider != Some(operation.provider)
        {
            return None;
        }
        self.voice_worker.take();
        self.model.voice.operation = None;
        self.model.voice.transcribing = false;
        match result {
            Ok(text) if !text.trim().is_empty() => {
                self.model.voice.status = "已回填草稿，可编辑或撤销；尚未发送。".into();
                Some(text)
            }
            Ok(_) => {
                self.model.voice.status = "未识别到文本，请重试。".into();
                None
            }
            Err(error) => {
                self.model.voice.status = error;
                None
            }
        }
    }
}
