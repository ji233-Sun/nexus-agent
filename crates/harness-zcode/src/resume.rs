use std::{path::Path, time::SystemTime};

use nexus_protocol::EnvironmentVariable;
use serde_json::{Value, json};

// Protocol v1 cold resume starts with an empty model adapter. Restore the
// selected provider from the CLI's documented provider configuration, only in
// the child process's stdin; never include credentials in model descriptors.
pub(super) fn runtime_model(
    cwd: &Path,
    environment: &[EnvironmentVariable],
    model: &Value,
    snapshot: &Value,
) -> Result<Value, String> {
    let env = |name: &str| {
        environment
            .iter()
            .rev()
            .find(|v| v.name == name)
            .map(|v| v.value.clone())
            .or_else(|| std::env::var(name).ok())
    };
    let mut paths = vec![];
    if let Some(home) = env(if cfg!(windows) { "USERPROFILE" } else { "HOME" }) {
        paths.push(Path::new(&home).join(".zcode/cli/config.json"));
    }
    let mut directories = vec![];
    for directory in cwd.ancestors() {
        directories.push(directory);
        if directory.join(".git").exists() {
            break;
        }
    }
    if !directories.last().is_some_and(|p| p.join(".git").exists()) {
        directories = vec![cwd];
    }
    for directory in directories.into_iter().rev() {
        paths.push(directory.join("zcode.json"));
        paths.push(directory.join(".zcode/config.json"));
    }
    let provider_id = model["providerId"]
        .as_str()
        .ok_or("ZCode 恢复会话缺少 Provider。")?;
    let model_id = model["modelId"]
        .as_str()
        .ok_or("ZCode 恢复会话缺少模型。")?;
    let mut configured = None;
    // Object-form model targets and ZCODE_MODEL can override provider endpoints.
    // Do not silently restore a different connection for these configurations.
    if env("ZCODE_MODEL").is_some_and(|value| !value.trim().is_empty()) {
        return Err(
            "ZCode 冷恢复请使用配置文件中的 provider/model，暂不支持 ZCODE_MODEL 覆盖。".into(),
        );
    }
    for path in paths {
        let contents = match std::fs::read(&path) {
            Ok(contents) => contents,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err(format!("无法读取 ZCode 配置：{}", path.display())),
        };
        let config: Value = serde_json::from_slice(&contents)
            .map_err(|_| format!("ZCode 配置不是有效 JSON：{}", path.display()))?;
        if config["model"].is_object()
            && ["main", "lite"]
                .iter()
                .any(|key| config["model"][key].is_object())
        {
            return Err(
                "ZCode 冷恢复请将 model.main / model.lite 配置为 provider/model 字符串。".into(),
            );
        }
        if let Some(provider) = config["provider"].get(provider_id) {
            configured = Some(provider.clone());
        }
    }
    let config = configured.ok_or(
        "ZCode 冷恢复需要 CLI 配置中的 provider；请在 zcode 中配置 API Key Provider 后重试。",
    )?;
    let kind = config["kind"]
        .as_str()
        .ok_or("ZCode 恢复需要显式配置 provider.kind。")?;
    if !["anthropic", "openai", "openai-compatible"].contains(&kind) {
        return Err("ZCode 恢复暂不支持此 Provider 类型。".into());
    }
    let model_config = config["models"]
        .as_object()
        .and_then(|models| {
            models
                .iter()
                .find(|(key, value)| value["id"].as_str().unwrap_or(key.as_str()) == model_id)
                .map(|(_, value)| value)
        })
        .ok_or("ZCode 历史模型已从 CLI Provider 配置移除，请选择可用模型。")?;
    let mut provider =
        json!({"providerId":provider_id,"kind":kind,"models":[{"modelId":model_id}]});
    // Use the runtime's model metadata, including reasoning mappings, rather
    // than duplicating the CLI's model-capability inference.
    if let Some(metadata) = snapshot["settings"]["model"]["available"]
        .as_array()
        .and_then(|models| {
            models
                .iter()
                .find(|m| m["ref"]["providerId"] == provider_id && m["ref"]["modelId"] == model_id)
        })
    {
        for key in [
            "label",
            "description",
            "contextWindow",
            "maxOutputTokens",
            "reasoning",
            "supportsImages",
            "supportsPdf",
            "supportsVideo",
            "supportsTools",
            "supportsStructuredOutput",
        ] {
            if let Some(value) = metadata.get(key) {
                provider["models"][0][key] = value.clone();
            }
        }
    }
    for key in ["baseURL", "apiKeyRequired"] {
        if let Some(value) = config["options"].get(key) {
            provider[key] = value.clone();
        }
    }
    if kind == "openai-compatible" && provider["baseURL"].as_str().is_none() {
        return Err("ZCode 恢复需要配置 provider.options.baseURL。".into());
    }
    if let Some(value) = config["options"]["apiKey"]
        .as_str()
        .filter(|s| !s.is_empty())
    {
        provider["apiKey"] = json!({"source":"inline","value":value});
    }
    let mut headers = serde_json::Map::new();
    for value in [
        &config["headers"],
        &config["options"]["headers"],
        &model_config["headers"],
    ] {
        if let Some(value) = value.as_object() {
            headers.extend(value.clone());
        }
    }
    if !headers.is_empty() {
        provider["headers"] = Value::Object(headers);
    }
    if let Some(name) = config.get("name") {
        provider["label"] = name.clone();
    }
    if let Some(options) = model_config.get("options") {
        let namespaced = [
            "anthropic",
            "openai",
            "openaiCompatible",
            "openai-compatible",
        ]
        .iter()
        .any(|key| options.get(key).is_some());
        let namespace = if kind == "openai-compatible" {
            "openaiCompatible"
        } else {
            kind
        };
        provider["models"][0]["providerOptions"] = if namespaced {
            options.clone()
        } else {
            json!({namespace: options})
        };
    }
    let generated_at = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|_| "系统时间早于 Unix epoch。")?
        .as_millis() as u64;
    Ok(
        json!({"revision":"nexus-resume","generatedAt":generated_at,"model":model,"provider":provider}),
    )
}
