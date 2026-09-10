use super::*;
use gpui::{Bounds, Pixels, canvas};
use std::{cell::Cell, rc::Rc};
use wry::{
    Rect, WebView, WebViewBuilder,
    dpi::{LogicalPosition, LogicalSize},
};

pub(super) struct Player {
    webview: Rc<WebView>,
    // Keep the page and authenticated attachment alive until the player closes.
    _media: LoadedMedia,
    server: tokio::task::JoinHandle<()>,
    bounds: Rc<Cell<Option<PlayerBounds>>>,
}

#[derive(Clone, Copy, PartialEq)]
struct PlayerBounds {
    full: Bounds<Pixels>,
    visible: Bounds<Pixels>,
}

#[derive(serde::Deserialize)]
struct Wheel {
    x: f32,
    y: f32,
    dx: f32,
    dy: f32,
}

impl Player {
    pub(super) fn new(
        media: LoadedMedia,
        kind: MediaKind,
        locale: Language,
        window: &Window,
        cx: &mut App,
    ) -> anyhow::Result<Self> {
        // Serve only this attachment, on loopback and under an unguessable path.
        // HTTP provides native byte-range seeking and working WebView IPC on both OSes.
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let base = reqwest::Url::parse(&format!(
            "http://{}/{}/",
            listener.local_addr()?,
            Uuid::new_v4()
        ))?;
        let router = player_router(&media, kind, locale, &base);
        let url = base.join("player")?.to_string();
        let allowed = url.clone();
        let bounds = Rc::new(Cell::new(None::<PlayerBounds>));
        let wheel_bounds = bounds.clone();
        let handle = window.window_handle();
        let async_cx = cx.to_async();
        let webview = WebViewBuilder::new()
            .with_url(url)
            .with_visible(false)
            .with_incognito(true)
            .with_devtools(false)
            .with_navigation_handler(move |url| url == allowed)
            .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny)
            .with_ipc_handler(move |request| {
                let Ok(wheel) = serde_json::from_str::<Wheel>(request.body()) else {
                    return;
                };
                let Some(bounds) = wheel_bounds.get() else {
                    return;
                };
                if ![wheel.x, wheel.y, wheel.dx, wheel.dy]
                    .into_iter()
                    .all(f32::is_finite)
                {
                    return;
                }
                let _ = handle.update(&mut async_cx.clone(), |_, window, cx| {
                    window.dispatch_event(
                        gpui::PlatformInput::ScrollWheel(gpui::ScrollWheelEvent {
                            position: bounds.visible.origin + gpui::point(px(wheel.x), px(wheel.y)),
                            delta: gpui::ScrollDelta::Pixels(gpui::point(
                                px(-wheel.dx),
                                px(-wheel.dy),
                            )),
                            modifiers: gpui::Modifiers::default(),
                            touch_phase: gpui::TouchPhase::Moved,
                        }),
                        cx,
                    );
                });
            })
            .build_as_child(window)?;
        let server = reqwest_client::runtime().spawn(async move {
            if let Ok(listener) = tokio::net::TcpListener::from_std(listener) {
                let _ = axum::serve(listener, router).await;
            }
        });
        Ok(Self {
            webview: Rc::new(webview),
            _media: media,
            server,
            bounds,
        })
    }

    pub(super) fn element(&self, kind: MediaKind) -> impl IntoElement {
        let webview = self.webview.clone();
        let last_bounds = self.bounds.clone();
        div()
            .w_full()
            .min_w_0()
            .bg(rgb(0x15181d))
            .when(kind == MediaKind::Video, |el| {
                el.aspect_ratio(16. / 9.).max_h(px(480.))
            })
            .when(kind == MediaKind::Audio, |el| el.h(px(64.)))
            .child(
                canvas(
                    move |bounds, window, _| {
                        // Native child views do not obey GPUI's clipping. Crop the view to
                        // the scroll viewport and offset its media by the clipped amount.
                        let visible = bounds.intersect(&window.content_mask().bounds);
                        let next = PlayerBounds {
                            full: bounds,
                            visible,
                        };
                        let previous = last_bounds.get();
                        if previous == Some(next) {
                            return;
                        }
                        last_bounds.set(Some(next));
                        let shown = visible.size.width > px(0.) && visible.size.height > px(0.);
                        if shown {
                            let _ = webview.set_bounds(Rect {
                                position: LogicalPosition::new(
                                    f64::from(visible.origin.x),
                                    f64::from(visible.origin.y),
                                )
                                .into(),
                                size: LogicalSize::new(
                                    f64::from(visible.size.width),
                                    f64::from(visible.size.height),
                                )
                                .into(),
                            });
                            let _ = webview.evaluate_script(&format!(
                                "window.playerBounds=[{},{},{},{}];window.layoutPlayer?.();",
                                f32::from(bounds.size.width),
                                f32::from(bounds.size.height),
                                f32::from(bounds.origin.x - visible.origin.x),
                                f32::from(bounds.origin.y - visible.origin.y),
                            ));
                        }
                        let was_shown = previous.is_some_and(|last| {
                            last.visible.size.width > px(0.) && last.visible.size.height > px(0.)
                        });
                        if shown != was_shown {
                            let _ = webview.set_visible(shown);
                        }
                    },
                    |_, _, _, _| {},
                )
                .size_full(),
            )
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.server.abort();
        let _ = self.webview.set_visible(false);
        let _ = self.webview.focus_parent();
        let _ = self.webview.evaluate_script("const m=document.querySelector('video,audio');if(m){m.pause();m.removeAttribute('src');m.load();}");
    }
}

fn player_router(
    media: &LoadedMedia,
    kind: MediaKind,
    locale: Language,
    base: &reqwest::Url,
) -> axum::Router {
    let mut router = axum::Router::new();
    let media_url = match media {
        LoadedMedia::Local(file) => {
            // WebKit rejects mime_guess's legacy audio/m4a type for an extensionless URL.
            let mime = match file
                .path
                .extension()
                .and_then(|ext| ext.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref()
            {
                Some("m4a") => Some("audio/mp4"),
                Some("m4v") => Some("video/mp4"),
                _ => None,
            };
            let service = if let Some(mime) = mime {
                tower_http::services::ServeFile::new_with_mime(
                    &file.path,
                    &mime.parse().expect("valid media MIME type"),
                )
            } else {
                tower_http::services::ServeFile::new(&file.path)
            };
            router = router.route_service(&format!("{}media", base.path()), service);
            base.join("media").expect("player media URL").to_string()
        }
        LoadedMedia::Remote(url) => url.to_string(),
    };
    let html = player_html(&media_url, kind, locale);
    router.route(
        &format!("{}player", base.path()),
        axum::routing::get(move || {
            let html = html.clone();
            async move { axum::response::Html(html) }
        }),
    )
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn player_html(url: &str, kind: MediaKind, locale: Language) -> String {
    let tag = if kind == MediaKind::Audio {
        "audio"
    } else {
        "video"
    };
    let url = escape_html(url);
    let error = escape_html(locale.text("无法播放此素材，请尝试打开原始文件。"));
    format!(
        r#"<!doctype html><html lang="{}"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; media-src https: http:; style-src 'unsafe-inline'; script-src 'unsafe-inline'">
<style>
html,body{{margin:0;width:100%;height:100%;overflow:hidden;background:#15181d;color:#eff2f7;color-scheme:dark}}
#player{{position:absolute;width:100%;height:100%}}video,audio{{display:block;width:100%;height:100%;object-fit:contain}}
#error{{position:absolute;inset:0;align-content:center;text-align:center;padding:16px;font:14px system-ui;background:#15181d}}
</style></head><body><div id="player"><{tag} controls playsinline preload="metadata" src="{url}"></{tag}><div id="error" hidden>{error}</div></div>
<script>
window.layoutPlayer=()=>{{const b=window.playerBounds||[innerWidth,innerHeight,0,0],p=document.getElementById('player');p.style.width=b[0]+'px';p.style.height=b[1]+'px';p.style.left=b[2]+'px';p.style.top=b[3]+'px'}};
layoutPlayer();const media=document.querySelector('video,audio');media.addEventListener('error',()=>document.getElementById('error').hidden=false);
addEventListener('wheel',e=>{{if(e.ctrlKey)return;const scale=e.deltaMode===1?16:e.deltaMode===2?innerHeight:1;window.ipc.postMessage(JSON.stringify({{x:e.clientX,y:e.clientY,dx:e.deltaX*scale,dy:e.deltaY*scale}}));e.preventDefault()}},{{passive:false}});
</script></body></html>"#,
        locale.as_str()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cnb_player_serves_byte_ranges_only_for_its_attachment() {
        use crate::infrastructure::cnb_media::LocalMedia;
        use tower::ServiceExt as _;
        for (filename, kind, mime) in [
            ("clip.mp4", MediaKind::Video, "video/mp4"),
            ("audio.m4a", MediaKind::Audio, "audio/mp4"),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join(filename);
            std::fs::write(&path, b"0123456789").unwrap();
            let media = LoadedMedia::Local(std::sync::Arc::new(LocalMedia {
                path,
                _directory: directory,
            }));
            let router = player_router(
                &media,
                kind,
                Language::English,
                &reqwest::Url::parse("http://127.0.0.1:1234/secret/").unwrap(),
            );
            let response = router
                .clone()
                .oneshot(
                    axum::http::Request::builder()
                        .uri("/secret/media")
                        .header("Range", "bytes=2-5")
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), axum::http::StatusCode::PARTIAL_CONTENT);
            assert_eq!(response.headers()["Content-Range"], "bytes 2-5/10");
            assert_eq!(response.headers()["Content-Type"], mime);
            assert_eq!(
                axum::body::to_bytes(response.into_body(), 10)
                    .await
                    .unwrap(),
                "2345"
            );
            let response = router
                .oneshot(
                    axum::http::Request::builder()
                        .uri("/media")
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), axum::http::StatusCode::NOT_FOUND);
        }
    }

    #[test]
    fn cnb_player_embeds_controls_and_escapes_untrusted_urls() {
        let html = player_html(
            "https://example.test/video.mp4?a=\"<script>&b=1",
            MediaKind::Video,
            Language::Chinese,
        );
        assert!(html.contains("<video controls playsinline preload=\"metadata\""));
        assert!(html.contains("&quot;&lt;script&gt;&amp;b=1"));
        assert!(!html.contains("autoplay"));
        assert!(!html.contains("<script>&b=1"));
        assert!(
            player_html(
                "https://example.test/a.mp3",
                MediaKind::Audio,
                Language::English
            )
            .contains("<audio controls")
        );
    }
}
