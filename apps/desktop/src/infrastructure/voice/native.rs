use std::{
    ffi::{CStr, CString, c_char, c_void},
    sync::mpsc,
    time::{Duration, Instant},
};

use anyhow::{Context, bail};

use super::Command;

unsafe extern "C" {
    fn nexus_microphone_authorization() -> i32;
    fn nexus_request_microphone_authorization() -> *mut c_char;
    fn nexus_speech_authorization() -> i32;
    fn nexus_request_speech_authorization() -> *mut c_char;
    fn nexus_speech_start(locale: *const c_char, error: *mut *mut c_char) -> *mut c_void;
    fn nexus_speech_stop(session: *mut c_void);
    fn nexus_speech_cancel(session: *mut c_void);
    fn nexus_speech_take_result(
        session: *mut c_void,
        done: *mut bool,
        failed: *mut bool,
    ) -> *mut c_char;
    fn nexus_speech_free_session(session: *mut c_void);
    fn nexus_speech_status(locale: *const c_char) -> *mut c_char;
    fn nexus_speech_locales() -> *mut c_char;
    fn nexus_speech_free_string(value: *mut c_char);
}

fn take_string(value: *mut c_char) -> Option<String> {
    if value.is_null() {
        return None;
    }
    let text = unsafe { CStr::from_ptr(value) }
        .to_string_lossy()
        .into_owned();
    unsafe { nexus_speech_free_string(value) };
    Some(text)
}

pub(super) fn run(
    commands: mpsc::Receiver<Command>,
    locale: Option<String>,
) -> Option<anyhow::Result<String>> {
    Some(run_inner(commands, locale))
}

fn run_inner(commands: mpsc::Receiver<Command>, locale: Option<String>) -> anyhow::Result<String> {
    authorize(
        &commands,
        unsafe { nexus_microphone_authorization() },
        nexus_microphone_authorization,
        nexus_request_microphone_authorization,
        "microphone",
    )?;
    authorize(
        &commands,
        unsafe { nexus_speech_authorization() },
        nexus_speech_authorization,
        nexus_request_speech_authorization,
        "Speech recognition",
    )?;
    ensure_recording_not_cancelled(&commands)?;
    let locale = locale
        .map(|value| CString::new(value).context("locale contains a null byte"))
        .transpose()?;
    let mut error = std::ptr::null_mut();
    let session = unsafe {
        nexus_speech_start(
            locale.as_ref().map_or(std::ptr::null(), |v| v.as_ptr()),
            &mut error,
        )
    };
    if session.is_null() {
        bail!(
            "{}",
            take_string(error).unwrap_or_else(|| "failed to start macOS Speech".into())
        );
    }
    struct Session(*mut c_void);
    impl Drop for Session {
        fn drop(&mut self) {
            unsafe { nexus_speech_free_session(self.0) }
        }
    }
    let session = Session(session);
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut final_deadline = None;
    let mut stopped = false;
    loop {
        match commands.recv_timeout(Duration::from_millis(50)) {
            Ok(Command::Cancel) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                unsafe { nexus_speech_cancel(session.0) };
                bail!("transcription cancelled");
            }
            Ok(Command::Stop) if !stopped => {
                unsafe { nexus_speech_stop(session.0) };
                stopped = true;
                final_deadline = Some(Instant::now() + Duration::from_secs(10));
            }
            Ok(Command::Stop) | Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if !stopped && Instant::now() >= deadline {
            unsafe { nexus_speech_stop(session.0) };
            stopped = true;
            final_deadline = Some(Instant::now() + Duration::from_secs(10));
        }
        if final_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            unsafe { nexus_speech_cancel(session.0) };
            bail!("macOS Speech timed out waiting for a final result");
        }
        let mut done = false;
        let mut failed = false;
        let text =
            take_string(unsafe { nexus_speech_take_result(session.0, &mut done, &mut failed) });
        if done {
            return final_result(text, failed);
        }
    }
}

fn final_result(text: Option<String>, failed: bool) -> anyhow::Result<String> {
    let text = text.context("macOS Speech failed without an error message")?;
    if failed {
        bail!("macOS Speech failed: {text}");
    }
    if text.is_empty() {
        bail!("macOS Speech returned an empty transcription");
    }
    Ok(text)
}

fn authorize(
    commands: &mpsc::Receiver<Command>,
    initial: i32,
    query: unsafe extern "C" fn() -> i32,
    request: unsafe extern "C" fn() -> *mut c_char,
    name: &str,
) -> anyhow::Result<()> {
    ensure_recording_not_cancelled(commands)?;
    if initial == 3 {
        return Ok(());
    }
    if initial == 2 {
        bail!("{name} permission was denied or restricted");
    }
    if initial != 1 {
        bail!("failed to query {name} permission");
    }
    if let Some(error) = take_string(unsafe { request() }) {
        bail!("{error}");
    }
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        match commands.recv_timeout(Duration::from_millis(50)) {
            Ok(Command::Cancel) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                bail!("transcription cancelled")
            }
            Ok(Command::Stop) => bail!("transcription stopped before recording started"),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        match unsafe { query() } {
            3 => return Ok(()),
            2 => bail!("{name} permission was denied or restricted"),
            1 if Instant::now() < deadline => {}
            1 => bail!("timed out waiting for {name} permission"),
            _ => bail!("failed to query {name} permission"),
        }
    }
}

pub(super) fn authorize_microphone(commands: &mpsc::Receiver<Command>) -> anyhow::Result<()> {
    authorize(
        commands,
        unsafe { nexus_microphone_authorization() },
        nexus_microphone_authorization,
        nexus_request_microphone_authorization,
        "microphone",
    )?;
    ensure_recording_not_cancelled(commands)
}

fn ensure_recording_not_cancelled(commands: &mpsc::Receiver<Command>) -> anyhow::Result<()> {
    match commands.try_recv() {
        Ok(Command::Cancel) | Err(mpsc::TryRecvError::Disconnected) => {
            bail!("transcription cancelled")
        }
        Ok(Command::Stop) => bail!("transcription stopped before recording started"),
        Err(mpsc::TryRecvError::Empty) => Ok(()),
    }
}

pub(super) fn status(locale: Option<&str>) -> String {
    let locale = locale.and_then(|value| CString::new(value).ok());
    take_string(unsafe {
        nexus_speech_status(locale.as_ref().map_or(std::ptr::null(), |v| v.as_ptr()))
    })
    .unwrap_or_else(|| "unavailable: failed to query macOS Speech".into())
}

pub(super) fn locales() -> Vec<String> {
    take_string(unsafe { nexus_speech_locales() })
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_final_result_uses_error_flag_not_transcription_prefix() {
        let text = "ERROR: file not found";
        assert_eq!(final_result(Some(text.into()), false).unwrap(), text);
        assert_eq!(
            final_result(Some(text.into()), true)
                .unwrap_err()
                .to_string(),
            "macOS Speech failed: ERROR: file not found"
        );
        assert!(final_result(Some(String::new()), false).is_err());
        assert!(final_result(None, false).is_err());
    }

    #[test]
    fn voice_permission_requests_reject_bare_test_binary_without_usage_descriptions() {
        // cargo test, like cargo run, has no .app Info.plist. Neither call may prompt.
        let (_sender, commands) = mpsc::channel();
        let microphone = authorize(
            &commands,
            1,
            nexus_microphone_authorization,
            nexus_request_microphone_authorization,
            "microphone",
        )
        .unwrap_err();
        assert!(
            microphone
                .to_string()
                .contains("NSMicrophoneUsageDescription")
        );
        let speech = authorize(
            &commands,
            1,
            nexus_speech_authorization,
            nexus_request_speech_authorization,
            "Speech recognition",
        )
        .unwrap_err();
        assert!(
            speech
                .to_string()
                .contains("NSSpeechRecognitionUsageDescription")
        );
    }
}
