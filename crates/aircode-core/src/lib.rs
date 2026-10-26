#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NavigationPanel {
    Project,
    Search,
    Git,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThemeMode {
    Dark,
    Light,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceState {
    navigation: Option<NavigationPanel>,
    pub ai_open: bool,
    pub terminal_open: bool,
    pub palette_open: bool,
    pub focus_mode: bool,
    pub theme: ThemeMode,
}

impl Default for WorkspaceState {
    fn default() -> Self {
        Self {
            navigation: Some(NavigationPanel::Project),
            ai_open: true,
            terminal_open: true,
            palette_open: false,
            focus_mode: false,
            theme: ThemeMode::Dark,
        }
    }
}

impl WorkspaceState {
    pub fn navigation(&self) -> Option<NavigationPanel> {
        self.navigation
    }

    pub fn navigation_visible(&self) -> bool {
        !self.focus_mode && self.navigation.is_some()
    }

    pub fn ai_visible(&self) -> bool {
        !self.focus_mode && self.ai_open
    }

    pub fn terminal_visible(&self) -> bool {
        !self.focus_mode && self.terminal_open
    }

    pub fn toggle_navigation(&mut self, panel: NavigationPanel) {
        self.navigation = if self.navigation == Some(panel) {
            None
        } else {
            Some(panel)
        };
    }

    pub fn toggle_ai(&mut self) {
        self.ai_open = !self.ai_open;
        self.focus_mode = false;
    }

    pub fn toggle_terminal(&mut self) {
        self.terminal_open = !self.terminal_open;
        self.focus_mode = false;
    }

    pub fn toggle_palette(&mut self) {
        self.palette_open = !self.palette_open;
    }

    pub fn close_palette(&mut self) {
        self.palette_open = false;
    }

    pub fn toggle_focus_mode(&mut self) {
        self.focus_mode = !self.focus_mode;
        self.palette_open = false;
    }

    pub fn toggle_theme(&mut self) {
        self.theme = match self.theme {
            ThemeMode::Dark => ThemeMode::Light,
            ThemeMode::Light => ThemeMode::Dark,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selecting_active_navigation_panel_hides_it() {
        let mut state = WorkspaceState::default();

        state.toggle_navigation(NavigationPanel::Project);

        assert_eq!(state.navigation(), None);
        assert!(!state.navigation_visible());
    }

    #[test]
    fn selecting_another_navigation_panel_replaces_the_current_one() {
        let mut state = WorkspaceState::default();

        state.toggle_navigation(NavigationPanel::Search);

        assert_eq!(state.navigation(), Some(NavigationPanel::Search));
    }

    #[test]
    fn focus_mode_preserves_panel_state() {
        let mut state = WorkspaceState::default();

        state.toggle_focus_mode();

        assert!(!state.navigation_visible());
        assert!(!state.ai_visible());
        assert!(!state.terminal_visible());

        state.toggle_focus_mode();

        assert!(state.navigation_visible());
        assert!(state.ai_visible());
        assert!(state.terminal_visible());
    }

    #[test]
    fn opening_a_tool_leaves_focus_mode() {
        let mut state = WorkspaceState {
            ai_open: false,
            focus_mode: true,
            ..WorkspaceState::default()
        };

        state.toggle_ai();

        assert!(!state.focus_mode);
        assert!(state.ai_visible());
    }

    #[test]
    fn focus_mode_closes_command_palette() {
        let mut state = WorkspaceState {
            palette_open: true,
            ..WorkspaceState::default()
        };

        state.toggle_focus_mode();

        assert!(!state.palette_open);
    }

    #[test]
    fn theme_switches_both_ways() {
        let mut state = WorkspaceState::default();

        state.toggle_theme();
        assert_eq!(state.theme, ThemeMode::Light);

        state.toggle_theme();
        assert_eq!(state.theme, ThemeMode::Dark);
    }
}
