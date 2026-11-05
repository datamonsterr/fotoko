//! Persistent set of job ids already accepted, so a redelivered job is never printed twice,
//! even across agent restarts.

use std::collections::{HashSet, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use thiserror::Error;

/// File name inside the state directory.
pub const FILE_NAME: &str = "seen-jobs.jsonl";
/// Most job ids kept across restarts; the oldest are forgotten first.
pub const MAX_REMEMBERED: usize = 2000;

/// Dedupe store failure.
#[derive(Debug, Error)]
#[error("dedupe store {path}: {source}")]
pub struct StoreError {
    path: PathBuf,
    #[source]
    source: io::Error,
}

/// Append-only file of job ids (one JSON string per line) plus an in-memory index.
#[derive(Debug)]
pub struct DedupeStore {
    path: PathBuf,
    file: File,
    seen: HashSet<String>,
}

impl DedupeStore {
    /// Open (creating if needed) the store in `state_dir`. Unreadable lines, such as a torn
    /// final line after a crash, are skipped. When more than [`MAX_REMEMBERED`] ids are stored
    /// the file is rewritten with the newest ones.
    pub fn open(state_dir: &Path) -> Result<Self, StoreError> {
        Self::open_with_limit(state_dir, MAX_REMEMBERED)
    }

    fn open_with_limit(state_dir: &Path, limit: usize) -> Result<Self, StoreError> {
        let path = state_dir.join(FILE_NAME);
        let err = |source| StoreError {
            path: path.clone(),
            source,
        };
        fs::create_dir_all(state_dir).map_err(err)?;
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(err(e)),
        };
        let mut order: VecDeque<String> = VecDeque::new();
        let mut seen = HashSet::new();
        for id in text
            .lines()
            .filter_map(|l| serde_json::from_str::<String>(l).ok())
        {
            if seen.insert(id.clone()) {
                order.push_back(id);
            }
        }
        let torn = !text.is_empty() && !text.ends_with('\n');
        if order.len() > limit || torn {
            while order.len() > limit {
                if let Some(old) = order.pop_front() {
                    seen.remove(&old);
                }
            }
            let mut rewritten = String::new();
            for id in &order {
                rewritten.push_str(&serde_json::to_string(id).map_err(|e| err(e.into()))?);
                rewritten.push('\n');
            }
            let tmp = path.with_extension("tmp");
            fs::write(&tmp, rewritten).map_err(err)?;
            fs::rename(&tmp, &path).map_err(err)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(err)?;
        Ok(Self { path, file, seen })
    }

    /// A store whose writes always fail (`/dev/full`), for testing error handling.
    #[cfg(test)]
    pub(crate) fn unwritable() -> Self {
        let path = PathBuf::from("/dev/full");
        let file = OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("/dev/full exists on Linux");
        Self {
            path,
            file,
            seen: HashSet::new(),
        }
    }

    /// Whether the job id was recorded.
    pub fn contains(&self, job_id: &str) -> bool {
        self.seen.contains(job_id)
    }

    /// Record a job id durably. Returns `false` (and writes nothing) when it was already there.
    pub fn record(&mut self, job_id: &str) -> Result<bool, StoreError> {
        if self.seen.contains(job_id) {
            return Ok(false);
        }
        let err = |source| StoreError {
            path: self.path.clone(),
            source,
        };
        let mut line = serde_json::to_string(job_id).map_err(|e| err(e.into()))?;
        line.push('\n');
        self.file.write_all(line.as_bytes()).map_err(err)?;
        self.file.sync_data().map_err(err)?;
        self.seen.insert(job_id.to_owned());
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_once_and_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = DedupeStore::open(dir.path()).unwrap();
        assert!(!store.contains("j1"));
        assert!(store.record("j1").unwrap());
        assert!(store.contains("j1"));
        assert!(!store.record("j1").unwrap());
        assert!(store.record("j2").unwrap());
        drop(store);
        let store = DedupeStore::open(dir.path()).unwrap();
        assert!(store.contains("j1") && store.contains("j2"));
        assert!(!store.contains("j3"));
    }

    #[test]
    fn creates_missing_state_dir() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a/b");
        let mut store = DedupeStore::open(&nested).unwrap();
        store.record("x").unwrap();
        assert!(nested.join(FILE_NAME).exists());
    }

    #[test]
    fn ids_with_awkward_characters_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let ids = ["a\nb", "quote\"d", "ünï", "../x", ""];
        let mut store = DedupeStore::open(dir.path()).unwrap();
        for id in ids {
            assert!(store.record(id).unwrap());
        }
        drop(store);
        let store = DedupeStore::open(dir.path()).unwrap();
        for id in ids {
            assert!(store.contains(id), "{id:?}");
        }
    }

    #[test]
    fn skips_garbage_lines_and_repairs_torn_tail() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(FILE_NAME), "\"a\"\nnot json\n\"b\"\n\"tor").unwrap();
        let mut store = DedupeStore::open(dir.path()).unwrap();
        assert!(store.contains("a") && store.contains("b"));
        assert!(!store.contains("tor"));
        // The next record must start on its own line, not glue onto the torn one.
        store.record("c").unwrap();
        drop(store);
        let store = DedupeStore::open(dir.path()).unwrap();
        assert!(store.contains("a") && store.contains("b") && store.contains("c"));
        assert!(!store.contains("tor"));
    }

    #[test]
    fn duplicate_lines_collapse() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(FILE_NAME), "\"a\"\n\"a\"\n").unwrap();
        let store = DedupeStore::open_with_limit(dir.path(), 10).unwrap();
        assert!(store.contains("a"));
    }

    #[test]
    fn compacts_to_newest_when_over_limit() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = DedupeStore::open_with_limit(dir.path(), 3).unwrap();
        for id in ["1", "2", "3", "4", "5"] {
            store.record(id).unwrap();
        }
        drop(store);
        let store = DedupeStore::open_with_limit(dir.path(), 3).unwrap();
        for id in ["1", "2"] {
            assert!(!store.contains(id), "{id}");
        }
        for id in ["3", "4", "5"] {
            assert!(store.contains(id), "{id}");
        }
        let lines = fs::read_to_string(dir.path().join(FILE_NAME)).unwrap();
        assert_eq!(lines.lines().count(), 3);
    }

    #[test]
    fn record_reports_write_failures_without_remembering() {
        let mut store = DedupeStore::unwritable();
        let e = store.record("j").unwrap_err();
        assert!(e.to_string().contains("/dev/full"));
        assert!(!store.contains("j"));
    }

    #[test]
    fn open_fails_when_state_dir_is_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("f");
        fs::write(&blocker, "x").unwrap();
        let e = DedupeStore::open(&blocker).unwrap_err();
        assert!(e.to_string().contains("seen-jobs.jsonl"));
    }

    #[test]
    fn open_fails_when_store_path_is_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join(FILE_NAME)).unwrap();
        assert!(DedupeStore::open(dir.path()).is_err());
    }
}
