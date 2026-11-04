//! Where Tap keeps its files, and the one thread that writes them (Search's `Store.swift`
//! and `Disk`).
//!
//! `~/Library/Application Support/Tap`, or `Tap (<world>)` for a test run (`TAP_PROBE`),
//! so a test run never reaches the real session. Writes go through one serial writer
//! that drops a write a newer one for the same file has already overtaken, and replaces
//! each file in one rename so a crash mid-write never leaves half a session.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        OnceLock,
        mpsc::{self, Receiver, Sender},
    },
    thread,
};

use tap_core::{history::History, identity, session};

/// The store folder, made on first use.
pub fn folder() -> &'static Path {
    static FOLDER: OnceLock<PathBuf> = OnceLock::new();
    FOLDER.get_or_init(|| {
        let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("/tmp"), PathBuf::from);
        let world = identity::world(std::env::var(identity::PROBE_ENV).ok().as_deref());
        let folder = home
            .join("Library/Application Support")
            .join(identity::folder_name(world.as_deref()));
        let _ = std::fs::create_dir_all(&folder);
        folder
    })
}

/// A file in the store folder.
pub fn file(name: &str) -> PathBuf {
    folder().join(name)
}

fn session_file() -> PathBuf {
    file("session.json")
}

/// Last time's session. A file that is there but won't decode is moved aside rather than
/// overwritten on the next save: something wrote it, and overwriting it without a trace
/// is how yesterday's tabs actually disappear.
pub fn read_session() -> session::Shape {
    let path = session_file();
    let Ok(data) = std::fs::read(&path) else {
        return session::Shape::default();
    };
    if let Some(shape) = session::Shape::decode(&data) {
        return shape;
    }
    quarantine(&path);
    session::Shape::default()
}

fn quarantine(path: &Path) {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let name = path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let _ = std::fs::rename(
        path,
        path.with_file_name(format!("{name}.unreadable-{stamp}")),
    );
}

/// Where you have been. A file that won't decode is moved aside, as the session is. The
/// second value says it was cleaned on the way in and should be written again now.
pub fn read_history() -> (History, bool) {
    let path = file("history.json");
    let Ok(data) = std::fs::read(&path) else {
        return (History::default(), false);
    };
    History::load(&data).unwrap_or_else(|| {
        quarantine(&path);
        (History::default(), false)
    })
}

/// Writes `history.json` on the writer thread.
pub fn save_history(data: Vec<u8>) {
    write(file("history.json"), data, false);
}

/// Writes any of Tap's files on the writer thread, newest winning.
pub fn save(path: PathBuf, data: Vec<u8>) {
    write(path, data, false);
}

/// Writes the session on the writer thread.
pub fn save_session(shape: &session::Shape) {
    write(session_file(), shape.encode(), false);
}

/// Writes the session before returning: quitting doesn't wait for a background thread.
pub fn save_session_now(shape: &session::Shape) {
    write(session_file(), shape.encode(), true);
}

struct Job {
    path: PathBuf,
    data: Vec<u8>,
    done: Option<Sender<()>>,
}

fn writer() -> &'static Sender<Job> {
    static WRITER: OnceLock<Sender<Job>> = OnceLock::new();
    WRITER.get_or_init(|| {
        let (send, receive) = mpsc::channel::<Job>();
        let _ = thread::Builder::new()
            .name("tap.disk".into())
            .spawn(move || drain(&receive));
        send
    })
}

fn drain(receive: &Receiver<Job>) {
    while let Ok(first) = receive.recv() {
        // Everything already waiting, newest per file winning: an older session
        // overtaken by a newer one is never written at all.
        let mut latest: HashMap<PathBuf, Job> = HashMap::new();
        let mut waiting = vec![];
        for job in std::iter::once(first).chain(receive.try_iter()) {
            if let Some(done) = latest.get_mut(&job.path).and_then(|old| old.done.take()) {
                waiting.push(done);
            }
            latest.insert(job.path.clone(), job);
        }
        for job in latest.into_values() {
            replace(&job.path, &job.data);
            if let Some(done) = job.done {
                let _ = done.send(());
            }
        }
        for done in waiting {
            let _ = done.send(());
        }
    }
}

fn write(path: PathBuf, data: Vec<u8>, now: bool) {
    if now {
        let (done, wait) = mpsc::channel();
        if writer()
            .send(Job {
                path,
                data,
                done: Some(done),
            })
            .is_ok()
        {
            let _ = wait.recv();
        }
    } else {
        let _ = writer().send(Job {
            path,
            data,
            done: None,
        });
    }
}

/// The file's new contents, whole or not at all.
fn replace(path: &Path, data: &[u8]) {
    let temporary = path.with_extension("tmp");
    if std::fs::write(&temporary, data).is_ok() {
        let _ = std::fs::rename(&temporary, path);
    }
}
