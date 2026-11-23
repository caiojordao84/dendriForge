//! Dedicated background simulation worker thread publishing immutable `Arc<SimSnapshot>` frames
//! without blocking the UI thread (`02-simulation-and-electrical-engine.md` §2.4).

use crate::kernel::{SimKernel, SimSnapshot};
use crate::timeline::StimulusEvent;
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};

/// Lifecycle state reported by [`SimWorker`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerState {
    Idle,
    Running,
    Paused,
    Faulted,
    Stopped,
}

/// Asynchronous commands sent from the UI/CLI thread to [`SimWorker`].
#[derive(Debug, Clone)]
pub enum WorkerCommand {
    /// Advance simulation up to exact timestamp `target_us`.
    StepUntil(u64),
    /// Advance simulation forward by `delta_us` microseconds.
    StepDelta(u64),
    /// Inject an external stimulus event into the live kernel.
    InjectStimulus(StimulusEvent),
    /// Mark worker paused (subsequent `StepDelta` ignored until `Resume`).
    Pause,
    /// Resume from paused state.
    Resume,
    /// Gracefully shut down the background worker thread.
    Shutdown,
}

/// Handle to a dedicated background thread owning a [`SimKernel`].
///
/// The worker never blocks waiting on UI consumers: latest immutable [`SimSnapshot`]
/// frames are published atomically into a shared slot AND pushed via `try_send` onto
/// a bounded notification channel (`02` §2.4).
pub struct SimWorker {
    cmd_tx: mpsc::Sender<WorkerCommand>,
    latest_snapshot: Arc<Mutex<Arc<SimSnapshot>>>,
    state: Arc<Mutex<WorkerState>>,
    notify_rx: mpsc::Receiver<Arc<SimSnapshot>>,
    handle: Option<JoinHandle<()>>,
}

impl SimWorker {
    /// Spawns a dedicated background thread (`dendri-sim-worker`) owning `kernel`.
    pub fn spawn(kernel: SimKernel) -> Result<Self, std::io::Error> {
        let initial = Arc::new(kernel.snapshot());
        let latest_snapshot = Arc::new(Mutex::new(Arc::clone(&initial)));
        let state = Arc::new(Mutex::new(WorkerState::Idle));

        let (cmd_tx, cmd_rx) = mpsc::channel::<WorkerCommand>();
        // Bounded latest-value notification channel (never blocks sim worker via `try_send`).
        let (notify_tx, notify_rx) = mpsc::sync_channel::<Arc<SimSnapshot>>(4);

        let snap_slot = Arc::clone(&latest_snapshot);
        let state_slot = Arc::clone(&state);

        let handle = thread::Builder::new()
            .name("dendri-sim-worker".to_owned())
            .spawn(move || {
                worker_loop(kernel, cmd_rx, snap_slot, state_slot, notify_tx);
            })?;

        Ok(Self {
            cmd_tx,
            latest_snapshot,
            state,
            notify_rx,
            handle: Some(handle),
        })
    }

    /// Enqueues a [`WorkerCommand`] for asynchronous execution on the worker thread.
    pub fn send(&self, cmd: WorkerCommand) -> Result<(), mpsc::SendError<WorkerCommand>> {
        self.cmd_tx.send(cmd)
    }

    /// Reads the latest immutable [`SimSnapshot`] without blocking on simulation execution.
    #[must_use]
    pub fn latest_snapshot(&self) -> Arc<SimSnapshot> {
        match self.latest_snapshot.lock() {
            Ok(guard) => Arc::clone(&guard),
            Err(poisoned) => Arc::clone(&poisoned.into_inner()),
        }
    }

    /// Reads the current [`WorkerState`].
    #[must_use]
    pub fn state(&self) -> WorkerState {
        match self.state.lock() {
            Ok(guard) => *guard,
            Err(poisoned) => *poisoned.into_inner(),
        }
    }

    /// Non-blocking poll for the next published snapshot notification.
    #[must_use]
    pub fn try_recv_snapshot(&self) -> Option<Arc<SimSnapshot>> {
        self.notify_rx.try_recv().ok()
    }

    /// Blocking wait (with timeout) for the next published snapshot notification.
    #[must_use]
    pub fn recv_snapshot_timeout(&self, timeout: std::time::Duration) -> Option<Arc<SimSnapshot>> {
        self.notify_rx.recv_timeout(timeout).ok()
    }

    /// Gracefully stops and joins the worker thread.
    pub fn shutdown(mut self) {
        let _ = self.cmd_tx.send(WorkerCommand::Shutdown);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for SimWorker {
    fn drop(&mut self) {
        let _ = self.cmd_tx.send(WorkerCommand::Shutdown);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn worker_loop(
    mut kernel: SimKernel,
    cmd_rx: mpsc::Receiver<WorkerCommand>,
    latest_snapshot: Arc<Mutex<Arc<SimSnapshot>>>,
    state: Arc<Mutex<WorkerState>>,
    notify_tx: mpsc::SyncSender<Arc<SimSnapshot>>,
) {
    let mut paused = false;

    while let Ok(cmd) = cmd_rx.recv() {
        match cmd {
            WorkerCommand::Shutdown => {
                set_state(&state, WorkerState::Stopped);
                break;
            }
            WorkerCommand::Pause => {
                paused = true;
                set_state(&state, WorkerState::Paused);
                publish_snapshot(&kernel, &latest_snapshot, &notify_tx);
            }
            WorkerCommand::Resume => {
                paused = false;
                set_state(&state, WorkerState::Idle);
                publish_snapshot(&kernel, &latest_snapshot, &notify_tx);
            }
            WorkerCommand::InjectStimulus(stim) => {
                kernel.schedule_stimulus(stim);
                publish_snapshot(&kernel, &latest_snapshot, &notify_tx);
            }
            WorkerCommand::StepUntil(target_us) => {
                if paused {
                    continue;
                }
                set_state(&state, WorkerState::Running);
                match kernel.run_until(target_us) {
                    Ok(()) => set_state(&state, WorkerState::Idle),
                    Err(_) => set_state(&state, WorkerState::Faulted),
                }
                publish_snapshot(&kernel, &latest_snapshot, &notify_tx);
            }
            WorkerCommand::StepDelta(delta_us) => {
                if paused {
                    continue;
                }
                set_state(&state, WorkerState::Running);
                match kernel.step_delta(delta_us) {
                    Ok(()) => set_state(&state, WorkerState::Idle),
                    Err(_) => set_state(&state, WorkerState::Faulted),
                }
                publish_snapshot(&kernel, &latest_snapshot, &notify_tx);
            }
        }
    }
}

fn set_state(slot: &Mutex<WorkerState>, next: WorkerState) {
    match slot.lock() {
        Ok(mut guard) => *guard = next,
        Err(poisoned) => *poisoned.into_inner() = next,
    }
}

fn publish_snapshot(
    kernel: &SimKernel,
    latest_snapshot: &Mutex<Arc<SimSnapshot>>,
    notify_tx: &mpsc::SyncSender<Arc<SimSnapshot>>,
) {
    let snap = Arc::new(kernel.snapshot());
    match latest_snapshot.lock() {
        Ok(mut guard) => *guard = Arc::clone(&snap),
        Err(poisoned) => *poisoned.into_inner() = Arc::clone(&snap),
    }
    // Non-blocking notification: if UI is slow, drop the ping cleanly (UI reads latest_snapshot).
    let _ = notify_tx.try_send(snap);
}
