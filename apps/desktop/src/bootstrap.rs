use crate::{
    infrastructure::{runner_client::RunnerClient, storage::Storage},
    presenter::{Presenter, RunnerPort},
    view::{NexusView, theme},
};
use gpui::{
    App, AppContext as _, Application, Bounds, TitlebarOptions, WindowBackgroundAppearance,
    WindowBounds, WindowOptions, point, px, size,
};
use gpui_component::Root;
use std::path::Path;

pub(crate) const RUNNER_MODE_ARG: &str = "--nexus-runner";

fn create_presenter() -> Presenter {
    let (storage, storage_error) = match Storage::open_default() {
        Ok(storage) => (storage, None),
        Err(error) => (
            Storage::open(Path::new(":memory:")).expect("open fallback database"),
            Some(format!(
                "无法打开本地数据库，历史记录仅在本次运行有效：{error}"
            )),
        ),
    };

    let runner = RunnerClient::spawn().map(|runner| Box::new(runner) as Box<dyn RunnerPort>);
    Presenter::new(storage, runner, storage_error)
}

pub(crate) fn run() -> anyhow::Result<()> {
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == RUNNER_MODE_ARG)
    {
        return nexus_runner::run();
    }

    Application::new().run(|cx: &mut App| {
        gpui_component::init(cx);
        theme::configure_theme(cx);
        let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
        cx.spawn(async move |cx| {
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: None,
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(18.), px(18.))),
                }),
                window_background: WindowBackgroundAppearance::Blurred,
                window_min_size: Some(size(px(1_040.), px(680.))),
                ..Default::default()
            };
            cx.open_window(options, |window, cx| {
                let presenter = create_presenter();
                let view = cx.new(|cx| NexusView::new(presenter, window, cx));
                cx.new(|cx| Root::new(view, window, cx))
            })?;
            Ok::<_, anyhow::Error>(())
        })
        .detach();
    });
    Ok(())
}
