use super::*;

#[test]
fn voice_selection_persists_without_key_and_credentials_are_isolated() {
    use crate::{infrastructure::credentials::MIMO_VOICE_CREDENTIAL, model::voice::Provider};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("voice.sqlite");
    let credentials = FakeCredentialStore::default();
    let chat_profile = Uuid::new_v4();
    credentials.set_api_key(chat_profile, "chat-only").unwrap();
    let runner = FakeRunner::default();
    let create = || {
        Presenter::new_with_credentials(
            Storage::open(&path).unwrap(),
            Ok(Box::new(runner.clone())),
            None,
            Box::new(credentials.clone()),
        )
    };
    let mut presenter = create();
    assert_eq!(presenter.model.voice.settings.provider, None);
    assert!(!presenter.model.voice.ready());
    presenter.select_voice_provider(Provider::Mimo).unwrap();
    assert!(!presenter.model.voice.ready());
    assert!(presenter.start_voice().is_err());
    assert!(presenter.voice_worker.is_none());
    drop(presenter);
    let mut presenter = create();
    assert_eq!(
        presenter.model.voice.settings.provider,
        Some(Provider::Mimo)
    );
    assert!(!presenter.model.voice.ready());
    assert!(presenter.save_voice_key("  ").is_err());
    let commands_before = runner.0.borrow().commands.len();
    presenter.save_voice_key("voice-only").unwrap();
    assert!(presenter.model.voice.ready());
    assert!(presenter.voice_worker.is_none());
    assert_eq!(presenter.model.voice.status, LocalizedText::default());
    assert_eq!(runner.0.borrow().commands.len(), commands_before);
    assert_eq!(
        credentials.api_key(chat_profile).unwrap().as_deref(),
        Some("chat-only")
    );
    assert_eq!(
        credentials
            .api_key(MIMO_VOICE_CREDENTIAL)
            .unwrap()
            .as_deref(),
        Some("voice-only")
    );
    assert!(
        !presenter
            .storage
            .setting("voice_input")
            .unwrap()
            .unwrap()
            .contains("voice-only")
    );
    #[cfg(target_os = "macos")]
    {
        presenter.select_voice_provider(Provider::MacOs).unwrap();
        assert!(presenter.model.voice.ready());
        presenter.select_voice_provider(Provider::Mimo).unwrap();
        assert!(presenter.model.voice.ready());
    }
    #[cfg(not(target_os = "macos"))]
    assert!(presenter.select_voice_provider(Provider::MacOs).is_err());
    drop(presenter);
    let presenter = create();
    assert!(presenter.model.voice.ready());
    assert_eq!(
        presenter.model.voice.settings.provider,
        Some(Provider::Mimo)
    );
}

#[test]
fn voice_completion_rejects_cancelled_wrong_session_and_provider_results() {
    use crate::model::voice::{Operation, Provider};
    let (mut presenter, runner, _directory) = fixture();
    presenter.select_voice_provider(Provider::Mimo).unwrap();
    let operation = Operation {
        id: Uuid::new_v4(),
        conversation: presenter.model.conversation.id,
        provider: Provider::Mimo,
    };
    presenter.model.voice.operation = Some(operation);
    let text = "检查 src/main.rs 的 parseHTTP 函数";
    presenter.voice_error("previous voice error");
    assert!(
        presenter
            .complete_voice(
                Operation {
                    id: Uuid::new_v4(),
                    ..operation
                },
                Ok(text.into())
            )
            .is_none()
    );
    assert!(
        presenter
            .complete_voice(
                Operation {
                    provider: Provider::MacOs,
                    ..operation
                },
                Ok(text.into())
            )
            .is_none()
    );
    assert_eq!(
        presenter
            .complete_voice(operation, Ok(text.into()))
            .as_deref(),
        Some(text)
    );
    assert_eq!(presenter.model.voice.status, LocalizedText::default());
    assert!(runner.0.borrow().commands.is_empty());
    assert!(presenter.model.conversation.queued_messages.is_empty());
    presenter.model.voice.operation = Some(operation);
    presenter.voice_error("previous voice error");
    presenter.cancel_voice();
    assert_eq!(presenter.model.voice.status, LocalizedText::default());
    assert!(
        presenter
            .complete_voice(operation, Ok(text.into()))
            .is_none()
    );
    presenter.model.voice.operation = Some(operation);
    presenter.new_task();
    assert!(
        presenter
            .complete_voice(operation, Ok(text.into()))
            .is_none()
    );
    assert!(presenter.model.voice.operation.is_none());
    let operation = Operation {
        conversation: presenter.model.conversation.id,
        ..operation
    };
    presenter.model.voice.operation = Some(operation);
    assert!(
        presenter
            .complete_voice(operation, Err("MiMo rejected the API key".into()))
            .is_none()
    );
    assert_eq!(
        presenter.model.voice.settings.provider,
        Some(Provider::Mimo)
    );
    for language in [Language::Chinese, Language::English] {
        assert_eq!(
            presenter.model.voice.status.render(language),
            "MiMo rejected the API key"
        );
    }
    presenter.model.voice.operation = Some(operation);
    assert!(
        presenter
            .complete_voice(operation, Ok("  ".into()))
            .is_none()
    );
    assert_eq!(
        presenter.model.voice.status.render(Language::Chinese),
        "未识别到文本，请重试。"
    );
    assert_eq!(
        presenter.model.voice.status.render(Language::English),
        "No text was recognized. Please try again."
    );

    assert!(presenter.set_language(Language::English));
    assert_eq!(
        presenter.save_voice_key(" ").unwrap_err().to_string(),
        "Enter a MiMo API key"
    );
}
