use super::*;
use crate::infrastructure::pdf::{PdfEvent, PdfServer, local_pdf_path};
use std::{path::PathBuf, sync::atomic::Ordering};

impl NexusView {
    pub(super) fn choose_pdf(&mut self, cx: &mut Context<Self>) {
        let task = reqwest_client::runtime().spawn_blocking(|| {
            rfd::FileDialog::new()
                .add_filter("PDF", &["pdf"])
                .pick_file()
        });
        cx.spawn(async move |view, cx| {
            if let Ok(Some(path)) = task.await {
                let _ = view.update(cx, |view, cx| view.open_pdf(path, cx));
            }
        })
        .detach();
    }

    pub(super) fn open_pdf(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let language = self.presenter.model().language;
        let events = self.pdf_events.0.clone();
        let task = reqwest_client::runtime()
            .spawn_blocking(move || PdfServer::open(&path, language, events));
        cx.spawn(async move |view, cx| {
            let result = task
                .await
                .map_err(anyhow::Error::from)
                .and_then(|result| result);
            let _ = view.update(cx, |view, cx| {
                match result {
                    Ok(server) => open_window(server, cx),
                    Err(error) => view.presenter.report_pdf_error(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn poll_pdf_events(&mut self, cx: &mut Context<Self>) {
        while let Ok(event) = self.pdf_events.1.try_recv() {
            match event {
                PdfEvent::Capture {
                    name,
                    page,
                    bytes,
                    reply,
                } => {
                    if reply.is_closed() {
                        continue;
                    }
                    let result = self
                        .presenter
                        .attach_pdf_capture(&name, page, &bytes)
                        .map_err(|error| error.to_string());
                    if let Err(error) = &result {
                        self.presenter.report_pdf_error(error.clone());
                    }
                    let _ = reply.send(result);
                }
                PdfEvent::Error(error) => self.presenter.report_pdf_error(error),
            }
            cx.notify();
        }
    }

    pub(super) fn pdf_link_handler(
        &self,
        cx: &Context<Self>,
    ) -> impl Fn(&SharedString, &gpui::ClickEvent, &mut Window, &mut gpui::App) + Send + Sync + 'static
    {
        let view = cx.entity().downgrade();
        let directory = self
            .presenter
            .model()
            .working_directory()
            .map(str::to_owned);
        move |link, _, _, cx| {
            if let Some(path) = local_pdf_path(link, directory.as_deref()) {
                let _ = view.update(cx, |view, cx| view.open_pdf(path, cx));
            } else {
                cx.open_url(link);
            }
        }
    }
}

fn builder<'a>(server: &PdfServer) -> wry::WebViewBuilder<'a> {
    let allowed = server.url.clone();
    let can_close = server.can_close.clone();
    wry::WebViewBuilder::new()
        .with_url(server.url.clone())
        .with_incognito(true)
        .with_devtools(false)
        .with_navigation_handler(move |url| {
            url == allowed
                || url
                    .strip_prefix(&allowed)
                    .is_some_and(|suffix| suffix.starts_with('#'))
        })
        .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny)
        .with_ipc_handler(move |request| {
            #[derive(serde::Deserialize)]
            struct State {
                dirty: bool,
                busy: bool,
            }
            if let Ok(state) = serde_json::from_str::<State>(request.body()) {
                can_close.store(!state.dirty && !state.busy, Ordering::Release);
            }
        })
}

fn open_window(server: PdfServer, cx: &mut gpui::App) {
    let events = server.events.clone();
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    let result = {
        let bounds = gpui::Bounds::centered(None, gpui::size(px(1060.), px(800.)), cx);
        let title = format!("{} · Nexus PDF", server.name);
        cx.open_window(
            gpui::WindowOptions {
                window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some(title.into()),
                    ..Default::default()
                }),
                window_min_size: Some(gpui::size(px(680.), px(480.))),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| PdfWindow::new(server, window, cx)),
        )
        .map(|_| ())
        .map_err(|error| error.to_string())
    };
    #[cfg(target_os = "linux")]
    let result = {
        let _ = cx;
        linux::open(server)
    };
    if let Err(error) = result {
        let _ = events.send(PdfEvent::Error(error));
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
struct PdfWindow {
    server: PdfServer,
    webview: Result<std::rc::Rc<wry::WebView>, String>,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl PdfWindow {
    fn new(server: PdfServer, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let webview = builder(&server)
            .build_as_child(window)
            .map(std::rc::Rc::new)
            .map_err(|error| error.to_string());
        let weak = cx.entity().downgrade();
        window.on_window_should_close(cx, move |_, cx| {
            weak.update(cx, |view, _| {
                if view.server.can_close.load(Ordering::Acquire) || view.webview.is_err() {
                    return true;
                }
                if let Ok(webview) = &view.webview {
                    let _ = webview.evaluate_script("window.requestClose?.()");
                }
                false
            })
            .unwrap_or(true)
        });
        let closed = server.close.clone();
        cx.spawn_in(window, async move |_, cx| {
            loop {
                smol::Timer::after(Duration::from_millis(100)).await;
                if closed.load(Ordering::Acquire) {
                    let _ = cx.update(|window, _| window.remove_window());
                    break;
                }
                if cx.update(|_, _| ()).is_err() {
                    break;
                }
            }
        })
        .detach();
        Self { server, webview }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl Render for PdfWindow {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        match &self.webview {
            Err(error) => div().p_4().child(error.clone()).into_any_element(),
            Ok(webview) => {
                let webview = webview.clone();
                div()
                    .size_full()
                    .child(
                        gpui::canvas(
                            move |bounds, _, _| {
                                let _ = webview.set_bounds(wry::Rect {
                                    position: wry::dpi::LogicalPosition::new(
                                        f64::from(bounds.origin.x),
                                        f64::from(bounds.origin.y),
                                    )
                                    .into(),
                                    size: wry::dpi::LogicalSize::new(
                                        f64::from(bounds.size.width),
                                        f64::from(bounds.size.height),
                                    )
                                    .into(),
                                });
                            },
                            |_, _, _, _| {},
                        )
                        .size_full(),
                    )
                    .into_any_element()
            }
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use gtk::prelude::*;
    use std::{
        rc::Rc,
        sync::{OnceLock, mpsc},
    };
    use wry::WebViewBuilderExtUnix as _;

    // GTK owns all its WebViews on one thread, including under Wayland. GPUI
    // retains its own event loop; the preview is another window in this process.
    pub(super) fn open(server: PdfServer) -> Result<(), String> {
        static WINDOWS: OnceLock<mpsc::Sender<PdfServer>> = OnceLock::new();
        let sender = WINDOWS.get_or_init(|| {
            let (sender, receive) = mpsc::channel::<PdfServer>();
            std::thread::spawn(move || {
                if let Err(error) = gtk::init() {
                    for server in receive {
                        let _ = server.events.send(PdfEvent::Error(error.to_string()));
                    }
                    return;
                }
                let mut windows: Vec<(gtk::Window, Rc<wry::WebView>, PdfServer)> = Vec::new();
                loop {
                    if let Ok(server) = receive.recv_timeout(Duration::from_millis(10)) {
                        let window = gtk::Window::new(gtk::WindowType::Toplevel);
                        window.set_title(&format!("{} · Nexus PDF", server.name));
                        window.set_default_size(1060, 800);
                        match builder(&server).build_gtk(&window) {
                            Ok(webview) => {
                                let webview = Rc::new(webview);
                                let view = webview.clone();
                                let can_close = server.can_close.clone();
                                let closed = server.close.clone();
                                window.connect_delete_event(move |_, _| {
                                    if closed.load(Ordering::Acquire) {
                                        return gtk::glib::Propagation::Proceed;
                                    }
                                    if can_close.load(Ordering::Acquire) {
                                        closed.store(true, Ordering::Release);
                                        return gtk::glib::Propagation::Proceed;
                                    } else {
                                        let _ = view.evaluate_script("window.requestClose?.()");
                                    }
                                    gtk::glib::Propagation::Stop
                                });
                                window.show_all();
                                windows.push((window, webview, server));
                            }
                            Err(error) => {
                                let _ = server.events.send(PdfEvent::Error(error.to_string()));
                            }
                        }
                    }
                    while gtk::events_pending() {
                        gtk::main_iteration_do(false);
                    }
                    windows.retain(|(window, _, server)| {
                        if server.close.load(Ordering::Acquire) {
                            window.close();
                            window.hide();
                            false
                        } else {
                            true
                        }
                    });
                }
            });
            sender
        });
        sender.send(server).map_err(|error| error.to_string())
    }
}
