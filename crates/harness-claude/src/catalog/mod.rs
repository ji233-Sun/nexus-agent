use super::*;

/// Anthropic 兼容 API 接入点。第三方网关（Kimi、GLM 等）与官方 API 共用同一套
/// 模型目录端点 `{base_url}/v1/models`。
#[derive(Debug, Default, PartialEq, Eq)]
struct AnthropicEndpoint {
    base_url: Option<String>,
    api_key: Option<String>,
    auth_token: Option<String>,
}

const ANTHROPIC_DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const MODEL_PAGE_LIMIT: u32 = 1000;
const MODEL_MAX_PAGES: usize = 5;
const MODEL_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

pub async fn discover_models(
    executable: &str,
    cwd: &Path,
    environment: &[EnvironmentVariable],
    cancel: watch::Receiver<bool>,
) -> Result<Vec<ModelDescriptor>, ModelCatalogError> {
    discover_models_in(
        executable,
        cwd,
        environment,
        cancel,
        user_claude_config_dir().as_deref(),
        |name| std::env::var(name).ok(),
    )
    .await
}

async fn discover_models_in(
    executable: &str,
    cwd: &Path,
    environment: &[EnvironmentVariable],
    cancel: watch::Receiver<bool>,
    user_config_dir: Option<&Path>,
    process_env: impl Fn(&str) -> Option<String>,
) -> Result<Vec<ModelDescriptor>, ModelCatalogError> {
    if *cancel.borrow() {
        return Err(ModelCatalogError::Cancelled);
    }
    if resolve_executable(executable).is_none() {
        return Err(ModelCatalogError::Failed(
            "未找到 Claude Code，无法加载模型目录。".into(),
        ));
    }
    // Claude Code 自身无法枚举第三方 API 的模型，目录改为直接询问该 API；
    // 任何失败（未配置凭证、网关不支持、网络异常）都回退到 CLI 别名。
    if let Some(endpoint) = resolve_endpoint_with(environment, cwd, user_config_dir, process_env) {
        match fetch_claude_models(&endpoint, &cancel).await {
            Ok(models) if !models.is_empty() => return Ok(models),
            Ok(_) | Err(ModelCatalogError::Failed(_)) => {}
            Err(error @ ModelCatalogError::Cancelled) => return Err(error),
        }
    }
    Ok(claude_alias_models())
}

/// CLI 别名目录：官方登录等无法枚举 API 目录的场景仍然可用。
fn claude_alias_models() -> Vec<ModelDescriptor> {
    // These are CLI aliases, not a discovered account catalog. Version-specific
    // model capabilities are deliberately left unknown until the adapter reports them.
    ClaudeModel::ALL
        .into_iter()
        .filter_map(|model| {
            Some(ModelDescriptor {
                id: model.cli_value()?.into(),
                display_name: model.to_string(),
                source: ModelSource::ClaudeAliases,
                availability: ModelAvailability::Unknown,
                provider: None,
                is_default: false,
                supported_reasoning_efforts: Vec::new(),
                default_reasoning_effort: None,
            })
        })
        .collect()
}

/// Claude Code 读取的用户级配置目录：`CLAUDE_CONFIG_DIR` 优先，默认 `~/.claude`。
fn user_claude_config_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        return Some(PathBuf::from(dir));
    }
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|home| PathBuf::from(home).join(".claude"))
}

fn claude_settings_paths(cwd: &Path, user_config_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut paths = vec![
        cwd.join(".claude/settings.local.json"),
        cwd.join(".claude/settings.json"),
    ];
    if let Some(dir) = user_config_dir {
        paths.push(dir.join("settings.json"));
    }
    paths
}

/// 解析模型目录端点。优先级与 Claude Code 启动时的生效顺序一致：
/// Provider Profile 注入的环境变量 → Claude Code settings 文件 → 进程环境。
fn resolve_endpoint_with(
    environment: &[EnvironmentVariable],
    cwd: &Path,
    user_config_dir: Option<&Path>,
    process_env: impl Fn(&str) -> Option<String>,
) -> Option<AnthropicEndpoint> {
    let mut endpoint = AnthropicEndpoint::default();
    for variable in environment {
        apply_endpoint_variable(&mut endpoint, &variable.name, &variable.value);
    }
    for path in claude_settings_paths(cwd, user_config_dir) {
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(settings) = serde_json::from_str::<Value>(&content) else {
            continue;
        };
        if let Some(env) = settings.get("env").and_then(Value::as_object) {
            for (name, value) in env {
                if let Some(value) = value.as_str() {
                    apply_endpoint_variable(&mut endpoint, name, value);
                }
            }
        }
    }
    for name in [
        "ANTHROPIC_BASE_URL",
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
    ] {
        if let Some(value) = process_env(name) {
            apply_endpoint_variable(&mut endpoint, name, &value);
        }
    }
    if endpoint.api_key.is_none() && endpoint.auth_token.is_none() {
        return None;
    }
    Some(AnthropicEndpoint {
        base_url: Some(
            endpoint
                .base_url
                .unwrap_or_else(|| ANTHROPIC_DEFAULT_BASE_URL.into()),
        ),
        api_key: endpoint.api_key,
        auth_token: endpoint.auth_token,
    })
}

/// 每个变量只接受首个非空来源，保证高优先级配置不被低优先级覆盖。
fn apply_endpoint_variable(endpoint: &mut AnthropicEndpoint, name: &str, value: &str) {
    let value = value.trim();
    if value.is_empty() {
        return;
    }
    match name {
        "ANTHROPIC_BASE_URL" => {
            let base = value.trim_end_matches('/');
            if !base.is_empty() {
                endpoint.base_url.get_or_insert(base.to_owned());
            }
        }
        "ANTHROPIC_API_KEY" => {
            endpoint.api_key.get_or_insert(value.to_owned());
        }
        "ANTHROPIC_AUTH_TOKEN" => {
            endpoint.auth_token.get_or_insert(value.to_owned());
        }
        _ => {}
    }
}

async fn fetch_claude_models(
    endpoint: &AnthropicEndpoint,
    cancel: &watch::Receiver<bool>,
) -> Result<Vec<ModelDescriptor>, ModelCatalogError> {
    let mut cancel = cancel.clone();
    let base_url = endpoint
        .base_url
        .as_deref()
        .unwrap_or(ANTHROPIC_DEFAULT_BASE_URL)
        .trim_end_matches('/');
    let client = reqwest::Client::builder()
        .timeout(MODEL_REQUEST_TIMEOUT)
        .build()
        .map_err(|_| ModelCatalogError::Failed("无法初始化模型目录 HTTP 客户端。".into()))?;
    let provider = reqwest::Url::parse(base_url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_else(|| "anthropic".into());
    let mut after_id: Option<String> = None;
    let mut models = Vec::new();
    for _ in 0..MODEL_MAX_PAGES {
        if *cancel.borrow() {
            return Err(ModelCatalogError::Cancelled);
        }
        let mut url = format!("{base_url}/v1/models?limit={MODEL_PAGE_LIMIT}");
        if let Some(after) = &after_id {
            url.push_str("&after_id=");
            url.push_str(after);
        }
        let mut request = client.get(url).header("anthropic-version", "2023-06-01");
        if let Some(token) = &endpoint.auth_token {
            request = request.bearer_auth(token);
        }
        if let Some(key) = &endpoint.api_key {
            request = request.header("x-api-key", key);
        }
        let value: Value = tokio::select! {
            biased;
            // 同时就绪时优先取消；未发出取消信号的关闭通道不影响请求。
            Ok(_) = cancel.wait_for(|cancelled| *cancelled) => {
                return Err(ModelCatalogError::Cancelled);
            }
            result = async {
                let response = request.send().await.map_err(|_| {
                    ModelCatalogError::Failed(format!("无法连接 {provider} 的模型目录接口。"))
                })?;
                if !response.status().is_success() {
                    return Err(ModelCatalogError::Failed(format!(
                        "{provider} 的模型目录接口返回了 {}。",
                        response.status().as_u16()
                    )));
                }
                response.json().await.map_err(|_| {
                    ModelCatalogError::Failed(format!("{provider} 的模型目录返回了无效 JSON。"))
                })
            } => result?,
        };
        models.extend(parse_claude_model_page(&value, &provider));
        match value.get("has_more").and_then(Value::as_bool) {
            Some(true) => {}
            _ => return Ok(models),
        }
        after_id = value
            .get("last_id")
            .and_then(Value::as_str)
            .filter(|last| !last.is_empty())
            .map(str::to_owned);
        if after_id.is_none() {
            return Ok(models);
        }
    }
    Ok(models)
}

fn parse_claude_model_page(value: &Value, provider: &str) -> Vec<ModelDescriptor> {
    value
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let id = item.get("id")?.as_str()?.trim();
            if id.is_empty() {
                return None;
            }
            let display_name = item
                .get("display_name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .unwrap_or(id);
            Some(ModelDescriptor {
                id: id.to_owned(),
                display_name: display_name.to_owned(),
                source: ModelSource::ClaudeApi,
                availability: ModelAvailability::Available,
                provider: Some(provider.to_owned()),
                is_default: false,
                supported_reasoning_efforts: Vec::new(),
                default_reasoning_effort: None,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests;
