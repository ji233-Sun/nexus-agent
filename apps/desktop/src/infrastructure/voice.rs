//! Audio capture and speech-to-text providers.

#[path = "voice/audio.rs"]
mod audio;
#[path = "voice/mimo.rs"]
mod mimo;
#[cfg(target_os = "macos")]
#[path = "voice/native.rs"]
mod native;

use std::{
    sync::{Arc, Mutex, mpsc},
    thread,
};

use crate::model::voice::{Provider, supported_providers};
use anyhow::{Context, bail};

enum Command {
    Stop,
    Cancel,
}

pub(crate) struct Worker {
    command: mpsc::Sender<Command>,
    result: Arc<Mutex<Option<Result<String, String>>>>,
}

impl Worker {
    pub(crate) fn start(
        provider: Provider,
        locale: Option<String>,
        key: Option<String>,
    ) -> anyhow::Result<Self> {
        #[cfg(not(target_os = "macos"))]
        let _ = &locale;
        if !supported_providers().contains(&provider) {
            bail!("speech provider is not supported on this platform");
        }
        if provider == Provider::Mimo && key.as_deref().is_none_or(str::is_empty) {
            bail!("MiMo API key is required");
        }

        let (command, commands) = mpsc::channel();
        let result = Arc::new(Mutex::new(None));
        let thread_result = Arc::clone(&result);
        thread::Builder::new()
            .name("voice-worker".into())
            .spawn(move || {
                let outcome = match provider {
                    Provider::Mimo => mimo::run(commands, key.expect("key checked above")),
                    #[cfg(target_os = "macos")]
                    Provider::MacOs => native::run(commands, locale),
                    #[cfg(not(target_os = "macos"))]
                    Provider::MacOs => unreachable!(),
                };
                if let Some(outcome) = outcome {
                    *thread_result.lock().unwrap_or_else(|e| e.into_inner()) =
                        Some(outcome.map_err(|error| format!("{error:#}")));
                }
            })
            .context("failed to start voice worker")?;
        Ok(Self { command, result })
    }

    pub(crate) fn stop(&self) {
        let _ = self.command.send(Command::Stop);
    }

    pub(crate) fn cancel(&self) {
        let _ = self.command.send(Command::Cancel);
    }

    pub(crate) fn try_result(&self) -> Option<Result<String, String>> {
        self.result.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn native_status(locale: Option<&str>) -> String {
    native::status(locale)
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn native_status(_locale: Option<&str>) -> String {
    "unavailable: macOS Speech is not supported on this platform".into()
}

#[cfg(target_os = "macos")]
pub(crate) fn native_locales() -> Vec<String> {
    native::locales()
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn native_locales() -> Vec<String> {
    Vec::new()
}
