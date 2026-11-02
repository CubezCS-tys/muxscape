//! Client-side mirror of one attached tmux session.
//!
//! The remote tmux server is the source of truth. This model is only ever
//! changed by (a) authoritative snapshots from `list-windows`/`list-panes`
//! and (b) notifications. Notifications that carry enough information are
//! applied directly; the rest (e.g. `%window-add`, which carries no index or
//! name) make [`SessionModel::apply`] request a resync.

use crate::commands::{PaneInfo, WindowInfo};
use crate::ids::{PaneId, SessionId, WindowId};
use crate::layout::LayoutCell;
use crate::protocol::Notification;
use serde::Serialize;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WindowState {
    pub id: WindowId,
    pub index: u32,
    pub name: String,
    pub active: bool,
    pub zoomed: bool,
    pub bell: bool,
    pub activity: bool,
    /// Full layout (all panes, even while zoomed).
    pub layout: LayoutCell,
    /// What is actually on screen (only the zoomed pane while zoomed).
    pub visible_layout: LayoutCell,
    pub active_pane: Option<PaneId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct SessionModel {
    pub session_id: Option<SessionId>,
    pub session_name: String,
    /// Ordered by window index.
    pub windows: Vec<WindowState>,
    #[serde(skip)]
    pub panes: HashMap<PaneId, PaneInfo>,
}

/// What a caller should do after applying a notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Nothing visible changed.
    None,
    /// The model changed; re-render.
    Changed,
    /// The model can't be updated from the notification alone; re-query
    /// windows and panes.
    Resync,
    /// Another session is now attached to this client (e.g. after the
    /// attached session was killed and tmux switched us).
    SessionSwitched,
    /// The set of sessions on the server changed (for the sidebar).
    SessionListChanged,
    /// The control client is going away.
    Exit(Option<String>),
}

impl SessionModel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn window(&self, id: WindowId) -> Option<&WindowState> {
        self.windows.iter().find(|w| w.id == id)
    }

    fn window_mut(&mut self, id: WindowId) -> Option<&mut WindowState> {
        self.windows.iter_mut().find(|w| w.id == id)
    }

    pub fn active_window(&self) -> Option<&WindowState> {
        self.windows.iter().find(|w| w.active)
    }

    /// Replace windows & panes with an authoritative snapshot.
    pub fn load_snapshot(&mut self, windows: Vec<WindowInfo>, panes: Vec<PaneInfo>) {
        let mut active_panes: HashMap<WindowId, PaneId> = HashMap::new();
        for p in &panes {
            if p.active {
                active_panes.insert(p.window, p.id);
            }
        }
        self.windows = windows
            .into_iter()
            .map(|w| WindowState {
                active_pane: active_panes.get(&w.id).copied(),
                id: w.id,
                index: w.index,
                name: w.name,
                active: w.active,
                zoomed: w.zoomed,
                bell: w.bell,
                activity: w.activity,
                layout: w.layout,
                visible_layout: w.visible_layout,
            })
            .collect();
        self.windows.sort_by_key(|w| w.index);
        self.panes = panes.into_iter().map(|p| (p.id, p)).collect();
    }

    /// All pane ids that currently exist in any window layout.
    pub fn all_panes(&self) -> Vec<PaneId> {
        self.windows.iter().flat_map(|w| w.layout.panes()).collect()
    }

    pub fn apply(&mut self, n: &Notification) -> Effect {
        use Notification as N;
        match n {
            N::LayoutChange { window, layout, visible_layout, flags } => {
                let Some(w) = self.window_mut(*window) else { return Effect::Resync };
                let Ok(l) = LayoutCell::parse(layout) else { return Effect::Resync };
                let vis = match visible_layout {
                    Some(v) => match LayoutCell::parse(v) {
                        Ok(v) => v,
                        Err(_) => return Effect::Resync,
                    },
                    None => l.clone(),
                };
                let old_panes = w.layout.panes();
                w.layout = l;
                w.visible_layout = vis;
                if let Some(f) = flags {
                    w.zoomed = f.contains('Z');
                }
                // New panes appeared (split, join): fetch their details.
                if w.layout.panes().iter().any(|p| !old_panes.contains(p)) {
                    return Effect::Resync;
                }
                let live = w.layout.panes();
                if w.active_pane.is_some_and(|p| !live.contains(&p)) {
                    w.active_pane = None;
                }
                self.panes.retain(|_, p| p.window != *window || live.contains(&p.id));
                Effect::Changed
            }
            N::WindowAdd { .. } => Effect::Resync,
            N::WindowClose { window } | N::UnlinkedWindowClose { window } => {
                let before = self.windows.len();
                self.windows.retain(|w| w.id != *window);
                self.panes.retain(|_, p| p.window != *window);
                if self.windows.len() != before {
                    Effect::Changed
                } else {
                    Effect::None
                }
            }
            N::WindowRenamed { window, name } => match self.window_mut(*window) {
                Some(w) if w.name != *name => {
                    w.name = name.clone();
                    Effect::Changed
                }
                Some(_) => Effect::None,
                None => Effect::Resync,
            },
            N::WindowPaneChanged { window, pane } => match self.window_mut(*window) {
                Some(w) => {
                    w.active_pane = Some(*pane);
                    for p in self.panes.values_mut().filter(|p| p.window == *window) {
                        p.active = p.id == *pane;
                    }
                    Effect::Changed
                }
                None => Effect::Resync,
            },
            N::SessionWindowChanged { session, window } => {
                if Some(*session) != self.session_id {
                    return Effect::None;
                }
                if self.window(*window).is_none() {
                    return Effect::Resync;
                }
                for w in &mut self.windows {
                    w.active = w.id == *window;
                }
                Effect::Changed
            }
            N::SessionChanged { session, name } => {
                let switched = self.session_id.is_some_and(|s| s != *session);
                self.session_id = Some(*session);
                self.session_name = name.clone();
                if switched {
                    self.windows.clear();
                    self.panes.clear();
                    Effect::SessionSwitched
                } else {
                    Effect::Resync
                }
            }
            N::SessionRenamed { session, name } => {
                if Some(*session) == self.session_id {
                    self.session_name = name.clone();
                }
                Effect::SessionListChanged
            }
            N::SessionsChanged => Effect::SessionListChanged,
            N::PaneModeChanged { pane } => {
                if let Some(p) = self.panes.get_mut(pane) {
                    p.in_mode = !p.in_mode;
                }
                Effect::Changed
            }
            N::Exit { reason } => Effect::Exit(reason.clone()),
            _ => Effect::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{parse_panes, parse_windows};
    use crate::layout::checksum;

    fn lay(body: &str) -> String {
        format!("{:04x},{}", checksum(body), body)
    }

    fn model() -> SessionModel {
        let a = lay("80x24,0,0,1");
        let b = lay("80x24,0,0{40x24,0,0,2,39x24,41,0,3}");
        let windows = parse_windows(
            [format!("@1|0|1|0|0|0|{a}|{a}|shell"), format!("@2|1|0|0|0|0|{b}|{b}|editor")]
                .iter()
                .map(|s| s.as_str()),
        )
        .unwrap();
        let panes = parse_panes([
            "%1|@1|0|1|80|24|0|0|1|0|0|0|0|0|0|1|0|0|0|0|0|0|0|23|0|1|bash",
            "%2|@2|0|0|40|24|0|0|1|0|0|0|0|0|0|1|0|0|0|0|0|0|0|23|0|2|vim",
            "%3|@2|1|1|39|24|0|0|1|0|0|0|0|0|0|1|0|0|0|0|0|0|0|23|0|3|bash",
        ])
        .unwrap();
        let mut m = SessionModel::new();
        m.session_id = Some(SessionId(0));
        m.load_snapshot(windows, panes);
        m
    }

    #[test]
    fn snapshot() {
        let m = model();
        assert_eq!(m.windows.len(), 2);
        assert_eq!(m.active_window().unwrap().id, WindowId(1));
        assert_eq!(m.window(WindowId(2)).unwrap().active_pane, Some(PaneId(3)));
        assert_eq!(m.all_panes(), vec![PaneId(1), PaneId(2), PaneId(3)]);
    }

    #[test]
    fn rename_and_select() {
        let mut m = model();
        assert_eq!(m.apply(&Notification::WindowRenamed { window: WindowId(2), name: "vim".into() }), Effect::Changed);
        assert_eq!(m.window(WindowId(2)).unwrap().name, "vim");
        assert_eq!(m.apply(&Notification::WindowRenamed { window: WindowId(2), name: "vim".into() }), Effect::None);
        assert_eq!(
            m.apply(&Notification::SessionWindowChanged { session: SessionId(0), window: WindowId(2) }),
            Effect::Changed
        );
        assert_eq!(m.active_window().unwrap().id, WindowId(2));
        // Unknown window -> resync; other session -> ignore.
        assert_eq!(m.apply(&Notification::WindowRenamed { window: WindowId(9), name: "x".into() }), Effect::Resync);
        assert_eq!(
            m.apply(&Notification::SessionWindowChanged { session: SessionId(5), window: WindowId(1) }),
            Effect::None
        );
    }

    #[test]
    fn layout_changes() {
        let mut m = model();
        // Pane 3 closed: layout shrinks to pane 2 only.
        let l = lay("80x24,0,0,2");
        let e = m.apply(&Notification::LayoutChange {
            window: WindowId(2),
            layout: l.clone(),
            visible_layout: Some(l),
            flags: Some("*".into()),
        });
        assert_eq!(e, Effect::Changed);
        assert_eq!(m.window(WindowId(2)).unwrap().active_pane, None);
        assert!(!m.panes.contains_key(&PaneId(3)));
        // Split creates pane 7: needs pane details.
        let l = lay("80x24,0,0[80x12,0,0,2,80x11,0,13,7]");
        let e = m.apply(&Notification::LayoutChange { window: WindowId(2), layout: l.clone(), visible_layout: Some(l), flags: None });
        assert_eq!(e, Effect::Resync);
        // Zoom: visible layout differs.
        let full = lay("80x24,0,0[80x12,0,0,2,80x11,0,13,7]");
        let vis = lay("80x24,0,0,7");
        m.apply(&Notification::LayoutChange { window: WindowId(2), layout: full, visible_layout: Some(vis), flags: Some("*Z".into()) });
        let w = m.window(WindowId(2)).unwrap();
        assert!(w.zoomed);
        assert_eq!(w.visible_layout.panes(), vec![PaneId(7)]);
        assert_eq!(w.layout.panes().len(), 2);
        // Garbage layout -> resync rather than corrupt state.
        assert_eq!(
            m.apply(&Notification::LayoutChange { window: WindowId(2), layout: "zzzz".into(), visible_layout: None, flags: None }),
            Effect::Resync
        );
    }

    #[test]
    fn windows_added_and_closed() {
        let mut m = model();
        assert_eq!(m.apply(&Notification::WindowAdd { window: WindowId(3) }), Effect::Resync);
        assert_eq!(m.apply(&Notification::WindowClose { window: WindowId(1) }), Effect::Changed);
        assert_eq!(m.windows.len(), 1);
        assert!(!m.panes.contains_key(&PaneId(1)));
        assert_eq!(m.apply(&Notification::WindowClose { window: WindowId(1) }), Effect::None);
    }

    #[test]
    fn session_lifecycle() {
        let mut m = model();
        assert_eq!(m.apply(&Notification::SessionsChanged), Effect::SessionListChanged);
        assert_eq!(m.apply(&Notification::SessionRenamed { session: SessionId(0), name: "work".into() }), Effect::SessionListChanged);
        assert_eq!(m.session_name, "work");
        // Same session announced again (initial attach) -> resync.
        assert_eq!(m.apply(&Notification::SessionChanged { session: SessionId(0), name: "work".into() }), Effect::Resync);
        // Our session got killed and tmux moved us to another one.
        assert_eq!(m.apply(&Notification::SessionChanged { session: SessionId(4), name: "other".into() }), Effect::SessionSwitched);
        assert!(m.windows.is_empty());
        assert_eq!(m.apply(&Notification::Exit { reason: Some("detached".into()) }), Effect::Exit(Some("detached".into())));
    }
}
