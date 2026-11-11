//! Sessions: one worker per physical monitor. Every command is addressed to exactly one session; events
//! of the sessions carry the `MonitorId` they come from; the window shows one monitor at a time and keeps the state of
//! the others. The [`Router`] sends the commands of the window to the selected monitor only.

use super::{MonitorId, UiCmd, WorkerCmd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

/// An event of one session.
pub struct SessionEvent {
    pub monitor: MonitorId,
    pub cmd: UiCmd,
}

pub struct MonitorSession {
    pub id: MonitorId,
    /// `\\.\DISPLAYn` or the I2C device the session was opened on.
    pub device: String,
    pub cmd_tx: mpsc::Sender<WorkerCmd>,
    /// Set when the monitor is gone or could not be connected: the session is replaced when the monitor is seen again.
    pub dead: Arc<AtomicBool>,
}

#[derive(Default)]
pub struct SessionManager {
    sessions: Vec<MonitorSession>,
}

impl SessionManager {
    pub fn add(&mut self, session: MonitorSession) {
        self.sessions.push(session);
    }

    /// Puts a new session in the place of the one with the same id (the old worker ends when its channel is dropped),
    /// or adds it.
    pub fn replace_or_add(&mut self, session: MonitorSession) {
        match self.sessions.iter_mut().find(|old| old.id == session.id) {
            Some(old) => *old = session,
            None => self.sessions.push(session),
        }
    }

    /// Drops the session of `id` (its worker ends and releases the monitor handle).
    pub fn remove(&mut self, id: &MonitorId) -> bool {
        let before = self.sessions.len();
        self.sessions.retain(|session| &session.id != id);
        self.sessions.len() != before
    }

    /// `Some(dead)` for a known session.
    pub fn is_dead(&self, id: &MonitorId) -> Option<bool> {
        self.sessions.iter().find(|session| &session.id == id).map(|session| session.dead.load(Ordering::Relaxed))
    }

    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    pub fn ids(&self) -> Vec<MonitorId> {
        self.sessions.iter().map(|session| session.id.clone()).collect()
    }

    /// Sends a command to one session. False when there is no such session or its worker has ended.
    pub fn send(&self, id: &MonitorId, cmd: WorkerCmd) -> bool {
        self.sessions
            .iter()
            .find(|session| &session.id == id)
            .is_some_and(|session| session.cmd_tx.send(cmd).is_ok())
    }
}

/// Forwards the events of one session, tagged with its id. Ends when the session's worker is gone.
pub fn forward_tagged(
    id: MonitorId,
    rx: async_channel::Receiver<UiCmd>,
    out: async_channel::Sender<SessionEvent>,
    dead: Option<Arc<AtomicBool>>,
) {
    while let Ok(cmd) = rx.recv_blocking() {
        if let Some(dead) = &dead {
            match &cmd {
                UiCmd::Responding(false) => dead.store(true, Ordering::Relaxed),
                UiCmd::Error(error) if error == "Monitor not found" => dead.store(true, Ordering::Relaxed),
                UiCmd::MonitorFound(_) | UiCmd::Responding(true) => dead.store(false, Ordering::Relaxed),
                _ => {}
            }
        }
        if out.send_blocking(SessionEvent { monitor: id.clone(), cmd }).is_err() {
            break;
        }
    }
}

/// Routes the commands of the UI: to the monitor chosen with `WorkerCmd::SelectMonitor`, never to another one.
/// Commands that arrive before a monitor is chosen wait for it. The firmware setting concerns every monitor.
pub struct Router {
    manager: SessionManager,
    active: Option<MonitorId>,
    pending: Vec<WorkerCmd>,
}

const PENDING_LIMIT: usize = 64;

impl Router {
    pub fn new(manager: SessionManager) -> Self {
        Self { manager, active: None, pending: Vec::new() }
    }

    pub fn active(&self) -> Option<&MonitorId> {
        self.active.as_ref()
    }

    /// Forgets a monitor that is gone: its session ends and it can no longer be the target of commands.
    pub fn forget(&mut self, id: &MonitorId) {
        self.manager.remove(id);
        if self.active.as_ref() == Some(id) {
            self.active = None;
        }
    }

    pub fn manager_mut(&mut self) -> &mut SessionManager {
        &mut self.manager
    }

    pub fn manager(&self) -> &SessionManager {
        &self.manager
    }

    pub fn on_command(&mut self, cmd: WorkerCmd) {
        match cmd {
            WorkerCmd::SelectMonitor(id) => {
                if !self.manager.ids().contains(&id) {
                    log!("[SESSION] cannot select {}: no such session", id.label());
                    return;
                }
                log!("[SESSION] active monitor: {}", id.label());
                self.active = Some(id.clone());
                for pending in std::mem::take(&mut self.pending) {
                    self.manager.send(&id, pending);
                }
            }
            WorkerCmd::DydsWithAdaptiveSync(allowed) => {
                for id in self.manager.ids() {
                    self.manager.send(&id, WorkerCmd::DydsWithAdaptiveSync(allowed));
                }
            }
            cmd => match &self.active {
                Some(id) => {
                    if !self.manager.send(id, cmd) {
                        log!("[SESSION] {}: worker has ended, command dropped", id.label());
                    }
                }
                None => {
                    if self.pending.len() < PENDING_LIMIT {
                        self.pending.push(cmd);
                    }
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(name: &str, path: &str) -> MonitorId {
        MonitorId { name: name.into(), edid_id: "LHC907D".into(), serial: "0".into(), path: path.into() }
    }

    fn session(id: &MonitorId) -> (MonitorSession, mpsc::Receiver<WorkerCmd>) {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        (MonitorSession { id: id.clone(), device: "test".into(), cmd_tx, dead: Arc::new(AtomicBool::new(false)) }, cmd_rx)
    }

    fn two() -> (Router, MonitorId, MonitorId, mpsc::Receiver<WorkerCmd>, mpsc::Receiver<WorkerCmd>) {
        let (a, b) = (id("P275MV PLUS", "A"), id("P275MV PLUS", "B"));
        let (sa, rx_a) = session(&a);
        let (sb, rx_b) = session(&b);
        let mut manager = SessionManager::default();
        manager.add(sa);
        manager.add(sb);
        (Router::new(manager), a, b, rx_a, rx_b)
    }

    #[test]
    fn commands_reach_only_the_selected_monitor() {
        let (mut router, a, b, rx_a, rx_b) = two();

        // Before a monitor is chosen commands wait; the firmware setting goes to both.
        router.on_command(WorkerCmd::ReadUsageTime);
        router.on_command(WorkerCmd::DydsWithAdaptiveSync(true));
        assert!(matches!(rx_a.try_recv(), Ok(WorkerCmd::DydsWithAdaptiveSync(true))));
        assert!(matches!(rx_b.try_recv(), Ok(WorkerCmd::DydsWithAdaptiveSync(true))));
        assert!(rx_a.try_recv().is_err() && rx_b.try_recv().is_err());

        router.on_command(WorkerCmd::SelectMonitor(b.clone()));
        assert_eq!(router.active(), Some(&b));
        assert!(matches!(rx_b.try_recv(), Ok(WorkerCmd::ReadUsageTime)), "the waiting command goes to B");

        router.on_command(WorkerCmd::Set(0x10, 40));
        assert!(matches!(rx_b.try_recv(), Ok(WorkerCmd::Set(0x10, 40))));
        assert!(rx_a.try_recv().is_err(), "a write to B must never reach A");

        // Switching: the next command goes to A, B receives nothing more.
        router.on_command(WorkerCmd::SelectMonitor(a));
        router.on_command(WorkerCmd::Set(0x12, 10));
        assert!(matches!(rx_a.try_recv(), Ok(WorkerCmd::Set(0x12, 10))));
        assert!(rx_b.try_recv().is_err());
    }

    #[test]
    fn a_removed_monitor_is_forgotten() {
        let (mut router, a, b, rx_a, rx_b) = two();
        router.on_command(WorkerCmd::SelectMonitor(a.clone()));
        router.forget(&a);
        assert!(router.active().is_none());
        assert_eq!(router.manager().ids(), vec![b.clone()]);
        // Nothing reaches the removed monitor; its worker sees the closed channel.
        router.on_command(WorkerCmd::Set(0x10, 1));
        assert!(rx_b.try_recv().is_err(), "no monitor is selected any more");
        assert!(matches!(rx_a.try_recv(), Err(mpsc::TryRecvError::Disconnected)));
    }

    #[test]
    fn an_unknown_monitor_cannot_be_selected() {
        let (mut router, _a, _b, rx_a, rx_b) = two();
        router.on_command(WorkerCmd::SelectMonitor(id("P275MV PLUS", "C")));
        assert!(router.active().is_none());
        router.on_command(WorkerCmd::Set(0x10, 1));
        assert!(rx_a.try_recv().is_err() && rx_b.try_recv().is_err(), "nothing is sent without a selected monitor");
    }
}
