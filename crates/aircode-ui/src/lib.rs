mod document;
mod text_input;
mod theme;
mod workspace;

use std::path::PathBuf;

use gpui::{
    App, AppContext, Application, Bounds, TitlebarOptions, WindowBackgroundAppearance,
    WindowBounds, WindowDecorations, WindowOptions, px, size,
};
use workspace::Workspace;

pub fn run() {
    Application::new().run(|cx: &mut App| {
        document::register_actions(cx);
        text_input::register_actions(cx);
        workspace::register_actions(cx);
        let project_path = std::env::args_os()
            .skip(1)
            .find_map(|argument| {
                if argument == "--project" {
                    std::env::args_os().nth(2)
                } else {
                    None
                }
            })
            .map(PathBuf::from);
        let bounds = Bounds::centered(None, size(px(1440.0), px(900.0)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(960.0), px(640.0))),
                    window_background: WindowBackgroundAppearance::Opaque,
                    window_decorations: Some(WindowDecorations::Client),
                    titlebar: Some(TitlebarOptions {
                        title: Some("AIRcode".into()),
                        appears_transparent: true,
                        traffic_light_position: None,
                    }),
                    app_id: Some("dev.aircode.desktop".into()),
                    ..Default::default()
                },
                |window, cx| {
                    let workspace = cx.new(Workspace::new);
                    if let Some(path) = project_path.clone() {
                        workspace.update(cx, |this, cx| this.open_project_path(path, cx));
                    }
                    workspace.update(cx, |this, cx| this.focus_document(window, cx));
                    workspace
                },
            )
            .expect("failed to open AIRcode window");
        let workspace = window
            .update(cx, |_, _, cx| cx.entity())
            .expect("failed to access AIRcode workspace");
        workspace::register_global_actions(workspace.clone(), cx);
        window
            .update(cx, |_, window, cx| {
                window.on_window_should_close(cx, move |_, cx| {
                    workspace.update(cx, |workspace, cx| workspace.should_close(cx))
                });
            })
            .expect("failed to configure close confirmation");
        cx.activate(true);
    });
}
