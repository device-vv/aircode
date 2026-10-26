use aircode_core::{NavigationPanel, ThemeMode, WorkspaceState};
use gpui::{
    App, Context, FocusHandle, Focusable, FontWeight, KeyBinding, Rgba, Window, actions, div,
    prelude::*, px,
};

use crate::theme::Theme;

actions!(
    aircode,
    [
        ClosePalette,
        ToggleAi,
        ToggleFocus,
        TogglePalette,
        ToggleTerminal,
        ToggleTheme,
    ]
);

pub fn register_actions(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("escape", ClosePalette, None),
        KeyBinding::new("cmd-p", TogglePalette, None),
        KeyBinding::new("cmd-j", ToggleTerminal, None),
        KeyBinding::new("cmd-shift-i", ToggleAi, None),
        KeyBinding::new("cmd-shift-f", ToggleFocus, None),
        KeyBinding::new("cmd-shift-t", ToggleTheme, None),
    ]);
}

pub struct Workspace {
    state: WorkspaceState,
    focus_handle: FocusHandle,
}

#[derive(Clone, Copy)]
enum PaletteCommand {
    Focus,
    Ai,
    Terminal,
    Theme,
}

impl Workspace {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            state: WorkspaceState::default(),
            focus_handle: cx.focus_handle(),
        }
    }

    fn toggle_palette(&mut self, _: &TogglePalette, _: &mut Window, cx: &mut Context<Self>) {
        self.state.toggle_palette();
        cx.notify();
    }

    fn close_palette(&mut self, _: &ClosePalette, _: &mut Window, cx: &mut Context<Self>) {
        self.state.close_palette();
        cx.notify();
    }

    fn toggle_terminal(&mut self, _: &ToggleTerminal, _: &mut Window, cx: &mut Context<Self>) {
        self.state.toggle_terminal();
        cx.notify();
    }

    fn toggle_ai(&mut self, _: &ToggleAi, _: &mut Window, cx: &mut Context<Self>) {
        self.state.toggle_ai();
        cx.notify();
    }

    fn toggle_focus(&mut self, _: &ToggleFocus, _: &mut Window, cx: &mut Context<Self>) {
        self.state.toggle_focus_mode();
        cx.notify();
    }

    fn toggle_theme(&mut self, _: &ToggleTheme, _: &mut Window, cx: &mut Context<Self>) {
        self.state.toggle_theme();
        cx.notify();
    }

    fn header(&self, theme: Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let command = div()
            .id("command-palette-button")
            .flex()
            .items_center()
            .justify_between()
            .w(px(420.0))
            .h(px(32.0))
            .px_3()
            .rounded_md()
            .border_1()
            .border_color(theme.border)
            .bg(theme.raised)
            .text_color(theme.muted)
            .text_sm()
            .cursor_pointer()
            .hover(|style| style.border_color(theme.accent).text_color(theme.text))
            .on_click(cx.listener(|this, _, _, cx| {
                this.state.toggle_palette();
                cx.notify();
            }))
            .child("Search files, symbols, or commands")
            .child(keycap("Ctrl P", theme));

        div()
            .flex()
            .items_center()
            .justify_between()
            .h(px(54.0))
            .px_4()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.surface)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(28.0))
                            .rounded_md()
                            .bg(theme.accent)
                            .text_color(gpui::white())
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("A"),
                    )
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap_2()
                            .child(
                                div()
                                    .text_color(theme.text)
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("AIRcode"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.faint)
                                    .child("0.1 prototype"),
                            ),
                    )
                    .child(div().w(px(16.0)))
                    .child(div().text_sm().text_color(theme.muted).child("aircode"))
                    .child(div().text_color(theme.faint).child("/"))
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.text)
                            .child("crates/aircode-ui"),
                    ),
            )
            .child(command)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        icon_button(
                            "TH",
                            "theme-button",
                            self.state.theme == ThemeMode::Light,
                            theme,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.state.toggle_theme();
                            cx.notify();
                        })),
                    )
                    .child(
                        icon_button("AI", "ai-button", self.state.ai_visible(), theme).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.state.toggle_ai();
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        icon_button("FO", "focus-button", self.state.focus_mode, theme).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.state.toggle_focus_mode();
                                cx.notify();
                            }),
                        ),
                    ),
            )
    }

    fn activity_rail(&self, theme: Theme, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .items_center()
            .w(px(52.0))
            .py_3()
            .gap_2()
            .border_r_1()
            .border_color(theme.border)
            .bg(theme.surface)
            .child(self.rail_button("PR", NavigationPanel::Project, theme, cx))
            .child(self.rail_button("SE", NavigationPanel::Search, theme, cx))
            .child(self.rail_button("GT", NavigationPanel::Git, theme, cx))
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(30.0))
                    .rounded_full()
                    .bg(theme.accent_soft)
                    .text_color(theme.accent)
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("AC"),
            )
    }

    fn rail_button(
        &self,
        label: &'static str,
        panel: NavigationPanel,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let active = self.state.navigation() == Some(panel);
        let id = match panel {
            NavigationPanel::Project => "project-button",
            NavigationPanel::Search => "search-button",
            NavigationPanel::Git => "git-button",
        };
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .size(px(34.0))
            .rounded_md()
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .cursor_pointer()
            .when(active, |this| {
                this.bg(theme.accent_soft).text_color(theme.accent)
            })
            .when(!active, |this| {
                this.text_color(theme.muted)
                    .hover(|style| style.bg(theme.hover).text_color(theme.text))
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.state.toggle_navigation(panel);
                cx.notify();
            }))
            .child(label)
    }

    fn navigation_panel(&self, theme: Theme) -> impl IntoElement {
        let (title, subtitle) = match self.state.navigation() {
            Some(NavigationPanel::Search) => ("Search", "Find across the workspace"),
            Some(NavigationPanel::Git) => ("Source control", "Working tree is clean"),
            _ => ("Explorer", "AIRCODE"),
        };

        let content: Vec<gpui::AnyElement> = match self.state.navigation() {
            Some(NavigationPanel::Search) => vec![
                search_result(
                    "WorkspaceState",
                    "aircode-core/src/lib.rs",
                    "12 matches",
                    theme,
                )
                .into_any_element(),
                search_result(
                    "Focus Workspace",
                    "aircode-ui/src/workspace.rs",
                    "8 matches",
                    theme,
                )
                .into_any_element(),
                search_result("Theme", "aircode-ui/src/theme.rs", "3 matches", theme)
                    .into_any_element(),
            ],
            Some(NavigationPanel::Git) => vec![
                change_row("M", "workspace.rs", "UI prototype", theme).into_any_element(),
                change_row("A", "theme.rs", "Design tokens", theme).into_any_element(),
                change_row("A", "lib.rs", "Workspace state", theme).into_any_element(),
            ],
            _ => vec![
                file_row("v", "crates", 0, true, theme).into_any_element(),
                file_row("v", "aircode-ui", 1, true, theme).into_any_element(),
                file_row("R", "workspace.rs", 2, false, theme).into_any_element(),
                file_row("R", "theme.rs", 2, false, theme).into_any_element(),
                file_row("R", "lib.rs", 2, false, theme).into_any_element(),
                file_row(">", "aircode-core", 1, false, theme).into_any_element(),
                file_row(">", "aircode-app", 1, false, theme).into_any_element(),
                file_row("T", "Cargo.toml", 0, false, theme).into_any_element(),
            ],
        };

        div()
            .flex()
            .flex_col()
            .w(px(238.0))
            .border_r_1()
            .border_color(theme.border)
            .bg(theme.surface)
            .child(panel_title(title, subtitle, theme))
            .child(div().flex().flex_col().gap(px(2.0)).p_2().children(content))
    }

    fn editor(&self, theme: Theme) -> impl IntoElement {
        let lines = code_lines(theme);
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .bg(theme.window)
            .child(
                div()
                    .flex()
                    .items_end()
                    .h(px(40.0))
                    .px_3()
                    .gap_1()
                    .border_b_1()
                    .border_color(theme.border)
                    .bg(theme.surface)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .h_full()
                            .px_3()
                            .border_b_2()
                            .border_color(theme.accent)
                            .text_sm()
                            .text_color(theme.text)
                            .child(div().text_color(theme.purple).child("R"))
                            .child("workspace.rs")
                            .child(div().text_color(theme.faint).child("x")),
                    )
                    .child(
                        div()
                            .h(px(32.0))
                            .px_3()
                            .flex()
                            .items_center()
                            .text_sm()
                            .text_color(theme.muted)
                            .child("theme.rs"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(34.0))
                    .px_5()
                    .text_xs()
                    .text_color(theme.faint)
                    .child("aircode-ui")
                    .child(">")
                    .child("src")
                    .child(">")
                    .child(div().text_color(theme.muted).child("workspace.rs"))
                    .child(">")
                    .child(div().text_color(theme.muted).child("Workspace")),
            )
            .child(
                div()
                    .id("editor-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .py_3()
                    .font_family("monospace")
                    .text_sm()
                    .children(lines.into_iter().enumerate().map(|(index, segments)| {
                        code_line(index + 1, segments, theme).into_any_element()
                    })),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(28.0))
                    .px_5()
                    .text_xs()
                    .text_color(theme.faint)
                    .child(div().size(px(5.0)).rounded_full().bg(theme.green))
                    .child("Tree-sitter ready")
                    .child(div().text_color(theme.border).child("|"))
                    .child("Rust Analyzer connected"),
            )
    }

    fn assistant_panel(&self, theme: Theme) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .w(px(326.0))
            .border_l_1()
            .border_color(theme.border)
            .bg(theme.surface)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .h(px(48.0))
                    .px_4()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(div().font_weight(FontWeight::SEMIBOLD).text_color(theme.text).child("AIR assistant"))
                    .child(badge("DeepSeek", theme.accent_soft, theme.accent)),
            )
            .child(
                div()
                    .id("assistant-scroll")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .gap_4()
                    .p_4()
                    .overflow_y_scroll()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(div().text_xs().text_color(theme.faint).child("CONTEXT"))
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(badge("workspace.rs", theme.raised, theme.muted))
                                    .child(badge("32 lines", theme.raised, theme.muted)),
                            ),
                    )
                    .child(message("You", "Make this workspace calmer and keep the code visible.", theme))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .size(px(22.0))
                                            .rounded_md()
                                            .bg(theme.accent)
                                            .text_color(gpui::white())
                                            .text_xs()
                                            .child("A"),
                                    )
                                    .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(theme.muted).child("AIR")),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .line_height(px(21.0))
                                    .text_color(theme.text)
                                    .child("The editor remains the primary surface. Secondary tools now stay hidden until needed, while Focus mode clears the entire workspace."),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .p_3()
                                    .rounded_lg()
                                    .border_1()
                                    .border_color(theme.border)
                                    .bg(theme.window)
                                    .child(div().text_xs().text_color(theme.muted).child("SUGGESTED CHANGE"))
                                    .child(div().text_sm().text_color(theme.green).child("+ Focus mode: hide secondary panels"))
                                    .child(div().text_sm().text_color(theme.red).child("- Persistent tool windows"))
                                    .child(
                                        div()
                                            .flex()
                                            .gap_2()
                                            .pt_1()
                                            .child(badge("Review diff", theme.accent_soft, theme.accent))
                                            .child(badge("Dismiss", theme.raised, theme.muted)),
                                    ),
                            ),
                    ),
            )
            .child(
                div()
                    .m_3()
                    .p_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.window)
                    .child(div().text_sm().text_color(theme.faint).child("Ask about your code..."))
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .items_center()
                            .pt_3()
                            .child(div().text_xs().text_color(theme.faint).child("+ Add context"))
                            .child(badge("Send", theme.accent, gpui::white().into())),
                    ),
            )
    }

    fn bottom_panel(&self, theme: Theme) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .h(px(174.0))
            .border_t_1()
            .border_color(theme.border)
            .bg(theme.surface)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_5()
                    .h(px(34.0))
                    .px_4()
                    .border_b_1()
                    .border_color(theme.border)
                    .text_xs()
                    .child(
                        div()
                            .text_color(theme.text)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Terminal"),
                    )
                    .child(div().text_color(theme.muted).child("Problems  0"))
                    .child(div().text_color(theme.muted).child("Output"))
                    .child(div().flex_1())
                    .child(div().text_color(theme.faint).child("PowerShell")),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_4()
                    .font_family("monospace")
                    .text_xs()
                    .child(div().text_color(theme.muted).child("D:/Projects/code-vibe"))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(div().text_color(theme.green).child(">"))
                            .child(div().text_color(theme.text).child("cargo test --workspace")),
                    )
                    .child(
                        div()
                            .text_color(theme.green)
                            .child("All checks passed. Ready to build."),
                    ),
            )
    }

    fn status_bar(&self, theme: Theme) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .h(px(26.0))
            .px_3()
            .gap_4()
            .border_t_1()
            .border_color(theme.border)
            .bg(theme.surface)
            .text_xs()
            .text_color(theme.muted)
            .child(div().text_color(theme.accent).child("main*"))
            .child("0 errors  0 warnings")
            .child(div().flex_1())
            .child(if self.state.focus_mode {
                "Focus mode"
            } else {
                "Ln 24, Col 18"
            })
            .child("UTF-8")
            .child("Rust")
    }

    fn command_palette(&self, theme: Theme, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .absolute()
            .top(px(72.0))
            .left_0()
            .right_0()
            .mx_auto()
            .w(px(620.0))
            .rounded_xl()
            .border_1()
            .border_color(theme.border)
            .bg(theme.surface)
            .shadow_lg()
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(px(52.0))
                    .px_4()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(div().text_color(theme.accent).pr_3().child(">"))
                    .child(
                        div()
                            .text_color(theme.faint)
                            .child("Type a command or search..."),
                    )
                    .child(div().flex_1())
                    .child(keycap("ESC", theme)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .child(command_row(
                        "Focus: toggle distraction-free mode",
                        "Ctrl Shift F",
                        PaletteCommand::Focus,
                        true,
                        theme,
                        cx,
                    ))
                    .child(command_row(
                        "AI: toggle assistant panel",
                        "Ctrl Shift I",
                        PaletteCommand::Ai,
                        false,
                        theme,
                        cx,
                    ))
                    .child(command_row(
                        "Terminal: toggle bottom panel",
                        "Ctrl J",
                        PaletteCommand::Terminal,
                        false,
                        theme,
                        cx,
                    ))
                    .child(command_row(
                        "Theme: switch light / dark",
                        "Ctrl Shift T",
                        PaletteCommand::Theme,
                        false,
                        theme,
                        cx,
                    )),
            )
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::from_mode(self.state.theme);
        div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .overflow_hidden()
            .bg(theme.window)
            .text_color(theme.text)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::toggle_palette))
            .on_action(cx.listener(Self::close_palette))
            .on_action(cx.listener(Self::toggle_terminal))
            .on_action(cx.listener(Self::toggle_ai))
            .on_action(cx.listener(Self::toggle_focus))
            .on_action(cx.listener(Self::toggle_theme))
            .child(self.header(theme, cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h(px(0.0))
                    .when(!self.state.focus_mode, |this| {
                        this.child(self.activity_rail(theme, cx))
                    })
                    .when(self.state.navigation_visible(), |this| {
                        this.child(self.navigation_panel(theme))
                    })
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w(px(0.0))
                            .child(self.editor(theme))
                            .when(self.state.terminal_visible(), |this| {
                                this.child(self.bottom_panel(theme))
                            }),
                    )
                    .when(self.state.ai_visible(), |this| {
                        this.child(self.assistant_panel(theme))
                    }),
            )
            .child(self.status_bar(theme))
            .when(self.state.palette_open, |this| {
                this.child(self.command_palette(theme, cx))
            })
    }
}

fn icon_button(
    label: &'static str,
    id: &'static str,
    active: bool,
    theme: Theme,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .size(px(30.0))
        .rounded_md()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .when(active, |this| {
            this.bg(theme.accent_soft).text_color(theme.accent)
        })
        .when(!active, |this| {
            this.text_color(theme.muted)
                .hover(|style| style.bg(theme.hover).text_color(theme.text))
        })
        .child(label)
}

fn keycap(label: &'static str, theme: Theme) -> impl IntoElement {
    div()
        .px(px(6.0))
        .py(px(2.0))
        .rounded(px(4.0))
        .border_1()
        .border_color(theme.border)
        .bg(theme.surface)
        .text_xs()
        .text_color(theme.faint)
        .child(label)
}

fn badge(text: &'static str, background: Rgba, foreground: Rgba) -> impl IntoElement {
    div()
        .px_2()
        .py_1()
        .rounded_md()
        .bg(background)
        .text_xs()
        .text_color(foreground)
        .child(text)
}

fn panel_title(title: &'static str, subtitle: &'static str, theme: Theme) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .h(px(64.0))
        .px_4()
        .pt_3()
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.muted)
                .child(title.to_uppercase()),
        )
        .child(div().text_xs().text_color(theme.faint).child(subtitle))
}

fn file_row(
    icon: &'static str,
    name: &'static str,
    depth: usize,
    expanded: bool,
    theme: Theme,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap_2()
        .h(px(28.0))
        .pr_2()
        .pl(px(8.0 + depth as f32 * 14.0))
        .rounded_md()
        .text_sm()
        .when(name == "workspace.rs", |this| this.bg(theme.accent_soft))
        .child(
            div()
                .w(px(14.0))
                .text_xs()
                .text_color(if expanded { theme.accent } else { theme.faint })
                .child(icon),
        )
        .child(
            div()
                .text_color(if name == "workspace.rs" {
                    theme.accent
                } else {
                    theme.muted
                })
                .child(name),
        )
}

fn search_result(
    title: &'static str,
    path: &'static str,
    count: &'static str,
    theme: Theme,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .p_2()
        .rounded_md()
        .bg(theme.raised)
        .child(
            div()
                .flex()
                .justify_between()
                .child(div().text_sm().text_color(theme.text).child(title))
                .child(div().text_xs().text_color(theme.accent).child(count)),
        )
        .child(div().text_xs().text_color(theme.faint).child(path))
}

fn change_row(
    status: &'static str,
    name: &'static str,
    detail: &'static str,
    theme: Theme,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap_3()
        .p_2()
        .rounded_md()
        .hover(|style| style.bg(theme.hover))
        .child(div().text_xs().text_color(theme.yellow).child(status))
        .child(
            div()
                .flex()
                .flex_col()
                .child(div().text_sm().text_color(theme.text).child(name))
                .child(div().text_xs().text_color(theme.faint).child(detail)),
        )
}

fn code_line(number: usize, segments: Vec<(&'static str, Rgba)>, theme: Theme) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .h(px(23.0))
        .when(number == 24, |this| this.bg(theme.raised))
        .child(
            div()
                .w(px(54.0))
                .pr_4()
                .text_right()
                .text_color(if number == 24 {
                    theme.accent
                } else {
                    theme.faint
                })
                .child(number.to_string()),
        )
        .child(
            div().flex().whitespace_nowrap().children(
                segments
                    .into_iter()
                    .map(|(text, color)| div().text_color(color).child(text).into_any_element()),
            ),
        )
}

fn code_lines(theme: Theme) -> Vec<Vec<(&'static str, Rgba)>> {
    vec![
        vec![
            ("use ", theme.purple),
            ("aircode_core", theme.blue),
            ("::{NavigationPanel, WorkspaceState};", theme.text),
        ],
        vec![
            ("use ", theme.purple),
            ("gpui", theme.blue),
            ("::{Context, FocusHandle, Window};", theme.text),
        ],
        vec![],
        vec![
            ("pub struct ", theme.purple),
            ("Workspace", theme.yellow),
            (" {", theme.text),
        ],
        vec![("    state", theme.blue), (": WorkspaceState,", theme.text)],
        vec![
            ("    focus_handle", theme.blue),
            (": FocusHandle,", theme.text),
        ],
        vec![("}", theme.text)],
        vec![],
        vec![
            ("impl ", theme.purple),
            ("Workspace", theme.yellow),
            (" {", theme.text),
        ],
        vec![
            ("    pub fn ", theme.purple),
            ("new", theme.blue),
            ("(cx: &mut Context<Self>) -> Self {", theme.text),
        ],
        vec![("        Self {", theme.text)],
        vec![("            state: WorkspaceState::default(),", theme.text)],
        vec![("            focus_handle: cx.focus_handle(),", theme.text)],
        vec![("        }", theme.text)],
        vec![("    }", theme.text)],
        vec![],
        vec![
            ("    fn ", theme.purple),
            ("toggle_focus", theme.blue),
            ("(&mut self, cx: &mut Context<Self>) {", theme.text),
        ],
        vec![("        self.state.toggle_focus_mode();", theme.text)],
        vec![("        cx.notify();", theme.text)],
        vec![("    }", theme.text)],
        vec![],
        vec![
            ("    fn ", theme.purple),
            ("render_workspace", theme.blue),
            ("(&self) -> WorkspaceLayout {", theme.text),
        ],
        vec![
            ("        ", theme.text),
            (
                "// Keep the editor at the center of attention.",
                theme.faint,
            ),
        ],
        vec![("        WorkspaceLayout::focused()", theme.text)],
        vec![("    }", theme.text)],
        vec![("}", theme.text)],
    ]
}

fn message(author: &'static str, text: &'static str, theme: Theme) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.muted)
                .child(author),
        )
        .child(
            div()
                .p_3()
                .rounded_lg()
                .bg(theme.raised)
                .text_sm()
                .line_height(px(20.0))
                .text_color(theme.text)
                .child(text),
        )
}

fn command_row(
    label: &'static str,
    shortcut: &'static str,
    command: PaletteCommand,
    selected: bool,
    theme: Theme,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    div()
        .id(label)
        .flex()
        .items_center()
        .h(px(38.0))
        .px_3()
        .rounded_md()
        .cursor_pointer()
        .when(selected, |this| this.bg(theme.accent_soft))
        .when(!selected, |this| this.hover(|style| style.bg(theme.hover)))
        .on_click(cx.listener(move |this, _, _, cx| {
            match command {
                PaletteCommand::Focus => this.state.toggle_focus_mode(),
                PaletteCommand::Ai => this.state.toggle_ai(),
                PaletteCommand::Terminal => this.state.toggle_terminal(),
                PaletteCommand::Theme => this.state.toggle_theme(),
            }
            this.state.close_palette();
            cx.notify();
        }))
        .child(div().text_sm().text_color(theme.text).child(label))
        .child(div().flex_1())
        .child(keycap(shortcut, theme))
}
