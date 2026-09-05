use std::{
    env,
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener},
    sync::{Arc, mpsc},
    thread,
    time::Duration,
};

use anyhow::{Context as _, Result};
use axum::{
    Json, Router,
    extract::{
        Path, Query, State,
        ws::{Message as WebSocketMessage, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures_util::StreamExt as _;
use nexus_domain::{HarnessKind, Message, Project, TaskSummary, ThinkingEffort};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, oneshot};
use tower_http::cors::{Any, CorsLayer};
use uuid::Uuid;

pub const TOKEN_SETTING_KEY: &str = "remote_control_token";

const DEFAULT_BIND_ADDRESS: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 3210);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(3);
const COMMAND_QUEUE_CAPACITY: usize = 128;
const MAX_PROMPT_BYTES: usize = 64 * 1024;
const WEB_INDEX: &str = include_str!("../../remote-web/dist/index.html");
const WEB_SCRIPT: &str = include_str!("../../remote-web/dist/assets/app.js");
const WEB_STYLES: &str = include_str!("../../remote-web/dist/assets/app.css");

#[derive(Debug, Clone, Serialize)]
pub struct RemoteProject {
    pub id: Uuid,
    pub display_name: String,
}

impl From<&Project> for RemoteProject {
    fn from(project: &Project) -> Self {
        Self {
            id: project.id,
            display_name: project.display_name.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RemoteState {
    pub projects: Vec<RemoteProject>,
    pub tasks: Vec<TaskSummary>,
    pub selected_project_id: Option<Uuid>,
    pub selected_task_id: Option<Uuid>,
    pub active_run_id: Option<Uuid>,
    pub active_task_id: Option<Uuid>,
    pub streaming_text: String,
    pub status: String,
    pub harness: HarnessKind,
    pub model: Option<String>,
    pub effort: ThinkingEffort,
    pub harness_ready: bool,
}

pub enum RemoteCommand {
    GetState {
        reply: oneshot::Sender<RemoteState>,
    },
    GetMessages {
        task_id: Uuid,
        reply: oneshot::Sender<Vec<Message>>,
    },
    StartRun {
        project_id: Uuid,
        prompt: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
    CancelRun {
        reply: oneshot::Sender<Result<(), String>>,
    },
}

#[derive(Clone, Copy)]
enum RemoteSignal {
    StateChanged,
    Shutdown,
}

#[derive(Clone)]
struct ServerState {
    token: Arc<str>,
    commands: mpsc::SyncSender<RemoteCommand>,
    signals: broadcast::Sender<RemoteSignal>,
}

pub struct RemoteControl {
    address: SocketAddr,
    token: Arc<str>,
    commands: mpsc::Receiver<RemoteCommand>,
    signals: broadcast::Sender<RemoteSignal>,
    shutdown: Option<oneshot::Sender<()>>,
    server_thread: Option<thread::JoinHandle<()>>,
}

impl RemoteControl {
    pub fn start(token: String) -> Result<Self> {
        let address = match env::var("NEXUS_REMOTE_ADDR") {
            Ok(value) => value
                .parse()
                .with_context(|| format!("NEXUS_REMOTE_ADDR 不是有效地址：{value}"))?,
            Err(_) => DEFAULT_BIND_ADDRESS,
        };
        Self::start_on(address, token)
    }

    fn start_on(address: SocketAddr, token: String) -> Result<Self> {
        let listener =
            TcpListener::bind(address).with_context(|| format!("监听远程控制地址 {address}"))?;
        listener
            .set_nonblocking(true)
            .context("配置远程控制监听器")?;
        let address = listener.local_addr().context("读取远程控制监听地址")?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("创建远程控制运行时")?;
        let (command_tx, commands) = mpsc::sync_channel(COMMAND_QUEUE_CAPACITY);
        let (signals, _) = broadcast::channel(64);
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let token: Arc<str> = token.into();
        let state = ServerState {
            token: token.clone(),
            commands: command_tx,
            signals: signals.clone(),
        };
        let server_thread = thread::Builder::new()
            .name("nexus-remote-control".into())
            .spawn(move || {
                runtime.block_on(async move {
                    let listener = match tokio::net::TcpListener::from_std(listener) {
                        Ok(listener) => listener,
                        Err(error) => {
                            eprintln!("remote control listener failed: {error}");
                            return;
                        }
                    };
                    if let Err(error) = axum::serve(listener, router(state))
                        .with_graceful_shutdown(async {
                            let _ = shutdown_rx.await;
                        })
                        .await
                    {
                        eprintln!("remote control server failed: {error}");
                    }
                });
            })
            .context("启动远程控制线程")?;

        Ok(Self {
            address,
            token,
            commands,
            signals,
            shutdown: Some(shutdown_tx),
            server_thread: Some(server_thread),
        })
    }

    pub fn endpoint(&self) -> String {
        format!("http://{}", self.address)
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    pub fn drain_commands(&self) -> Vec<RemoteCommand> {
        let mut commands = Vec::new();
        while let Ok(command) = self.commands.try_recv() {
            commands.push(command);
        }
        commands
    }

    pub fn notify_changed(&self) {
        let _ = self.signals.send(RemoteSignal::StateChanged);
    }
}

impl Drop for RemoteControl {
    fn drop(&mut self) {
        let _ = self.signals.send(RemoteSignal::Shutdown);
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(server_thread) = self.server_thread.take() {
            let _ = server_thread.join();
        }
    }
}

fn router(state: ServerState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE]);
    Router::new()
        .route("/", get(web_app))
        .route("/assets/app.js", get(web_script))
        .route("/assets/app.css", get(web_styles))
        .route("/api/v1", get(service_info))
        .route("/health", get(health))
        .route("/api/v1/state", get(get_state))
        .route("/api/v1/tasks/{task_id}/messages", get(get_messages))
        .route("/api/v1/runs", post(start_run))
        .route("/api/v1/runs/cancel", post(cancel_run))
        .route("/api/v1/events", get(events))
        .layer(cors)
        .with_state(state)
}

async fn web_app() -> Response {
    static_response("text/html; charset=utf-8", WEB_INDEX)
}

async fn web_script() -> Response {
    static_response("text/javascript; charset=utf-8", WEB_SCRIPT)
}

async fn web_styles() -> Response {
    static_response("text/css; charset=utf-8", WEB_STYLES)
}

fn static_response(content_type: &'static str, body: &'static str) -> Response {
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        body,
    )
        .into_response()
}

#[derive(Serialize)]
struct ServiceInfo {
    name: &'static str,
    api_version: u8,
}

async fn service_info() -> Json<ServiceInfo> {
    Json(ServiceInfo {
        name: "Nexus Remote Control",
        api_version: 1,
    })
}

#[derive(Serialize)]
struct Health {
    status: &'static str,
}

async fn health() -> Json<Health> {
    Json(Health { status: "ok" })
}

async fn get_state(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<RemoteState>, ApiError> {
    authorize(&state, &headers)?;
    let (reply, response) = oneshot::channel();
    enqueue(&state, RemoteCommand::GetState { reply })?;
    Ok(Json(wait_for_response(response).await?))
}

async fn get_messages(
    State(state): State<ServerState>,
    Path(task_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<Vec<Message>>, ApiError> {
    authorize(&state, &headers)?;
    let (reply, response) = oneshot::channel();
    enqueue(&state, RemoteCommand::GetMessages { task_id, reply })?;
    Ok(Json(wait_for_response(response).await?))
}

#[derive(Deserialize)]
struct StartRunRequest {
    project_id: Uuid,
    prompt: String,
}

#[derive(Serialize)]
struct ActionResponse {
    accepted: bool,
}

async fn start_run(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(request): Json<StartRunRequest>,
) -> Result<(StatusCode, Json<ActionResponse>), ApiError> {
    authorize(&state, &headers)?;
    let prompt = request.prompt.trim().to_owned();
    if prompt.is_empty() {
        return Err(ApiError::invalid("Prompt 不能为空"));
    }
    if prompt.len() > MAX_PROMPT_BYTES {
        return Err(ApiError::invalid("Prompt 超过 64 KiB 限制"));
    }
    let (reply, response) = oneshot::channel();
    enqueue(
        &state,
        RemoteCommand::StartRun {
            project_id: request.project_id,
            prompt,
            reply,
        },
    )?;
    wait_for_response(response)
        .await?
        .map_err(ApiError::conflict)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(ActionResponse { accepted: true }),
    ))
}

async fn cancel_run(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<(StatusCode, Json<ActionResponse>), ApiError> {
    authorize(&state, &headers)?;
    let (reply, response) = oneshot::channel();
    enqueue(&state, RemoteCommand::CancelRun { reply })?;
    wait_for_response(response)
        .await?
        .map_err(ApiError::conflict)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(ActionResponse { accepted: true }),
    ))
}

#[derive(Deserialize)]
struct EventQuery {
    token: String,
}

async fn events(
    ws: WebSocketUpgrade,
    State(state): State<ServerState>,
    Query(query): Query<EventQuery>,
) -> Response {
    if query.token != state.token.as_ref() {
        return ApiError::unauthorized().into_response();
    }
    ws.on_upgrade(move |socket| stream_events(socket, state))
}

async fn stream_events(mut socket: WebSocket, state: ServerState) {
    let mut signals = state.signals.subscribe();
    if socket
        .send(WebSocketMessage::Text(r#"{"kind":"state.changed"}"#.into()))
        .await
        .is_err()
    {
        return;
    }

    loop {
        tokio::select! {
            signal = signals.recv() => match signal {
                Ok(RemoteSignal::StateChanged) | Err(broadcast::error::RecvError::Lagged(_)) => {
                    if socket
                        .send(WebSocketMessage::Text(
                            r#"{"kind":"state.changed"}"#.into(),
                        ))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Ok(RemoteSignal::Shutdown) | Err(broadcast::error::RecvError::Closed) => break,
            },
            message = socket.next() => match message {
                Some(Ok(WebSocketMessage::Close(_))) | None | Some(Err(_)) => break,
                _ => {}
            },
        }
    }
}

fn authorize(state: &ServerState, headers: &HeaderMap) -> Result<(), ApiError> {
    let provided = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    if provided == Some(state.token.as_ref()) {
        Ok(())
    } else {
        Err(ApiError::unauthorized())
    }
}

fn enqueue(state: &ServerState, command: RemoteCommand) -> Result<(), ApiError> {
    match state.commands.try_send(command) {
        Ok(()) => Ok(()),
        Err(mpsc::TrySendError::Full(_)) => Err(ApiError {
            status: StatusCode::TOO_MANY_REQUESTS,
            message: "远程请求过多，请稍后重试".into(),
        }),
        Err(mpsc::TrySendError::Disconnected(_)) => Err(ApiError::unavailable("Nexus 界面已断开")),
    }
}

async fn wait_for_response<T>(response: oneshot::Receiver<T>) -> Result<T, ApiError> {
    tokio::time::timeout(COMMAND_TIMEOUT, response)
        .await
        .map_err(|_| ApiError::unavailable("Nexus 界面响应超时"))?
        .map_err(|_| ApiError::unavailable("Nexus 界面已断开"))
}

struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn unauthorized() -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            message: "访问令牌无效".into(),
        }
    }

    fn invalid(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            message: message.into(),
        }
    }

    fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message: message.into(),
        }
    }

    fn unavailable(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            message: message.into(),
        }
    }
}

#[derive(Serialize)]
struct ApiErrorBody {
    error: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ApiErrorBody {
                error: self.message,
            }),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read as _, Write as _};

    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use serde_json::{Value, json};
    use tower::ServiceExt as _;

    use super::*;

    fn test_router(commands: mpsc::SyncSender<RemoteCommand>) -> Router {
        let (signals, _) = broadcast::channel(4);
        router(ServerState {
            token: "secret-token".into(),
            commands,
            signals,
        })
    }

    fn empty_state() -> RemoteState {
        RemoteState {
            projects: Vec::new(),
            tasks: Vec::new(),
            selected_project_id: None,
            selected_task_id: None,
            active_run_id: None,
            active_task_id: None,
            streaming_text: String::new(),
            status: "ready".into(),
            harness: HarnessKind::Codex,
            model: None,
            effort: ThinkingEffort::High,
            harness_ready: true,
        }
    }

    #[test]
    fn server_listens_on_tcp_and_reports_health() {
        let remote =
            RemoteControl::start_on("127.0.0.1:0".parse().unwrap(), "secret-token".into()).unwrap();
        let mut stream = std::net::TcpStream::connect(remote.address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        stream
            .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();

        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.contains(r#"{"status":"ok"}"#));
    }

    #[tokio::test]
    async fn websocket_accepts_the_token_and_streams_change_notifications() {
        let remote =
            RemoteControl::start_on("127.0.0.1:0".parse().unwrap(), "secret-token".into()).unwrap();
        let socket_url = format!("ws://{}/api/v1/events?token=secret-token", remote.address);
        let (mut socket, _) = tokio_tungstenite::connect_async(socket_url).await.unwrap();

        let initial = socket.next().await.unwrap().unwrap();
        assert_eq!(initial.into_text().unwrap(), r#"{"kind":"state.changed"}"#);
        remote.notify_changed();
        let changed = socket.next().await.unwrap().unwrap();
        assert_eq!(changed.into_text().unwrap(), r#"{"kind":"state.changed"}"#);

        socket.close(None).await.unwrap();
    }

    #[tokio::test]
    async fn web_app_is_available_without_exposing_api_data() {
        let (commands, _) = mpsc::sync_channel(4);
        let response = test_router(commands)
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/html; charset=utf-8"
        );
        let body = to_bytes(response.into_body(), 16 * 1024).await.unwrap();
        assert!(String::from_utf8_lossy(&body).contains("Nexus Remote"));
    }

    #[tokio::test]
    async fn state_endpoint_requires_the_access_token() {
        let (commands, _) = mpsc::sync_channel(4);
        let response = test_router(commands)
            .oneshot(
                Request::builder()
                    .uri("/api/v1/state")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn state_endpoint_returns_the_ui_snapshot() {
        let (commands, receiver) = mpsc::sync_channel(4);
        let responder = thread::spawn(move || {
            let RemoteCommand::GetState { reply } = receiver.recv().unwrap() else {
                panic!("expected state request")
            };
            reply.send(empty_state()).unwrap();
        });
        let response = test_router(commands)
            .oneshot(
                Request::builder()
                    .uri("/api/v1/state")
                    .header(header::AUTHORIZATION, "Bearer secret-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["status"], "ready");
        assert_eq!(body["harness"], "codex");
        responder.join().unwrap();
    }

    #[tokio::test]
    async fn timed_out_run_request_closes_the_queued_reply() {
        let (commands, receiver) = mpsc::sync_channel(4);
        let response = test_router(commands)
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/runs")
                    .header(header::AUTHORIZATION, "Bearer secret-token")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({
                            "project_id": Uuid::new_v4(),
                            "prompt": "expired request"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let RemoteCommand::StartRun { reply, .. } = receiver.try_recv().unwrap() else {
            panic!("expected run request")
        };
        assert!(reply.is_closed());
    }

    #[tokio::test]
    async fn run_endpoint_validates_and_forwards_the_prompt() {
        let project_id = Uuid::new_v4();
        let (commands, receiver) = mpsc::sync_channel(4);
        let responder = thread::spawn(move || {
            let RemoteCommand::StartRun {
                project_id: received_project_id,
                prompt,
                reply,
            } = receiver.recv().unwrap()
            else {
                panic!("expected run request")
            };
            assert_eq!(received_project_id, project_id);
            assert_eq!(prompt, "fix the issue");
            reply.send(Ok(())).unwrap();
        });
        let response = test_router(commands)
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/runs")
                    .header(header::AUTHORIZATION, "Bearer secret-token")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({
                            "project_id": project_id,
                            "prompt": "  fix the issue  "
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::ACCEPTED);
        responder.join().unwrap();
    }
}
