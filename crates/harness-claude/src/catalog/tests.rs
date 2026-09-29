use super::*;

async fn serve_model_catalog(body: &'static str) -> (String, tokio::task::JoinHandle<String>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0u8; 4096];
        loop {
            let read = socket.read(&mut buffer).await.unwrap();
            request.extend_from_slice(&buffer[..read]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        socket.write_all(response.as_bytes()).await.unwrap();
        String::from_utf8(request).unwrap()
    });
    (format!("http://{address}/"), server)
}

#[tokio::test]
async fn catalog_prefers_api_discovery_and_falls_back_to_aliases() {
    let executable = std::env::current_exe().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let user_config = tempfile::tempdir().unwrap();
    let endpoint = vec![
        EnvironmentVariable {
            name: "ANTHROPIC_BASE_URL".into(),
            value: "http://127.0.0.1:0/anthropic".into(),
        },
        EnvironmentVariable {
            name: "ANTHROPIC_AUTH_TOKEN".into(),
            value: "gateway-token".into(),
        },
    ];

    // 网关不可达或未实现 /v1/models 时回退 CLI 别名，官方登录用户不受影响。
    let models = discover_models_in(
        executable.to_str().unwrap(),
        cwd.path(),
        &endpoint,
        watch::channel(false).1,
        Some(user_config.path()),
        |_| None,
    )
    .await
    .unwrap();
    assert_eq!(
        models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>(),
        ["sonnet", "opus", "haiku"]
    );
    for model in &models {
        assert_eq!(model.source, ModelSource::ClaudeAliases);
        assert_eq!(model.availability, ModelAvailability::Unknown);
        assert!(model.supported_reasoning_efforts.is_empty());
        assert!(model.default_reasoning_effort.is_none());
    }

    // 网关返回目录时直接采用 API 模型，不再混合别名。
    let (base_url, server) = serve_model_catalog(
        r#"{"data":[{"type":"model","id":"glm-4.6","display_name":"GLM-4.6"}],"has_more":false}"#,
    )
    .await;
    let mut endpoint = endpoint;
    endpoint[0].value = base_url;
    let models = discover_models_in(
        executable.to_str().unwrap(),
        cwd.path(),
        &endpoint,
        watch::channel(false).1,
        Some(user_config.path()),
        |_| None,
    )
    .await
    .unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, "glm-4.6");
    assert_eq!(models[0].source, ModelSource::ClaudeApi);
    assert_eq!(models[0].availability, ModelAvailability::Available);
    assert!(server.await.unwrap().contains("GET /v1/models?limit=1000 "));

    // 未配置任何凭证时完全不发起请求，直接回退别名。
    let models = discover_models_in(
        executable.to_str().unwrap(),
        cwd.path(),
        &[],
        watch::channel(false).1,
        Some(user_config.path()),
        |_| None,
    )
    .await
    .unwrap();
    assert_eq!(models[0].id, "sonnet");

    // 取消与缺失可执行文件的语义保持不变。
    let (cancel, receiver) = watch::channel(false);
    cancel.send_replace(true);
    assert_eq!(
        discover_models_in(
            "unused",
            cwd.path(),
            &[],
            receiver,
            Some(user_config.path()),
            |_| None,
        )
        .await,
        Err(ModelCatalogError::Cancelled)
    );
    assert!(matches!(
        discover_models_in(
            "nexus-missing-claude",
            cwd.path(),
            &[],
            watch::channel(false).1,
            Some(user_config.path()),
            |_| None,
        )
        .await,
        Err(ModelCatalogError::Failed(_))
    ));
}

#[tokio::test]
async fn fetch_claude_models_sends_gateway_credentials() {
    let (base_url, server) = serve_model_catalog(
        r#"{"data":[{"id":"kimi-k2-turbo-preview","display_name":"K2 Turbo"}],"has_more":false}"#,
    )
    .await;
    let endpoint = AnthropicEndpoint {
        base_url: Some(base_url),
        api_key: Some("sk-key".into()),
        auth_token: Some("bearer-token".into()),
    };
    let models = fetch_claude_models(&endpoint, &watch::channel(false).1)
        .await
        .unwrap();
    let request = server.await.unwrap();
    assert!(
        request.contains("GET /v1/models?limit=1000 HTTP/1.1"),
        "actual request: {request}"
    );
    assert!(
        request
            .to_ascii_lowercase()
            .contains("authorization: bearer bearer-token")
    );
    assert!(request.to_ascii_lowercase().contains("x-api-key: sk-key"));
    assert!(
        request
            .to_ascii_lowercase()
            .contains("anthropic-version: 2023-06-01")
    );
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, "kimi-k2-turbo-preview");
}

#[tokio::test]
async fn catalog_cancellation_interrupts_stalled_headers_and_body() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    for send_headers in [false, true] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let executable = std::env::current_exe().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let environment = [
            EnvironmentVariable {
                name: "ANTHROPIC_BASE_URL".into(),
                value: format!("http://{}", listener.local_addr().unwrap()),
            },
            EnvironmentVariable {
                name: "ANTHROPIC_API_KEY".into(),
                value: "test-key".into(),
            },
            EnvironmentVariable {
                name: "ANTHROPIC_AUTH_TOKEN".into(),
                value: "test-token".into(),
            },
        ];
        let (cancel, receiver) = watch::channel(false);
        let discovery = discover_models_in(
            executable.to_str().unwrap(),
            directory.path(),
            &environment,
            receiver,
            Some(directory.path()),
            |_| None,
        );
        tokio::pin!(discovery);
        let gateway = async {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = BufReader::new(socket);
            loop {
                let mut line = String::new();
                assert!(socket.read_line(&mut line).await.unwrap() > 0);
                if line == "\r\n" {
                    break;
                }
            }
            if send_headers {
                socket
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n")
                    .await
                    .unwrap();
            }
            socket
        };
        let socket = tokio::select! {
            result = &mut discovery => panic!("catalog returned before cancellation: {result:?}"),
            socket = tokio::time::timeout(Duration::from_secs(5), gateway) => socket.unwrap(),
        };
        // 继续轮询请求以消费已发送的响应头，同时保持响应体未完成。
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut discovery)
                .await
                .is_err()
        );
        cancel.send_replace(true);
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), &mut discovery).await,
            Ok(Err(ModelCatalogError::Cancelled)),
            "cancellation must interrupt the request (headers sent: {send_headers})"
        );
        drop(socket);
    }
}

#[test]
fn endpoint_resolution_follows_profile_settings_then_process_env() {
    let project = tempfile::tempdir().unwrap();
    let user_config = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".claude")).unwrap();
    std::fs::write(
        project.path().join(".claude/settings.json"),
        r#"{"env":{"ANTHROPIC_BASE_URL":"https://project.example/anthropic"}}"#,
    )
    .unwrap();
    std::fs::write(
        project.path().join(".claude/settings.local.json"),
        r#"{"env":{"ANTHROPIC_BASE_URL":"https://local.example/","ANTHROPIC_AUTH_TOKEN":"local-token"}}"#,
    )
    .unwrap();
    std::fs::write(
        user_config.path().join("settings.json"),
        r#"{"env":{"ANTHROPIC_AUTH_TOKEN":"user-token","ANTHROPIC_API_KEY":""}}"#,
    )
    .unwrap();
    let no_process_env = |name: &str| -> Option<String> {
        let _ = name;
        None
    };

    // 项目 local > 项目 > 用户级；空值被跳过；无凭证时返回 None。
    let resolved = resolve_endpoint_with(
        &[],
        project.path(),
        Some(user_config.path()),
        no_process_env,
    )
    .unwrap();
    assert_eq!(resolved.base_url.as_deref(), Some("https://local.example"));
    assert_eq!(resolved.auth_token.as_deref(), Some("local-token"));
    assert!(resolved.api_key.is_none());

    // Provider Profile 显式环境变量优先于 settings 文件。
    let profile = vec![
        EnvironmentVariable {
            name: "ANTHROPIC_BASE_URL".into(),
            value: "https://profile.example/".into(),
        },
        EnvironmentVariable {
            name: "ANTHROPIC_API_KEY".into(),
            value: "profile-key".into(),
        },
    ];
    let resolved = resolve_endpoint_with(
        &profile,
        project.path(),
        Some(user_config.path()),
        no_process_env,
    )
    .unwrap();
    assert_eq!(
        resolved.base_url.as_deref(),
        Some("https://profile.example")
    );
    assert_eq!(resolved.api_key.as_deref(), Some("profile-key"));
    assert_eq!(resolved.auth_token.as_deref(), Some("local-token"));

    // 进程环境只填补缺口；仅有凭证、无 base URL 时使用官方默认地址。
    let bare_project = tempfile::tempdir().unwrap();
    let resolved = resolve_endpoint_with(&[], bare_project.path(), None, |name| {
        (name == "ANTHROPIC_AUTH_TOKEN").then(|| "process-token".into())
    })
    .unwrap();
    assert_eq!(
        resolved.base_url.as_deref(),
        Some("https://api.anthropic.com")
    );
    assert_eq!(resolved.auth_token.as_deref(), Some("process-token"));
}

#[test]
fn claude_model_page_maps_api_catalog() {
    let models = parse_claude_model_page(
        &serde_json::json!({
            "data": [
                {"type": "model", "id": "claude-sonnet-4-5", "display_name": "Claude Sonnet 4.5"},
                {"id": "kimi-k2-turbo-preview"},
                {"id": "  "},
                {"display_name": "missing id"}
            ],
            "has_more": false
        }),
        "api.moonshot.ai",
    );
    assert_eq!(models.len(), 2);
    assert_eq!(models[0].id, "claude-sonnet-4-5");
    assert_eq!(models[0].display_name, "Claude Sonnet 4.5");
    assert_eq!(models[1].id, "kimi-k2-turbo-preview");
    assert_eq!(models[1].display_name, "kimi-k2-turbo-preview");
    for model in &models {
        assert_eq!(model.source, ModelSource::ClaudeApi);
        assert_eq!(model.source.harness(), HarnessKind::Claude);
        assert_eq!(model.availability, ModelAvailability::Available);
        assert_eq!(model.provider.as_deref(), Some("api.moonshot.ai"));
        assert!(!model.is_default);
    }
}
