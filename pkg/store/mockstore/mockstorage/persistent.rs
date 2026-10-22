// Copyright 2026 AsterSQL.

//! Durable transaction journal for the local canonical SQL store. A committed
//! record is synced before becoming visible; an incomplete final record is
//! discarded on reopen. File locks prevent concurrent writable instances.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Mutex;

use crate::storage::{MockStorageError, Result};
use fs2::FileExt;

pub(crate) type Writes = Vec<(Vec<u8>, Option<Vec<u8>>)>;
type Record = (u64, Writes);

pub(crate) struct Journal {
    file: Mutex<Option<File>>,
    read_only: bool,
}

fn error(e: impl ToString) -> MockStorageError {
    MockStorageError::Begin(format!("local transaction journal: {}", e.to_string()))
}

impl Journal {
    pub(crate) fn open(path: &Path, read_only: bool) -> Result<(Self, Vec<Record>)> {
        if !read_only {
            if path.is_dir()
                && !path.join("canonical-kv.wal").exists()
                && std::fs::read_dir(path).map_err(error)?.next().is_some()
            {
                return Err(error("directory contains unsupported storage data"));
            }
            std::fs::create_dir_all(path).map_err(error)?;
        }
        let mut file = OpenOptions::new()
            .read(true)
            .write(!read_only)
            .create(!read_only)
            .open(path.join("canonical-kv.wal"))
            .map_err(error)?;
        if read_only {
            FileExt::try_lock_shared(&file)
        } else {
            FileExt::try_lock_exclusive(&file)
        }
        .map_err(error)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).map_err(error)?;
        let complete = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
        let mut records = Vec::new();
        for line in bytes[..complete]
            .split(|b| *b == b'\n')
            .filter(|s| !s.is_empty())
        {
            let record: Record = serde_json::from_slice(line).map_err(error)?;
            if record.0 == 0 {
                return Err(error("invalid commit timestamp"));
            }
            records.push(record);
        }
        if !read_only && complete != bytes.len() {
            file.set_len(complete as u64).map_err(error)?;
            file.sync_all().map_err(error)?;
        }
        if !read_only {
            File::open(path)
                .and_then(|dir| dir.sync_all())
                .map_err(error)?;
        }
        file.seek(SeekFrom::End(0)).map_err(error)?;
        Ok((
            Self {
                file: Mutex::new(Some(file)),
                read_only,
            },
            records,
        ))
    }

    pub(crate) fn append(&self, ts: u64, writes: Writes) -> Result<()> {
        if writes.is_empty() {
            return Ok(());
        }
        if self.read_only {
            return Err(error("read-only local store"));
        }
        let mut guard = self.file.lock().expect("journal lock poisoned");
        let file = guard.as_mut().ok_or(MockStorageError::Closed)?;
        let mut bytes = serde_json::to_vec(&(ts, writes)).map_err(error)?;
        bytes.push(b'\n');
        let offset = file.stream_position().map_err(error)?;
        if let Err(e) = file.write_all(&bytes).and_then(|_| file.sync_data()) {
            // Do not leave a partial record in front of the next transaction.
            if file
                .set_len(offset)
                .and_then(|_| file.seek(SeekFrom::Start(offset)))
                .is_err()
            {
                *guard = None;
            }
            return Err(error(e));
        }
        Ok(())
    }

    pub(crate) fn close(&self) {
        self.file.lock().expect("journal lock poisoned").take();
    }
}
