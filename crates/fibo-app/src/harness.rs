//! Native visual evidence: scripted layouts, pages and clicks that save what the real
//! renderer drew. Built only with the `visual-test` feature.
use super::*;
use crate::terminal::screen;
use gpui::WindowHandle;
use std::time::Duration;
mod agent_picker_evidence;
mod attach_evidence;
mod blank_pane_evidence;
mod changes_evidence;
mod complete_evidence;
#[path = "cpu_probe_evidence.rs"]
mod cpu_probe_evidence;
mod decoration_evidence;
mod diagnostic_evidence;
#[path = "first_run_evidence.rs"]
mod first_run_evidence;
mod folderless_space_evidence;
mod follow_input_evidence;
mod keymap_evidence;
mod layout_evidence;
mod link_evidence;
mod mouse_taken_evidence;
mod new_space_evidence;
mod osc7_evidence;
mod overview_evidence;
mod page_evidence;
#[path = "pane_drag_evidence.rs"]
mod pane_drag_evidence;
mod peek_evidence;
#[path = "phones_evidence.rs"]
mod phones_evidence;
#[path = "pinned_ask_evidence.rs"]
mod pinned_ask_evidence;
mod program_status_evidence;
#[path = "quiet_space_evidence.rs"]
mod quiet_space_evidence;
mod reason_evidence;
mod reconnect_evidence;
#[path = "remote_typing_evidence.rs"]
mod remote_typing_evidence;
mod rename_click_out_evidence;
mod restart_evidence;
#[path = "resume_evidence.rs"]
mod resume_evidence;
mod route_evidence;
mod scroll_keys_evidence;
mod scrollback_evidence;
mod selection_evidence;
mod selection_pixel_evidence;
#[path = "settings_evidence.rs"]
mod settings_evidence;
mod shortcut_evidence;
mod shots_evidence;
#[path = "sidebar_rows_evidence.rs"]
mod sidebar_rows_evidence;
mod space_evidence;
mod split_cli_evidence;
mod split_evidence;
#[path = "subagent_evidence.rs"]
mod subagent_evidence;
mod tab_cwd_evidence;
mod tab_evidence;
mod tab_geometry_evidence;
mod tab_overflow_evidence;
#[path = "todo_flow_evidence.rs"]
mod todo_flow_evidence;
mod wheel_evidence;
mod why_evidence;

/// Runs the evidence script the environment asks for against the opened workbench.
pub fn start(
    handle: Option<WindowHandle<workbench::Workbench>>,
    evidence: Option<PathBuf>,
    cx: &mut App,
) {
    if agent_picker_evidence::start(handle, evidence.clone(), cx)
        || first_run_evidence::start(handle, evidence.clone(), cx)
        || settings_evidence::start(handle, evidence.clone(), cx)
        || phones_evidence::start(handle, evidence.clone(), cx)
        || remote_typing_evidence::start(handle, evidence.clone(), cx)
        || attach_evidence::start(handle, evidence.clone(), cx)
        || cpu_probe_evidence::start(handle, evidence.clone(), cx)
        || pane_drag_evidence::start(handle, evidence.clone(), cx)
        || resume_evidence::start(handle, evidence.clone(), cx)
        || subagent_evidence::start(handle, evidence.clone(), cx)
        || sidebar_rows_evidence::start(handle, evidence.clone(), cx)
        || quiet_space_evidence::start(handle, evidence.clone(), cx)
        || pinned_ask_evidence::start(handle, evidence.clone(), cx)
        || todo_flow_evidence::start(handle, evidence.clone(), cx)
    {
        return;
    }
    if rename_click_out_evidence::start(handle, evidence.clone(), cx) {
        return;
    }
    if complete_evidence::start(handle, evidence.clone(), cx)
        || changes_evidence::start(handle, evidence.clone(), cx)
    {
        return;
    }
    reason_evidence::start(handle, evidence.clone(), cx);
    if split_cli_evidence::start(handle, evidence.clone(), cx) {
        return;
    }
    if route_evidence::start(handle, evidence.clone(), cx) {
        return;
    }
    if blank_pane_evidence::start(handle, evidence.clone(), cx) {
        return;
    }
    if reconnect_evidence::start(handle, evidence.clone(), cx) {
        return;
    }
    if scrollback_evidence::start(handle, evidence.clone(), cx) {
        return;
    }
    if scroll_keys_evidence::start(handle, evidence.clone(), cx) {
        return;
    }
    if selection_pixel_evidence::start(handle, evidence.clone(), cx) {
        return;
    }
    if mouse_taken_evidence::start(handle, evidence.clone(), cx) {
        return;
    }
    if selection_evidence::start(handle, evidence.clone(), cx) {
        return;
    }
    if diagnostic_evidence::start(handle, evidence.clone(), cx) {
        return;
    }
    if split_evidence::start(handle, evidence.clone(), cx) {
        return;
    }
    if osc7_evidence::start(handle, evidence.clone(), cx) {
        return;
    }
    if decoration_evidence::start(handle, evidence.clone(), cx) {
        return;
    }
    if program_status_evidence::start(handle, evidence.clone(), cx) {
        return;
    }
    if tab_cwd_evidence::start(handle, evidence.clone(), cx) {
        return;
    }
    if tab_geometry_evidence::start(handle, evidence.clone(), cx) {
        return;
    }
    if keymap_evidence::start(handle, evidence.clone(), cx) {
        return;
    }
    if shortcut_evidence::start(handle, evidence.clone(), cx) {
        return;
    }
    if tab_overflow_evidence::start(handle, evidence.clone(), cx) {
        return;
    }
    tab_evidence::start(handle, evidence.clone(), cx);
    wheel_evidence::start(handle, cx);
    if follow_input_evidence::start(handle, evidence.clone(), cx) {
        return;
    }
    if link_evidence::start(handle, evidence.clone(), cx) {
        return;
    }
    if new_space_evidence::start(handle, evidence.clone(), cx) {
        return;
    }
    if space_evidence::start(handle, evidence.clone(), cx) {
        return;
    }
    if folderless_space_evidence::start(handle, evidence.clone(), cx) {
        return;
    }
    if layout_evidence::start(handle, evidence.clone(), cx) {
        return;
    }
    if peek_evidence::start(handle, evidence.clone(), cx) {
        return;
    }
    if why_evidence::start(handle, evidence.clone(), cx) {
        return;
    }
    restart_evidence::start(handle, cx);
    page_evidence::start(handle, evidence.clone(), cx);
    shots_evidence::start(handle, evidence.clone(), cx);
    overview_evidence::start(handle, evidence.clone(), cx);
}

fn native_click(
    window: &mut gpui::Window,
    app: &mut App,
    position: gpui::Point<gpui::Pixels>,
    button: gpui::MouseButton,
) {
    use gpui::InputEvent;
    window.dispatch_event(
        gpui::MouseDownEvent {
            button,
            position,
            modifiers: Default::default(),
            click_count: 1,
            first_mouse: false,
        }
        .to_platform_input(),
        app,
    );
    window.dispatch_event(
        gpui::MouseUpEvent {
            button,
            position,
            modifiers: Default::default(),
            click_count: 1,
        }
        .to_platform_input(),
        app,
    );
    window.refresh();
    window.draw(app).clear(app);
}
