//! Coalescing file writer for the small files the runtime keeps current (L2).
//!
//! `persist_session` runs after every message and tool result, and every status
//! update rewrites `hyusk-status.json`. Both used to serialise their snapshot
//! and do their file IO on the async worker that was running a turn. Now the
//! caller only hands over a *builder* and returns: the queue keeps at most one
//! entry per file (the newest builder wins, so a superseded snapshot is never
//! even serialised) and a single writer task replaces the file atomically.
//!
//! The synchronous path stays for two cases: shutdown ([`flush`], which writes
//! everything still queued before the process exits) and callers without a
//! started writer (tests, the standalone workflow binary, early startup). A
//! failed write is logged with its category and dropped; nothing in a turn
//! depends on the file having landed.
//!
//! Lock order: a caller may hold its own state lock while enqueueing (the status
//! store does), so this module never takes another module's lock. Writes are
//! serialised through one lock so an older snapshot can never overwrite a newer
//! one that already landed.

use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, MutexGuard, OnceLock,
    },
};

/// Which file a write belongs to, for the failure log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Session,
    Status,
}

impl Category {
    pub fn as_str(self) -> &'static str {
        match self {
            Category::Session => "session",
            Category::Status => "status",
        }
    }
}

/// Produce the bytes to write. `None` means the snapshot could not be built
/// (a serialisation failure), which is reported and dropped like a failed write.
pub type Builder = Box<dyn FnOnce() -> Option<Vec<u8>> + Send>;

struct Entry {
    category: Category,
    path: PathBuf,
    /// Assigned by [`Queue::enqueue`]; a write only lands if it is not older
    /// than the newest one already on disk.
    generation: u64,
    build: Builder,
}

/// At most one entry per path. More than one is still handled, so a stale take
/// can never drop a newer snapshot on the floor.
#[derive(Default)]
struct Queue {
    entries: Vec<Entry>,
    next_generation: u64,
}

impl Queue {
    fn enqueue(&mut self, category: Category, path: PathBuf, build: Builder) {
        self.next_generation = self.next_generation.saturating_add(1);
        let generation = self.next_generation;
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.path == path) {
            // Coalescing: the superseded snapshot is dropped unbuilt.
            entry.category = category;
            entry.generation = generation;
            entry.build = build;
            return;
        }
        self.entries.push(Entry {
            category,
            path,
            generation,
            build,
        });
    }

    fn take(&mut self) -> Vec<Entry> {
        std::mem::take(&mut self.entries)
    }

    #[cfg(test)]
    fn pending_len(&self) -> usize {
        self.entries.len()
    }
}

fn queue() -> &'static Mutex<Queue> {
    static QUEUE: OnceLock<Mutex<Queue>> = OnceLock::new();
    QUEUE.get_or_init(|| Mutex::new(Queue::default()))
}

fn queue_lock() -> MutexGuard<'static, Queue> {
    queue()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn notify() -> &'static tokio::sync::Notify {
    static NOTIFY: OnceLock<tokio::sync::Notify> = OnceLock::new();
    NOTIFY.get_or_init(tokio::sync::Notify::new)
}

fn started() -> &'static AtomicBool {
    static STARTED: AtomicBool = AtomicBool::new(false);
    &STARTED
}

/// The newest generation written per path, so a slow writer cannot put an older
/// snapshot back over a newer one.
fn written() -> &'static Mutex<std::collections::HashMap<PathBuf, u64>> {
    static WRITTEN: OnceLock<Mutex<std::collections::HashMap<PathBuf, u64>>> = OnceLock::new();
    WRITTEN.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// Start the single writer task.
///
/// Called once by the composition root, inside the runtime. Without it every
/// [`enqueue`] writes synchronously, which is what tests, the standalone
/// workflow binary, and any caller before startup need.
pub fn start() {
    if started().swap(true, Ordering::SeqCst) {
        return;
    }
    if tokio::runtime::Handle::try_current().is_ok() {
        tokio::spawn(writer_task());
    }
}

async fn writer_task() {
    loop {
        notify().notified().await;
        for entry in queue_lock().take() {
            write_entry(entry);
        }
    }
}

/// Queue a write, or write it now when no writer is running.
///
/// Never returns an error: a turn must not fail because a snapshot could not be
/// saved, and the failure is logged with its category instead.
pub fn enqueue(category: Category, path: PathBuf, build: Builder) {
    if !started().load(Ordering::SeqCst) {
        write_now(category, &path, build);
        return;
    }
    queue_lock().enqueue(category, path, build);
    notify().notify_one();
}

/// Write everything still queued, synchronously. Used at shutdown.
///
/// Returns how many writes landed, which the tests use to prove the queue is
/// empty afterwards.
pub fn flush() -> usize {
    flush_queue(queue())
}

fn flush_queue(queue: &Mutex<Queue>) -> usize {
    let entries = queue
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    let mut landed = 0;
    for entry in entries {
        if write_entry(entry) {
            landed += 1;
        }
    }
    landed
}

fn write_now(category: Category, path: &Path, build: Builder) {
    let Some(bytes) = build() else {
        eprintln!(
            "[Writer] Could not build the {} snapshot",
            category.as_str()
        );
        return;
    };
    if let Err(error) = write_atomically(path, &bytes) {
        eprintln!(
            "[Writer] Could not write {} {}: {error}",
            category.as_str(),
            path.display()
        );
    }
}

/// Serialise and write a queued entry, honouring the newest-wins rule.
fn write_entry(entry: Entry) -> bool {
    let Some(bytes) = (entry.build)() else {
        eprintln!(
            "[Writer] Could not build the {} snapshot",
            entry.category.as_str()
        );
        return false;
    };
    // Held across the write so two writers cannot interleave per path, and so a
    // slower writer cannot put an older generation back on disk.
    let mut written = written_lock();
    if entry.generation < written.get(&entry.path).copied().unwrap_or(0) {
        return false;
    }
    if let Err(error) = write_atomically(&entry.path, &bytes) {
        eprintln!(
            "[Writer] Could not write {} {}: {error}",
            entry.category.as_str(),
            entry.path.display()
        );
        return false;
    }
    written.insert(entry.path, entry.generation);
    true
}

fn written_lock() -> MutexGuard<'static, std::collections::HashMap<PathBuf, u64>> {
    written()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Replace `path` with `bytes` atomically, owner-only.
///
/// A temporary file in the same directory is created with mode `0600` and
/// renamed over the target, so a reader (the GNOME extension polls the status
/// file) never sees a half-written file, and the result is private even in the
/// shared temporary directory (X4).
pub fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    write_atomically_with(
        path,
        bytes,
        |file: &mut std::fs::File, bytes: &[u8]| std::io::Write::write_all(file, bytes),
        |from, to| std::fs::rename(from, to),
    )
}

fn write_atomically_with(
    path: &Path,
    bytes: &[u8],
    write: impl FnOnce(&mut std::fs::File, &[u8]) -> std::io::Result<()>,
    rename: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "no parent"))?;
    std::fs::create_dir_all(parent)?;
    let file_name = path
        .file_name()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "no file name"))?;

    static TEMP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let (temporary, mut file) = loop {
        let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut temporary_name = file_name.to_os_string();
        temporary_name.push(format!(".tmp-{}-{sequence}", std::process::id()));
        let temporary = parent.join(temporary_name);
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&temporary) {
            Ok(file) => break (temporary, file),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    };

    let result = write(&mut file, bytes).and_then(|()| {
        drop(file);
        rename(&temporary, path)
    });
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    } else {
        // An existing file keeps its old mode across a rename.
        crate::paths::restrict_file(path);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{
        flush_queue, write_atomically, write_atomically_with, Builder, Category, Entry, Queue,
    };
    use std::{
        path::PathBuf,
        sync::{atomic::AtomicU64, Mutex},
    };

    fn scratch(label: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!(
            "hyusk-persist-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&directory).expect("scratch directory");
        directory
    }

    fn bytes_builder(text: &'static str) -> Builder {
        Box::new(move || Some(text.as_bytes().to_vec()))
    }

    /// A fake writer: records what the queue handed it, in order.
    fn drain_with(queue: &Mutex<Queue>, written: &mut Vec<(Category, PathBuf, String)>) {
        let entries = queue
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        for entry in entries {
            let text = String::from_utf8((entry.build)().expect("built")).expect("utf-8");
            written.push((entry.category, entry.path, text));
        }
    }

    #[test]
    fn coalescing_keeps_one_pending_write_per_file() {
        let queue = Mutex::new(Queue::default());
        let session = PathBuf::from("/run/session.json");
        let status = PathBuf::from("/run/hyusk-status.json");
        {
            let mut queue = queue.lock().unwrap();
            queue.enqueue(Category::Session, session.clone(), bytes_builder("first"));
            queue.enqueue(Category::Status, status.clone(), bytes_builder("state-1"));
            queue.enqueue(Category::Session, session.clone(), bytes_builder("second"));
            queue.enqueue(Category::Session, session.clone(), bytes_builder("latest"));
            assert_eq!(queue.pending_len(), 2, "one entry per file");
        }

        let mut written = Vec::new();
        drain_with(&queue, &mut written);
        assert_eq!(
            written,
            vec![
                (Category::Session, session, "latest".to_string()),
                (Category::Status, status, "state-1".to_string()),
            ],
            "the superseded snapshots never reach the writer"
        );
        assert!(queue.lock().unwrap().pending_len() == 0);
    }

    #[test]
    fn a_superseded_snapshot_is_not_even_built() {
        let queue = Mutex::new(Queue::default());
        let path = PathBuf::from("/run/session.json");
        let built = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = std::sync::Arc::clone(&built);
        queue.lock().unwrap().enqueue(
            Category::Session,
            path.clone(),
            Box::new(move || {
                count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Some(b"stale".to_vec())
            }),
        );
        let count = std::sync::Arc::clone(&built);
        queue.lock().unwrap().enqueue(
            Category::Session,
            path,
            Box::new(move || {
                count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Some(b"fresh".to_vec())
            }),
        );
        let entries = queue.lock().unwrap().take();
        assert_eq!(entries.len(), 1);
        let entry = entries.into_iter().next().expect("one entry");
        assert_eq!((entry.build)().as_deref(), Some(&b"fresh"[..]));
        assert_eq!(
            built.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "only the kept snapshot is serialised"
        );
    }

    #[test]
    fn enqueue_does_no_file_io_and_flush_writes_the_latest_snapshot() {
        let directory = scratch("flush");
        let path = directory.join("session.json");
        let queue = Mutex::new(Queue::default());
        queue
            .lock()
            .unwrap()
            .enqueue(Category::Session, path.clone(), bytes_builder("first"));
        assert!(
            !path.exists(),
            "a queued write is not a write: the hot path does no IO"
        );
        queue
            .lock()
            .unwrap()
            .enqueue(Category::Session, path.clone(), bytes_builder("latest"));

        assert_eq!(flush_queue(&queue), 1, "shutdown flushes the queue");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "latest");
        assert_eq!(flush_queue(&queue), 0, "nothing is left to flush");
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_snapshot_that_cannot_be_built_is_dropped() {
        let directory = scratch("unbuildable");
        let path = directory.join("session.json");
        let queue = Mutex::new(Queue::default());
        queue.lock().unwrap().enqueue(
            Category::Session,
            path.clone(),
            Box::new(|| None) as Builder,
        );
        assert_eq!(flush_queue(&queue), 0);
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_failed_write_is_reported_and_keeps_the_previous_file() {
        let directory = scratch("failure");
        // A path whose parent is a file, so the directory cannot be created.
        let blocker = directory.join("blocker");
        std::fs::write(&blocker, "not a directory").unwrap();
        let path = blocker.join("session.json");
        assert!(write_atomically(&path, b"snapshot").is_err());
        assert_eq!(
            std::fs::read_to_string(&blocker).unwrap(),
            "not a directory"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn write_atomically_leaves_a_private_file_and_no_temp_files() {
        let directory = scratch("modes");
        let path = directory.join("hyusk-status.json");
        // An older file with a wide mode is replaced by a private one.
        std::fs::write(&path, "old").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        write_atomically(&path, b"new").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "the snapshot is owner-only");
        }
        let leftovers = std::fs::read_dir(&directory)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp-"))
            .count();
        assert_eq!(leftovers, 0, "the temporary file is renamed away");
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_failed_rename_keeps_the_previous_file_and_cleans_the_temp() {
        let directory = scratch("rename-failure");
        let path = directory.join("session.json");
        std::fs::write(&path, "previous").unwrap();
        let result = write_atomically_with(
            &path,
            b"replacement",
            std::io::Write::write_all,
            |_from, _to| Err(std::io::Error::other("injected rename failure")),
        );
        assert!(result.is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "previous");
        let leftovers = std::fs::read_dir(&directory)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp-"))
            .count();
        assert_eq!(leftovers, 0);
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn an_older_snapshot_never_overwrites_a_newer_one() {
        let directory = scratch("ordering");
        let path = directory.join("session.json");
        write_atomically(&path, b"newest").unwrap();
        // Simulate a newer generation already on disk, without holding the
        // ordering lock across `write_entry` (which takes it itself).
        {
            let mut written = super::written_lock();
            written.insert(path.clone(), 5);
        }
        let stale = Entry {
            category: Category::Session,
            path: path.clone(),
            generation: 4,
            build: bytes_builder("stale"),
        };
        assert!(!super::write_entry(stale), "an older snapshot is skipped");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "newest");
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn categories_name_themselves() {
        assert_eq!(Category::Session.as_str(), "session");
        assert_eq!(Category::Status.as_str(), "status");
    }
}
