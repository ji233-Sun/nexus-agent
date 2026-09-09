use std::sync::mpsc;

use anyhow::{Context, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};

use super::{Command, audio};

const ENDPOINT: &str = "https://api.xiaomimimo.com/v1/chat/completions";
const MAX_BASE64_BYTES: usize = 10_000_000;
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(90);

pub(super) fn run(
    commands: mpsc::Receiver<Command>,
    key: String,
) -> Option<anyhow::Result<String>> {
    let recording = match audio::capture(&commands) {
        Ok(Some(value)) => value,
        Ok(None) => return None,
        Err(e) => return Some(Err(e)),
    };
    let wav = match audio::wav_mono(&recording) {
        Ok(value) => value,
        Err(e) => return Some(Err(e)),
    };
    if let Err(error) = validate_base64_size(wav.len()) {
        return Some(Err(error));
    }
    let data = STANDARD.encode(wav);
    let body = request_body(&data);
    Some(send(commands, key, body))
}

fn request_body(audio: &str) -> Value {
    json!({
        "model": "mimo-v2.5-asr",
        "messages": [{"role": "user", "content": [{"type": "input_audio", "input_audio": {
            "data": format!("data:audio/wav;base64,{audio}")
        }}]}],
        "asr_options": {"language": "auto"},
        "stream": false
    })
}

fn validate_base64_size(input_bytes: usize) -> anyhow::Result<()> {
    let encoded = input_bytes
        .checked_add(2)
        .and_then(|value| value.checked_div(3))
        .and_then(|value| value.checked_mul(4))
        .context("recording is too large to encode")?;
    if encoded > MAX_BASE64_BYTES {
        bail!("recording exceeds MiMo's 10,000,000-byte Base64 limit");
    }
    Ok(())
}

fn status_error(status: reqwest::StatusCode) -> &'static str {
    match status.as_u16() {
        401 | 403 => "MiMo rejected the API key",
        402 => "MiMo account quota is exhausted",
        429 => "MiMo rate limit exceeded",
        400 => "MiMo rejected the transcription request",
        413 => "MiMo rejected the recording as too large",
        500..=599 => "MiMo service is temporarily unavailable",
        _ => "MiMo request was rejected",
    }
}

fn send(commands: mpsc::Receiver<Command>, key: String, body: Value) -> anyhow::Result<String> {
    let mut key = reqwest::header::HeaderValue::from_str(&key)
        .context("MiMo API key contains invalid characters")?;
    key.set_sensitive(true);
    // Never forward audio or the custom authentication header to a redirected host.
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let operation = async {
            let mut response = client
                .post(ENDPOINT)
                .header("api-key", key)
                .json(&body)
                .send()
                .await
                .context("MiMo request failed")?;
            if !response.status().is_success() {
                bail!(status_error(response.status()));
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .context("failed to read MiMo response")?
            {
                if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                    bail!("MiMo response exceeded the size limit");
                }
                bytes.extend_from_slice(&chunk);
            }
            anyhow::Ok(bytes)
        };
        tokio::pin!(operation);
        let mut poll = tokio::time::interval(std::time::Duration::from_millis(50));
        let bytes = tokio::select! {
            biased;
            _ = async {
                loop {
                    poll.tick().await;
                    match commands.try_recv() {
                        Ok(Command::Cancel) | Err(mpsc::TryRecvError::Disconnected) => return,
                        Ok(Command::Stop) | Err(mpsc::TryRecvError::Empty) => {}
                    }
                }
            } => bail!("transcription cancelled"),
            result = &mut operation => result?,
            _ = tokio::time::sleep(REQUEST_TIMEOUT) => bail!("MiMo request timed out"),
        };
        let response: Value = serde_json::from_slice(&bytes).context("invalid MiMo response")?;
        let text = response
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .context("MiMo returned an empty transcription")?;
        Ok(text.to_owned())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn protocol_is_official_non_streaming_schema() {
        let body = request_body("YWJj");
        assert_eq!(body["model"], "mimo-v2.5-asr");
        assert_eq!(body["stream"], false);
        assert_eq!(body["asr_options"]["language"], "auto");
        assert_eq!(
            body["messages"][0]["content"][0]["input_audio"]["data"],
            "data:audio/wav;base64,YWJj"
        );
    }

    #[test]
    fn base64_limit_accepts_boundary_and_rejects_above() {
        // 7,500,000 bytes encode to exactly 10,000,000 Base64 bytes.
        assert!(validate_base64_size(7_500_000).is_ok());
        assert!(validate_base64_size(7_500_001).is_err());
    }

    #[test]
    fn status_errors_do_not_include_server_bodies() {
        assert_eq!(
            status_error(reqwest::StatusCode::UNAUTHORIZED),
            "MiMo rejected the API key"
        );
        assert_eq!(
            status_error(reqwest::StatusCode::PAYMENT_REQUIRED),
            "MiMo account quota is exhausted"
        );
        assert_eq!(
            status_error(reqwest::StatusCode::TOO_MANY_REQUESTS),
            "MiMo rate limit exceeded"
        );
    }
}
