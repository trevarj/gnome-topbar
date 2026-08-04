//! What the panel is allowed to see of niri's state.
//!
//! The event-stream reducer keeps a full [`EventStreamState`]; widgets get
//! these small, immutable, `PartialEq` projections of it instead. Two reasons:
//!
//! - `PartialEq` is what makes "publish after every event" cheap. Most niri
//!   events (window layout, focus timestamps, screencasts) change nothing the
//!   panel draws, so the watch channel simply does not fire.
//! - The projections carry no compositor vocabulary the widgets would have to
//!   re-derive. Occupancy and urgency are already folded in here.

use std::collections::{BTreeMap, HashSet};

use niri_ipc::state::EventStreamState;

/// One workspace, as the panel draws it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceView {
    /// Stable id, used to focus this workspace.
    pub id: u64,
    /// Position on its output. Changes when workspaces are re-ordered.
    pub idx: u8,
    /// Configured name, if the user gave it one.
    pub name: Option<String>,
    /// Whether this is the visible workspace on its output.
    pub is_active: bool,
    /// Whether this is the one focused workspace across all outputs.
    pub is_focused: bool,
    /// Whether this workspace, or a window on it, wants attention.
    pub is_urgent: bool,
    /// Whether any window lives on this workspace.
    pub has_windows: bool,
}

/// Every workspace the compositor reports, grouped by output.
///
/// The map is a `BTreeMap` so iterating it (which the widget does when
/// `filter_by_output = false`) has a stable, name-sorted order rather than a
/// hash order that reshuffles the bar on every rebuild.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspacesSnapshot {
    /// Whether the event stream is currently connected.
    ///
    /// While this is `false` the rest of the snapshot is the last state we
    /// saw; widgets dim rather than empty themselves.
    pub connected: bool,
    /// Connector name (`eDP-1`) → its workspaces, sorted by index.
    pub outputs: BTreeMap<String, Vec<WorkspaceView>>,
    /// Connector holding the focused workspace, if any.
    pub focused_output: Option<String>,
}

impl WorkspacesSnapshot {
    /// The workspaces on `connector`, or an empty slice if it has none.
    pub fn for_output(&self, connector: &str) -> &[WorkspaceView] {
        self.outputs.get(connector).map_or(&[], Vec::as_slice)
    }

    /// Every workspace on every output, in connector order then index order.
    pub fn all(&self) -> impl Iterator<Item = &WorkspaceView> {
        self.outputs.values().flatten()
    }

    /// The same snapshot, marked as coming from a live connection or not.
    pub fn with_connected(mut self, connected: bool) -> Self {
        self.connected = connected;
        self
    }
}

/// The configured keyboard layouts and which one is active.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeyboardLayoutSnapshot {
    /// Whether the event stream is currently connected.
    pub connected: bool,
    /// Layout descriptions as niri reports them, e.g. `"English (US)"`.
    pub names: Vec<String>,
    /// Index into [`Self::names`] of the active layout.
    pub current_idx: u8,
}

impl KeyboardLayoutSnapshot {
    /// The active layout's full name, if the index is in range.
    pub fn current(&self) -> Option<&str> {
        self.names
            .get(usize::from(self.current_idx))
            .map(String::as_str)
    }

    /// Whether there is more than one layout to switch between.
    pub fn is_switchable(&self) -> bool {
        self.names.len() > 1
    }

    /// The same snapshot, marked as coming from a live connection or not.
    pub fn with_connected(mut self, connected: bool) -> Self {
        self.connected = connected;
        self
    }
}

/// Project the workspace half of `state`.
///
/// Occupancy comes from the window map rather than `active_window_id`: a
/// workspace can hold windows without any of them being active, and the two
/// parts of the state are explicitly allowed to disagree for an event or two.
pub(crate) fn workspaces(state: &EventStreamState, connected: bool) -> WorkspacesSnapshot {
    let mut occupied: HashSet<u64> = HashSet::new();
    let mut urgent_windows: HashSet<u64> = HashSet::new();
    for window in state.windows.windows.values() {
        let Some(workspace_id) = window.workspace_id else {
            continue;
        };
        occupied.insert(workspace_id);
        if window.is_urgent {
            urgent_windows.insert(workspace_id);
        }
    }

    let mut outputs: BTreeMap<String, Vec<WorkspaceView>> = BTreeMap::new();
    let mut focused_output = None;
    for workspace in state.workspaces.workspaces.values() {
        // A workspace with no output belongs to a monitor that is not
        // connected right now; no bar can draw it.
        let Some(output) = workspace.output.clone() else {
            continue;
        };
        if workspace.is_focused {
            focused_output = Some(output.clone());
        }
        outputs.entry(output).or_default().push(WorkspaceView {
            id: workspace.id,
            idx: workspace.idx,
            name: workspace.name.clone(),
            is_active: workspace.is_active,
            is_focused: workspace.is_focused,
            is_urgent: workspace.is_urgent || urgent_windows.contains(&workspace.id),
            has_windows: occupied.contains(&workspace.id),
        });
    }

    // The state stores workspaces in a hash map, so the order it hands them
    // back is arbitrary and unstable between events.
    for views in outputs.values_mut() {
        views.sort_by_key(|view| (view.idx, view.id));
    }

    WorkspacesSnapshot {
        connected,
        outputs,
        focused_output,
    }
}

/// Project the keyboard-layout half of `state`.
pub(crate) fn keyboard_layout(state: &EventStreamState, connected: bool) -> KeyboardLayoutSnapshot {
    let layouts = state.keyboard_layouts.keyboard_layouts.as_ref();
    KeyboardLayoutSnapshot {
        connected,
        names: layouts
            .map(|layouts| layouts.names.clone())
            .unwrap_or_default(),
        current_idx: layouts.map_or(0, |layouts| layouts.current_idx),
    }
}
