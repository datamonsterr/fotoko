//! Persistent job ledger: which job ids were accepted, and how each ended. It keeps a redelivered
//! job from printing twice, even across restarts, and finds jobs a crash left unfinished.
//!
//! The cloud republishes an order's job under the same job id, so a job id is only refused while
//! it is in progress or ended well. A job that failed with `download_failed` (typically an expired
//! presigned URL) may be accepted again.

use std::collections::{HashMap, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use fotoko_protocol::PiErrorCode;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// File name inside the state directory.
pub const FILE_NAME: &str = "jobs.jsonl";
/// Most job ids kept across restarts; the oldest are forgotten first.
pub const MAX_REMEMBERED: usize = 2000;

/// Ledger failure.
#[derive(Debug, Error)]
#[error("job ledger {path}: {source}")]
pub struct StoreError {
    path: PathBuf,
    #[source]
    source: io::Error,
}

/// How a finished job ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", content = "code", rename_all = "snake_case")]
pub enum Outcome {
    /// All copies printed.
    Completed,
    /// Failed with this code.
    Failed(PiErrorCode),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Entry {
    job_id: String,
    order_id: String,
    /// `None` while the job is in progress.
    outcome: Option<Outcome>,
}

/// A job accepted but never finished (the agent stopped in the middle of it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unfinished {
    /// Job id.
    pub job_id: String,
    /// Order id.
    pub order_id: String,
}

/// Append-only ledger (one JSON object per line, the last line per job wins) plus an index.
#[derive(Debug)]
pub struct DedupeStore {
    path: PathBuf,
    file: File,
    entries: HashMap<String, Entry>,
}

impl DedupeStore {
    /// Open (creating if needed) the ledger in `state_dir`. Unreadable lines, such as a torn
    /// final line after a crash, are skipped. When more than [`MAX_REMEMBERED`] jobs are stored
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
        // Order of last update, oldest first.
        let mut order: VecDeque<String> = VecDeque::new();
        let mut entries: HashMap<String, Entry> = HashMap::new();
        for entry in text
            .lines()
            .filter_map(|l| serde_json::from_str::<Entry>(l).ok())
        {
            order.retain(|id| *id != entry.job_id);
            order.push_back(entry.job_id.clone());
            entries.insert(entry.job_id.clone(), entry);
        }
        let torn = !text.is_empty() && !text.ends_with('\n');
        let lines = text.lines().count();
        if order.len() > limit || torn || lines > order.len() * 2 + 16 {
            while order.len() > limit {
                if let Some(old) = order.pop_front() {
                    entries.remove(&old);
                }
            }
            let mut rewritten = String::new();
            for id in &order {
                if let Some(entry) = entries.get(id) {
                    rewritten.push_str(&serde_json::to_string(entry).map_err(|e| err(e.into()))?);
                    rewritten.push('\n');
                }
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
        Ok(Self {
            path,
            file,
            entries,
        })
    }

    /// A ledger whose writes always fail (`/dev/full`), for testing error handling.
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
            entries: HashMap::new(),
        }
    }

    fn append(&mut self, entry: Entry) -> Result<(), StoreError> {
        let err = |source| StoreError {
            path: self.path.clone(),
            source,
        };
        let mut line = serde_json::to_string(&entry).map_err(|e| err(e.into()))?;
        line.push('\n');
        self.file.write_all(line.as_bytes()).map_err(err)?;
        self.file.sync_data().map_err(err)?;
        self.entries.insert(entry.job_id.clone(), entry);
        Ok(())
    }

    /// How the job ended, if the ledger knows it (`None` also while it is in progress).
    pub fn outcome(&self, job_id: &str) -> Option<Outcome> {
        self.entries.get(job_id).and_then(|e| e.outcome)
    }

    /// Whether the ledger has any record of the job.
    pub fn contains(&self, job_id: &str) -> bool {
        self.entries.contains_key(job_id)
    }

    /// Accept a job. Returns `false` (writing nothing) when the job id is in progress or ended
    /// in anything but a `download_failed` failure.
    pub fn begin(&mut self, job_id: &str, order_id: &str) -> Result<bool, StoreError> {
        let retriable = match self.entries.get(job_id) {
            None => true,
            Some(entry) => entry.outcome == Some(Outcome::Failed(PiErrorCode::DownloadFailed)),
        };
        if !retriable {
            return Ok(false);
        }
        self.append(Entry {
            job_id: job_id.to_owned(),
            order_id: order_id.to_owned(),
            outcome: None,
        })?;
        Ok(true)
    }

    /// Record how a job ended.
    pub fn finish(
        &mut self,
        job_id: &str,
        order_id: &str,
        outcome: Outcome,
    ) -> Result<(), StoreError> {
        self.append(Entry {
            job_id: job_id.to_owned(),
            order_id: order_id.to_owned(),
            outcome: Some(outcome),
        })
    }

    /// Jobs accepted but never finished.
    pub fn unfinished(&self) -> Vec<Unfinished> {
        let mut jobs: Vec<Unfinished> = self
            .entries
            .values()
            .filter(|e| e.outcome.is_none())
            .map(|e| Unfinished {
                job_id: e.job_id.clone(),
                order_id: e.order_id.clone(),
            })
            .collect();
        jobs.sort_by(|a, b| a.job_id.cmp(&b.job_id));
        jobs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DL: Outcome = Outcome::Failed(PiErrorCode::DownloadFailed);

    #[test]
    fn begin_once_and_survive_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = DedupeStore::open(dir.path()).unwrap();
        assert!(!store.contains("j1"));
        assert!(store.begin("j1", "o1").unwrap());
        assert!(store.contains("j1"));
        assert!(!store.begin("j1", "o1").unwrap(), "in progress");
        store.finish("j1", "o1", Outcome::Completed).unwrap();
        assert!(!store.begin("j1", "o1").unwrap(), "completed");
        drop(store);
        let mut store = DedupeStore::open(dir.path()).unwrap();
        assert_eq!(store.outcome("j1"), Some(Outcome::Completed));
        assert!(!store.begin("j1", "o1").unwrap());
        assert!(store.begin("j2", "o2").unwrap());
    }

    #[test]
    fn download_failures_may_be_retried_other_failures_may_not() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = DedupeStore::open(dir.path()).unwrap();
        store.begin("a", "o").unwrap();
        store.finish("a", "o", DL).unwrap();
        assert!(
            store.begin("a", "o").unwrap(),
            "download_failed is retriable"
        );
        assert_eq!(store.outcome("a"), None, "back in progress");
        store.finish("a", "o", Outcome::Completed).unwrap();
        assert!(!store.begin("a", "o").unwrap());
        for code in [
            PiErrorCode::InvalidImage,
            PiErrorCode::PaperOut,
            PiErrorCode::RibbonOut,
            PiErrorCode::PrinterOffline,
            PiErrorCode::DriverError,
        ] {
            let id = format!("{code:?}");
            store.begin(&id, "o").unwrap();
            store.finish(&id, "o", Outcome::Failed(code)).unwrap();
            assert!(!store.begin(&id, "o").unwrap(), "{code:?}");
        }
        // The retry state survives a restart too.
        store.begin("b", "o").unwrap();
        store.finish("b", "o", DL).unwrap();
        drop(store);
        let mut store = DedupeStore::open(dir.path()).unwrap();
        assert!(store.begin("b", "o").unwrap());
    }

    #[test]
    fn unfinished_jobs_are_listed_after_restart() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = DedupeStore::open(dir.path()).unwrap();
        store.begin("done", "o1").unwrap();
        store.finish("done", "o1", Outcome::Completed).unwrap();
        store.begin("b-crashed", "o3").unwrap();
        store.begin("a-crashed", "o2").unwrap();
        drop(store);
        let store = DedupeStore::open(dir.path()).unwrap();
        assert_eq!(
            store.unfinished(),
            vec![
                Unfinished {
                    job_id: "a-crashed".into(),
                    order_id: "o2".into()
                },
                Unfinished {
                    job_id: "b-crashed".into(),
                    order_id: "o3".into()
                },
            ]
        );
    }

    #[test]
    fn creates_missing_state_dir() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a/b");
        let mut store = DedupeStore::open(&nested).unwrap();
        store.begin("x", "o").unwrap();
        assert!(nested.join(FILE_NAME).exists());
    }

    #[test]
    fn ids_with_awkward_characters_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let ids = ["a\nb", "quote\"d", "ünï", "../x", ""];
        let mut store = DedupeStore::open(dir.path()).unwrap();
        for id in ids {
            assert!(store.begin(id, "o").unwrap());
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
        let good = |id: &str| {
            serde_json::to_string(&Entry {
                job_id: id.into(),
                order_id: "o".into(),
                outcome: Some(Outcome::Completed),
            })
            .unwrap()
        };
        let text = format!("{}\nnot json\n{}\n{{\"job_id\":\"tor", good("a"), good("b"));
        fs::write(dir.path().join(FILE_NAME), text).unwrap();
        let mut store = DedupeStore::open(dir.path()).unwrap();
        assert!(store.contains("a") && store.contains("b"));
        assert!(!store.contains("tor"));
        // The next record must start on its own line, not glue onto the torn one.
        store.begin("c", "o").unwrap();
        drop(store);
        let store = DedupeStore::open(dir.path()).unwrap();
        assert!(store.contains("a") && store.contains("b") && store.contains("c"));
        assert!(!store.contains("tor"));
    }

    #[test]
    fn compacts_to_newest_when_over_limit() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = DedupeStore::open_with_limit(dir.path(), 3).unwrap();
        for id in ["1", "2", "3", "4", "5"] {
            store.begin(id, "o").unwrap();
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
    fn compacts_superseded_lines() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = DedupeStore::open(dir.path()).unwrap();
        for _ in 0..20 {
            store.begin("x", "o").unwrap();
            store.finish("x", "o", DL).unwrap();
        }
        drop(store);
        let store = DedupeStore::open(dir.path()).unwrap();
        assert_eq!(store.outcome("x"), Some(DL));
        let lines = fs::read_to_string(dir.path().join(FILE_NAME)).unwrap();
        assert_eq!(lines.lines().count(), 1);
    }

    #[test]
    fn write_failures_are_reported_without_remembering() {
        let mut store = DedupeStore::unwritable();
        let e = store.begin("j", "o").unwrap_err();
        assert!(e.to_string().contains("/dev/full"));
        assert!(!store.contains("j"));
        assert!(store.finish("j", "o", Outcome::Completed).is_err());
    }

    #[test]
    fn open_fails_when_state_dir_is_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("f");
        fs::write(&blocker, "x").unwrap();
        let e = DedupeStore::open(&blocker).unwrap_err();
        assert!(e.to_string().contains(FILE_NAME));
    }

    #[test]
    fn open_fails_when_store_path_is_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join(FILE_NAME)).unwrap();
        assert!(DedupeStore::open(dir.path()).is_err());
    }
}
