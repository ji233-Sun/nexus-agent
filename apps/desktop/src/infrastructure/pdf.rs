use crate::i18n::Language;
use anyhow::{Context as _, Result};
use axum::{
    Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path as RoutePath, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse as _, Response},
    routing::{get, post},
};
use serde::Deserialize;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};
use tokio::sync::{Mutex, oneshot};

include!(concat!(env!("OUT_DIR"), "/pdf_assets.rs"));
const MAX_PDF_BYTES: usize = 100 * 1024 * 1024;

pub(crate) enum PdfEvent {
    Capture {
        name: String,
        page: u32,
        bytes: Bytes,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Error(String),
}

pub(crate) struct PdfServer {
    pub(crate) url: String,
    pub(crate) name: String,
    pub(crate) close: Arc<AtomicBool>,
    pub(crate) can_close: Arc<AtomicBool>,
    pub(crate) events: mpsc::Sender<PdfEvent>,
    task: tokio::task::JoinHandle<()>,
    _snapshot: tempfile::TempDir,
}

#[derive(Clone)]
struct DocumentState {
    name: String,
    original: PathBuf,
    language: Language,
    origin: String,
    events: mpsc::Sender<PdfEvent>,
    destination: Arc<Mutex<Option<PathBuf>>>,
    close: Arc<AtomicBool>,
}

impl PdfServer {
    pub(crate) fn open(
        path: &Path,
        language: Language,
        events: mpsc::Sender<PdfEvent>,
    ) -> Result<Self> {
        use std::io::{Read as _, Write as _};
        let original = path.canonicalize().context("无法打开 PDF")?;
        anyhow::ensure!(original.is_file(), "PDF 路径不是文件");
        let mut bytes = Vec::new();
        std::fs::File::open(&original)?
            .take(MAX_PDF_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        anyhow::ensure!(bytes.len() <= MAX_PDF_BYTES, "PDF 不能超过 100 MiB");
        anyhow::ensure!(bytes.starts_with(b"%PDF-"), "此文件不是有效的 PDF");
        // Hold a snapshot so external edits cannot change a document halfway through reading it.
        let snapshot = tempfile::tempdir()?;
        let document = snapshot.path().join("document.pdf");
        std::fs::File::create(&document)?.write_all(&bytes)?;
        let name = original
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let origin = format!("http://{}", listener.local_addr()?);
        let prefix = format!("/{}", uuid::Uuid::new_v4());
        let url = format!("{origin}{prefix}/index.html");
        let close = Arc::new(AtomicBool::new(false));
        let state = DocumentState {
            name: name.clone(),
            original,
            language,
            origin,
            events: events.clone(),
            destination: Arc::new(Mutex::new(None)),
            close: close.clone(),
        };
        let router = document_router(&prefix, &document, state);
        let task = reqwest_client::runtime().spawn(async move {
            if let Ok(listener) = tokio::net::TcpListener::from_std(listener) {
                let _ = axum::serve(listener, router).await;
            }
        });
        Ok(Self {
            url,
            name,
            events,
            close,
            can_close: Arc::new(AtomicBool::new(true)),
            task,
            _snapshot: snapshot,
        })
    }
}

impl Drop for PdfServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn document_router(prefix: &str, document: &Path, state: DocumentState) -> Router {
    Router::new().nest(
        prefix,
        Router::new()
            .route(
                "/meta",
                get(|State(state): State<DocumentState>| async move {
                    axum::Json(
                        serde_json::json!({"name":state.name,"language":state.language.as_str()}),
                    )
                }),
            )
            .route_service(
                "/document",
                tower_http::services::ServeFile::new_with_mime(
                    document,
                    &"application/pdf".parse().unwrap(),
                ),
            )
            .route(
                "/capture/{page}",
                post(capture).layer(DefaultBodyLimit::max(
                    nexus_domain::ImageAttachment::MAX_BYTES,
                )),
            )
            .route(
                "/save",
                post(save).layer(DefaultBodyLimit::max(MAX_PDF_BYTES)),
            )
            .route(
                "/close",
                post(
                    |State(state): State<DocumentState>, headers: HeaderMap| async move {
                        if !same_origin(&state, &headers) {
                            return StatusCode::FORBIDDEN.into_response();
                        }
                        state.close.store(true, Ordering::Release);
                        axum::Json(serde_json::json!({"closed":true})).into_response()
                    },
                ),
            )
            .route("/{*asset}", get(asset))
            .with_state(state),
    )
}

fn same_origin(state: &DocumentState, headers: &HeaderMap) -> bool {
    headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        == Some(state.origin.as_str())
}

async fn capture(
    State(state): State<DocumentState>,
    RoutePath(page): RoutePath<u32>,
    headers: HeaderMap,
    bytes: Bytes,
) -> Response {
    if !same_origin(&state, &headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if page == 0 || nexus_harness_core::validate_capture(&bytes).is_err() {
        return (StatusCode::BAD_REQUEST, "无效的 PDF 截图").into_response();
    }
    let (reply, receive) = oneshot::channel();
    if state
        .events
        .send(PdfEvent::Capture {
            name: state.name,
            page,
            bytes,
            reply,
        })
        .is_err()
    {
        return (StatusCode::GONE, "聊天窗口已关闭").into_response();
    }
    match tokio::time::timeout(std::time::Duration::from_secs(15), receive).await {
        Ok(Ok(Ok(()))) => axum::Json(serde_json::json!({"attached":true})).into_response(),
        Ok(Ok(Err(error))) => (StatusCode::BAD_REQUEST, error).into_response(),
        _ => (StatusCode::SERVICE_UNAVAILABLE, "无法加入聊天，请重试").into_response(),
    }
}

#[derive(Deserialize)]
struct SaveOptions {
    #[serde(default)]
    copy: bool,
}

async fn save(
    State(state): State<DocumentState>,
    Query(options): Query<SaveOptions>,
    headers: HeaderMap,
    bytes: Bytes,
) -> Response {
    if !same_origin(&state, &headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if !bytes.starts_with(b"%PDF-") {
        return (StatusCode::BAD_REQUEST, "无效的 PDF 文件").into_response();
    }
    let mut saved = state.destination.lock().await;
    let destination = if options.copy || saved.is_none() {
        let original = state.original.clone();
        let name = format!(
            "{}-annotated.pdf",
            original.file_stem().unwrap_or_default().to_string_lossy()
        );
        tokio::task::spawn_blocking(move || {
            rfd::FileDialog::new()
                .add_filter("PDF", &["pdf"])
                .set_directory(original.parent().unwrap_or(Path::new(".")))
                .set_file_name(name)
                .save_file()
        })
        .await
        .ok()
        .flatten()
    } else {
        saved.clone()
    };
    let Some(destination) = destination else {
        return axum::Json(serde_json::json!({"saved":false})).into_response();
    };
    let path = destination.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<()> {
        use std::io::Write as _;
        let mut file = tempfile::NamedTempFile::new_in(path.parent().context("保存路径无效")?)?;
        file.write_all(&bytes)?;
        file.as_file().sync_all()?;
        if let Ok(metadata) = std::fs::metadata(&path) {
            file.as_file().set_permissions(metadata.permissions())?;
        }
        file.persist(&path).context("保存 PDF 失败")?;
        Ok(())
    })
    .await;
    match result {
        Ok(Ok(())) => {
            let name = destination
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            *saved = Some(destination);
            axum::Json(serde_json::json!({"saved":true,"name":name})).into_response()
        }
        Ok(Err(error)) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response(),
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response(),
    }
}

async fn asset(RoutePath(name): RoutePath<String>) -> Response {
    let Ok(index) = ASSETS.binary_search_by_key(&name.as_str(), |(path, _)| *path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mime = match name.rsplit('.').next().unwrap_or_default() {
        "html" => "text/html; charset=utf-8",
        "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "wasm" => "application/wasm",
        "svg" => "image/svg+xml",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        _ => "application/octet-stream",
    };
    (
        [
            (header::CONTENT_TYPE, mime),
            (header::CONTENT_ENCODING, "gzip"),
            (header::CACHE_CONTROL, "private, max-age=3600"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        ASSETS[index].1,
    )
        .into_response()
}

pub(crate) fn local_pdf_path(link: &str, directory: Option<&str>) -> Option<PathBuf> {
    let path = if let Ok(url) = reqwest::Url::parse(link) {
        if url.scheme() == "file" {
            url.to_file_path().ok()?
        } else if Path::new(link).is_absolute() {
            PathBuf::from(link)
        } else {
            return None;
        }
    } else if Path::new(link).is_absolute() {
        PathBuf::from(link)
    } else {
        reqwest::Url::from_directory_path(directory?)
            .ok()?
            .join(link)
            .ok()?
            .to_file_path()
            .ok()?
    };
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
        .then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt as _;

    #[tokio::test]
    async fn pdf_server_scopes_files_rejects_foreign_writes_and_reports_capture_failures() {
        use base64::Engine as _;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("document.pdf");
        std::fs::write(&path, b"%PDF-1.7\nsnapshot").unwrap();
        let (events, receive) = mpsc::channel();
        let state = DocumentState {
            name: "报告.pdf".into(),
            original: path.clone(),
            language: Language::Chinese,
            origin: "http://127.0.0.1:1234".into(),
            events,
            destination: Arc::new(Mutex::new(None)),
            close: Arc::new(AtomicBool::new(false)),
        };
        let router = document_router("/secret", &path, state);
        let response = router
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/document")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let response = router
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/secret/document")
                    .header("Range", "bytes=0-4")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        let response = router
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/secret/close")
                    .header("Origin", "https://example.test")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let png = base64::engine::general_purpose::STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=").unwrap();
        let responder = std::thread::spawn(move || {
            let PdfEvent::Capture {
                page,
                name,
                bytes,
                reply,
            } = receive.recv().unwrap()
            else {
                panic!("capture expected");
            };
            assert_eq!(page, 12);
            assert_eq!(name, "报告.pdf");
            assert!(bytes.starts_with(b"\x89PNG"));
            reply.send(Err("draft full".into())).unwrap();
        });
        let response = router
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/secret/capture/12")
                    .header("Origin", "http://127.0.0.1:1234")
                    .body(axum::body::Body::from(png))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        responder.join().unwrap();
    }

    #[test]
    fn pdf_links_resolve_only_local_documents_and_assets_are_sorted() {
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(
            local_pdf_path("report.pdf", directory.path().to_str()),
            Some(directory.path().join("report.pdf"))
        );
        let url = reqwest::Url::from_file_path(directory.path().join("报告 1.PDF")).unwrap();
        assert_eq!(
            local_pdf_path(url.as_str(), None),
            Some(directory.path().join("报告 1.PDF"))
        );
        assert!(local_pdf_path("https://example.test/file.pdf", None).is_none());
        assert!(local_pdf_path("README.md", directory.path().to_str()).is_none());
        assert!(ASSETS.windows(2).all(|pair| pair[0].0 < pair[1].0));
        assert!(
            ASSETS
                .iter()
                .any(|(path, _)| *path == "vendor/pdf.worker.mjs")
        );
    }
}
