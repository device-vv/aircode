mod document;
mod text_input;
mod theme;
mod workspace;

use gpui::{
    App, AppContext, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size,
};
use rfd::{MessageButtons, MessageDialog, MessageDialogResult};
use workspace::Workspace;

pub fn run() {
    Application::new().run(|cx: &mut App| {
        document::register_actions(cx);
        text_input::register_actions(cx);
        workspace::register_actions(cx);
        let bounds = Bounds::centered(None, size(px(1440.0), px(900.0)), cx);
        let window = cx
            .open_window(
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
                |window, cx| {
                    let workspace = cx.new(Workspace::new);
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
                    if !workspace.read(cx).has_unsaved_changes(cx) {
                        return true;
                    }
                    MessageDialog::new()
                        .set_title("Unsaved changes")
                        .set_description("Close AIRcode and discard unsaved changes? Choose Yes to close or No to keep editing.")
                        .set_buttons(MessageButtons::YesNo)
                        .show()
                        == MessageDialogResult::Yes
                });
            })
            .expect("failed to configure close confirmation");
        cx.activate(true);
    });
}
