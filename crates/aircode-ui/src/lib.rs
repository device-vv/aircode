mod theme;
mod workspace;

use gpui::{
    App, AppContext, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size,
};
use workspace::Workspace;

pub fn run() {
    Application::new().run(|cx: &mut App| {
        workspace::register_actions(cx);
        let bounds = Bounds::centered(None, size(px(1440.0), px(900.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(960.0), px(640.0))),
                titlebar: Some(TitlebarOptions {
                    title: Some("AIRcode".into()),
                    appears_transparent: false,
                    traffic_light_position: None,
                }),
                app_id: Some("dev.aircode.desktop".into()),
                ..Default::default()
            },
            |_, cx| cx.new(Workspace::new),
        )
        .expect("failed to open AIRcode window");
        cx.activate(true);
    });
}
