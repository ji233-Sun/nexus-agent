use crate::i18n::LocalizedText;
use crate::infrastructure::voice::Provider;
use uuid::Uuid;

#[derive(Default, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub(crate) struct VoiceSettings {
    pub(crate) provider: Option<Provider>,
    pub(crate) locale: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Operation {
    pub(crate) id: Uuid,
    pub(crate) conversation: Uuid,
    pub(crate) provider: Provider,
}

#[derive(Default)]
pub(crate) struct VoiceModel {
    pub(crate) settings: VoiceSettings,
    pub(crate) mimo_configured: bool,
    pub(crate) operation: Option<Operation>,
    pub(crate) transcribing: bool,
    pub(crate) status: LocalizedText,
}

impl VoiceModel {
    pub(crate) fn ready(&self) -> bool {
        match self.settings.provider {
            Some(Provider::Mimo) => self.mimo_configured,
            Some(Provider::MacOs) => cfg!(target_os = "macos"),
            None => false,
        }
    }
}
