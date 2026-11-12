//! Writes the layout record (`session.rs`) on a thread of its own. Each write syncs the file and
//! then its folder, which on a busy disk can take seconds; on the event loop that would freeze
//! the desktop for as long, and long enough would end the session through the watchdog.
//!
//! Only the newest change waiting is made: a record sent while another is still waiting takes
//! its place, and so does deleting the record. What's being written meanwhile is finished first,
//! so the file always ends up as the last thing sent.

use std::{
    path::PathBuf,
    sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError},
    thread,
};

use crate::session::{self, Record};

enum Job {
    Write { path: PathBuf, record: Record },
    Forget(PathBuf),
}

#[derive(Default)]
struct Queue {
    next: Option<Job>,
    busy: bool,
    failed: bool,
}

#[derive(Default)]
struct Shared {
    queue: Mutex<Queue>,
    changed: Condvar,
}

impl Shared {
    fn queue(&self) -> MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The writer thread's end on the event loop. The thread starts with the first change sent.
#[derive(Default)]
pub struct Writer {
    shared: Arc<Shared>,
    started: bool,
}

impl Writer {
    /// Writes `record` to `path`, replacing any change still waiting.
    pub fn write(&mut self, path: PathBuf, record: Record) {
        self.send(Job::Write { path, record });
    }

    /// Deletes the record at `path`, replacing any change still waiting.
    pub fn forget(&mut self, path: PathBuf) {
        self.send(Job::Forget(path));
    }

    /// Waits until every change sent has been made, for the way out.
    pub fn flush(&self) {
        let mut queue = self.shared.queue();
        while queue.next.is_some() || queue.busy {
            queue = self
                .shared
                .changed
                .wait(queue)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// Whether a change has failed since this was last asked.
    pub fn take_failed(&self) -> bool {
        std::mem::take(&mut self.shared.queue().failed)
    }

    fn send(&mut self, job: Job) {
        if !self.started {
            let shared = self.shared.clone();
            let spawned = thread::Builder::new()
                .name("session-record".into())
                .spawn(move || work(&shared));
            match spawned {
                Ok(_) => self.started = true,
                // Better late than never: on this thread, as before there was one of its own.
                Err(err) => {
                    tracing::warn!("no thread for writing the layout down: {err}");
                    let ok = run(job);
                    self.shared.queue().failed |= !ok;
                    return;
                }
            }
        }
        self.shared.queue().next = Some(job);
        self.shared.changed.notify_all();
    }
}

/// The writer thread: each change as it comes, for as long as Slipstream runs.
fn work(shared: &Shared) {
    loop {
        let job = {
            let mut queue = shared.queue();
            loop {
                if let Some(job) = queue.next.take() {
                    queue.busy = true;
                    break job;
                }
                queue = shared
                    .changed
                    .wait(queue)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        };
        let ok = run(job);
        let mut queue = shared.queue();
        queue.busy = false;
        queue.failed |= !ok;
        drop(queue);
        shared.changed.notify_all();
    }
}

/// Makes one change, saying how it went in the log. Whether it worked.
fn run(job: Job) -> bool {
    match job {
        Job::Write { path, record } => match session::write(&path, &record) {
            Ok(()) => {
                tracing::debug!(windows = record.windows.len(), "layout written down");
                true
            }
            Err(err) => {
                tracing::warn!(path = %path.display(), "couldn't write the layout down: {err}");
                false
            }
        },
        Job::Forget(path) => match session::forget(&path) {
            Ok(()) => true,
            Err(err) => {
                tracing::warn!(path = %path.display(), "couldn't delete the layout: {err}");
                false
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_change_sent_is_what_stays() {
        let dir = crate::files::test_scratch("record-writer");
        let path = dir.join("session.toml");
        let mut writer = Writer::default();
        let mut record = Record::default();
        for n in 1..=20 {
            record.active_workspace = n;
            writer.write(path.clone(), record.clone());
        }
        writer.flush();
        let read = session::read(&path).unwrap().unwrap();
        assert_eq!(read.active_workspace, 20);
        writer.forget(path.clone());
        writer.flush();
        assert!(!path.exists());
        assert!(!writer.take_failed());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
