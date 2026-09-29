use super::*;

const MODEL_PAGE_LIMIT: u64 = 100;
const MODEL_MAX_PAGES: usize = 100;
const APP_SERVER_TIMEOUT: Duration = Duration::from_secs(10);

pub async fn discover_models(
    configured_executable: &str,
    cwd: &Path,
    environment: &[EnvironmentVariable],
    mut cancel: watch::Receiver<bool>,
) -> Result<Vec<ModelDescriptor>, ModelCatalogError> {
    if *cancel.borrow() {
        return Err(ModelCatalogError::Cancelled);
    }
    let executable = resolve_executable(configured_executable).ok_or_else(|| {
        ModelCatalogError::Failed(
            "未找到 Codex CLI，无法加载模型目录。请检查可执行文件路径。".into(),
        )
    })?;
    let mut server = AppServer::spawn(&executable, cwd, environment)?;
    let result = {
        let query = async {
            server
                .request(
                    "initialize",
                    json!({
                        "clientInfo": {
                            "name": "nexus-agent",
                            "title": "Nexus Agent",
                            "version": env!("CARGO_PKG_VERSION")
                        }
                    }),
                )
                .await?;
            server.notify("initialized", json!({})).await?;
            list_all_models(&mut server).await
        };
        tokio::pin!(query);
        tokio::select! {
            result = &mut query => result,
            _ = cancel.changed() => Err(ModelCatalogError::Cancelled),
        }
    };
    server.shutdown().await;
    result
}

struct AppServer {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl AppServer {
    fn spawn(
        executable: &Path,
        cwd: &Path,
        environment: &[EnvironmentVariable],
    ) -> Result<Self, ModelCatalogError> {
        let mut command = Command::new(executable);
        hide_console_window(command.as_std_mut());
        let mut child = command
            .args(["app-server", "--listen", "stdio://"])
            .envs(
                environment
                    .iter()
                    .map(|variable| (&variable.name, &variable.value)),
            )
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| {
                ModelCatalogError::Failed(
                    "无法启动 Codex App Server。请检查 CLI 版本和可执行文件权限。".into(),
                )
            })?;
        let stdin = child.stdin.take().ok_or_else(|| {
            ModelCatalogError::Failed("无法连接 Codex App Server 输入流。".into())
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            ModelCatalogError::Failed("无法连接 Codex App Server 输出流。".into())
        })?;
        Ok(Self {
            child,
            stdin: Some(stdin),
            stdout: BufReader::new(stdout),
            next_id: 0,
        })
    }

    async fn notify(&mut self, method: &str, params: Value) -> Result<(), ModelCatalogError> {
        self.write_frame(&json!({ "method": method, "params": params }))
            .await
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value, ModelCatalogError> {
        let id = self.next_id;
        self.next_id += 1;
        self.write_frame(&json!({ "id": id, "method": method, "params": params }))
            .await?;
        timeout(APP_SERVER_TIMEOUT, self.read_response(id, method))
            .await
            .map_err(|_| {
                ModelCatalogError::Failed(format!(
                    "Codex App Server 的 {method} 请求超时，请重试。"
                ))
            })?
    }

    async fn read_response(&mut self, id: u64, method: &str) -> Result<Value, ModelCatalogError> {
        loop {
            let mut line = String::new();
            let read = self.stdout.read_line(&mut line).await.map_err(|_| {
                ModelCatalogError::Failed(format!("读取 Codex App Server {method} 响应失败。"))
            })?;
            if read == 0 {
                return Err(ModelCatalogError::Failed(format!(
                    "Codex App Server 在返回 {method} 前退出，当前 CLI 可能不支持模型目录。"
                )));
            }
            let frame: Value = serde_json::from_str(line.trim()).map_err(|_| {
                ModelCatalogError::Failed(format!(
                    "Codex App Server 的 {method} 响应不是有效 JSON。"
                ))
            })?;
            if frame.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(message) = frame.pointer("/error/message").and_then(Value::as_str) {
                return Err(ModelCatalogError::Failed(format!(
                    "Codex App Server {method} 失败：{}",
                    summarize_text(message)
                )));
            }
            return frame.get("result").cloned().ok_or_else(|| {
                ModelCatalogError::Failed(format!("Codex App Server 的 {method} 响应缺少 result。"))
            });
        }
    }

    async fn write_frame(&mut self, frame: &Value) -> Result<(), ModelCatalogError> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| ModelCatalogError::Failed("Codex App Server 输入流已关闭。".into()))?;
        let mut encoded = serde_json::to_vec(frame)
            .map_err(|_| ModelCatalogError::Failed("无法编码 Codex App Server 请求。".into()))?;
        encoded.push(b'\n');
        stdin
            .write_all(&encoded)
            .await
            .map_err(|_| ModelCatalogError::Failed("写入 Codex App Server 请求失败。".into()))?;
        stdin
            .flush()
            .await
            .map_err(|_| ModelCatalogError::Failed("刷新 Codex App Server 请求失败。".into()))
    }

    async fn shutdown(mut self) {
        self.stdin.take();
        if matches!(
            timeout(Duration::from_millis(500), self.child.wait()).await,
            Ok(Ok(_))
        ) {
            return;
        }
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
    }
}

async fn list_all_models(
    server: &mut AppServer,
) -> Result<Vec<ModelDescriptor>, ModelCatalogError> {
    let mut models = Vec::new();
    let mut cursor: Option<String> = None;
    let mut seen_cursors = HashSet::new();
    for _ in 0..MODEL_MAX_PAGES {
        let mut params = Map::new();
        params.insert("limit".into(), Value::from(MODEL_PAGE_LIMIT));
        params.insert("includeHidden".into(), Value::Bool(false));
        if let Some(cursor) = &cursor {
            params.insert("cursor".into(), Value::String(cursor.clone()));
        }
        let result = server.request("model/list", Value::Object(params)).await?;
        let (mut page, next_cursor) = parse_model_page(&result)?;
        models.append(&mut page);
        let Some(next_cursor) = next_cursor else {
            return Ok(models);
        };
        if next_cursor.is_empty() || !seen_cursors.insert(next_cursor.clone()) {
            return Err(ModelCatalogError::Failed(
                "Codex App Server 返回了重复的模型目录游标。".into(),
            ));
        }
        cursor = Some(next_cursor);
    }
    Err(ModelCatalogError::Failed(
        "Codex App Server 模型目录分页超过安全上限。".into(),
    ))
}

fn parse_model_page(
    result: &Value,
) -> Result<(Vec<ModelDescriptor>, Option<String>), ModelCatalogError> {
    let data = result
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| ModelCatalogError::Failed("Codex model/list 响应缺少 data。".into()))?;
    let mut models = Vec::with_capacity(data.len());
    for item in data {
        if item.get("hidden").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let id = item
            .get("id")
            .and_then(Value::as_str)
            .or_else(|| item.get("model").and_then(Value::as_str))
            .filter(|id| !id.is_empty())
            .ok_or_else(|| {
                ModelCatalogError::Failed("Codex model/list 返回了缺少 ID 的模型。".into())
            })?
            .to_owned();
        let display_name = item
            .get("displayName")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .unwrap_or(&id)
            .to_owned();
        let mut supported_reasoning_efforts = Vec::new();
        for option in item
            .get("supportedReasoningEfforts")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let value = option
                .get("reasoningEffort")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    ModelCatalogError::Failed(
                        "Codex model/list 返回了缺少 reasoningEffort 的能力项。".into(),
                    )
                })?;
            let effort = value.parse().map_err(ModelCatalogError::Failed)?;
            supported_reasoning_efforts.push(ModelReasoningEffort {
                effort,
                description: option
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            });
        }
        let default_reasoning_effort = item
            .get("defaultReasoningEffort")
            .and_then(Value::as_str)
            .map(str::parse)
            .transpose()
            .map_err(ModelCatalogError::Failed)?;
        models.push(ModelDescriptor {
            id,
            display_name,
            source: nexus_domain::ModelSource::CodexAppServer,
            availability: nexus_domain::ModelAvailability::Available,
            provider: None,
            is_default: item
                .get("isDefault")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            supported_reasoning_efforts,
            default_reasoning_effort,
        });
    }
    let next_cursor = result
        .get("nextCursor")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Ok((models, next_cursor))
}

#[cfg(test)]
mod tests;
