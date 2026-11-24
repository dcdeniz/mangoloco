//! Background jobs: long work on its own thread, with progress and cancel,
//! finishing back on the UI thread.

use crate::workspace::Workspace;
use gpui_kit::{Context, Window};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// Runs on the UI thread when the job's thread has finished.
pub type Completion = Box<dyn FnOnce(&mut Workspace, &mut Window, &mut Context<Workspace>) + Send>;

#[derive(Default)]
pub struct JobShared {
    progress: AtomicU32,
    pub cancel: Arc<AtomicBool>,
    done: Mutex<Option<Completion>>,
    note: Mutex<String>,
}

impl JobShared {
    pub fn set_progress(&self, p: f32) {
        self.progress.store(p.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }
    pub fn progress(&self) -> f32 {
        f32::from_bits(self.progress.load(Ordering::Relaxed))
    }
    /// A short description of the current step, shown under the label.
    pub fn set_note(&self, note: impl Into<String>) {
        *self.note.lock().unwrap() = note.into();
    }
    pub fn note(&self) -> String {
        self.note.lock().unwrap().clone()
    }
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

pub struct Job {
    pub id: u64,
    pub label: String,
    pub shared: Arc<JobShared>,
    thread: Option<JoinHandle<()>>,
}

#[derive(Default)]
pub struct Jobs {
    pub list: Vec<Job>,
    next_id: u64,
}

impl Jobs {
    /// Starts `work` on a new thread. `work` returns what to do with the
    /// result on the UI thread.
    pub fn start(&mut self, label: impl Into<String>, work: impl FnOnce(&JobShared) -> Completion + Send + 'static) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        let shared = Arc::new(JobShared::default());
        let s = shared.clone();
        let thread = std::thread::Builder::new()
            .name(format!("mango-job-{id}"))
            .spawn(move || {
                let completion = work(&s);
                *s.done.lock().unwrap() = Some(completion);
            })
            .ok();
        self.list.push(Job { id, label: label.into(), shared, thread });
        id
    }

    pub fn cancel(&self, id: u64) {
        if let Some(j) = self.list.iter().find(|j| j.id == id) {
            j.shared.cancel.store(true, Ordering::Relaxed);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// Removes finished jobs and returns their completions.
    pub fn take_finished(&mut self) -> Vec<Completion> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < self.list.len() {
            let done = self.list[i].shared.done.lock().unwrap().take();
            match done {
                Some(c) => {
                    let mut job = self.list.remove(i);
                    if let Some(t) = job.thread.take() {
                        let _ = t.join();
                    }
                    out.push(c);
                }
                None => {
                    // A thread that died without reporting (panic) must not sit in the list forever.
                    if self.list[i].thread.as_ref().is_some_and(|t| t.is_finished()) && self.list[i].shared.done.lock().unwrap().is_none() {
                        let job = self.list.remove(i);
                        let label = job.label.clone();
                        out.push(Box::new(move |ws, _, cx| {
                            ws.model.fail(format!("{label} stopped unexpectedly"));
                            cx.notify();
                        }));
                    } else {
                        i += 1;
                    }
                }
            }
        }
        out
    }

    /// Cancels everything and waits for the threads. Call before quitting.
    pub fn shutdown(&mut self) {
        for j in &self.list {
            j.shared.cancel.store(true, Ordering::Relaxed);
        }
        for mut j in self.list.drain(..) {
            if let Some(t) = j.thread.take() {
                let _ = t.join();
            }
        }
    }
}
