// Copyright 2026 AsterSQL.

//! Shared whole-file and row-group buffers for the production Parquet decoder.
use bytes::Bytes;
pub use parquet::errors::ParquetError as SourceError;
use parquet::errors::{ParquetError, Result};
use parquet::file::reader::{ChunkReader, Length};
use std::io::{Cursor, Read};
use std::sync::{Arc, Mutex};

pub const WHOLE_FILE_THRESHOLD: u64 = 32 << 20;
pub const ROW_GROUP_THRESHOLD: u64 = 128 << 20;
/// A range opener must return an independent stream for [start, end).
pub type RangeOpener = Arc<dyn Fn(u64, u64) -> Result<Box<dyn Read>> + Send + Sync>;

struct Buffer {
    start: u64,
    end: u64,
    bytes: Bytes,
}
struct State {
    closed: bool,
    whole: Option<Buffer>,
    group: Option<Buffer>,
    stream: Option<Buffer>,
    ranges: Vec<(u64, u64)>,
    column_ranges: Vec<(u64, u64)>,
    group_threshold: u64,
    peak_buffer: u64,
}
#[derive(Clone)]
pub struct SourceReader {
    size: u64,
    open: RangeOpener,
    state: Arc<Mutex<State>>,
}
fn load(open: &RangeOpener, start: u64, end: u64) -> Result<Buffer> {
    let length = usize::try_from(
        end.checked_sub(start)
            .ok_or_else(|| ParquetError::General("invalid range".into()))?,
    )
    .map_err(|_| ParquetError::General("range too large".into()))?;
    let mut bytes = vec![0; length];
    open(start, end)?.read_exact(&mut bytes)?;
    Ok(Buffer {
        start,
        end,
        bytes: bytes.into(),
    })
}
impl SourceReader {
    /// `known_size` is the exact size supplied by the caller; zero disables
    /// whole-file preload even when the footer opener discovers the size.
    pub fn prepare(
        known_size: i64,
        discover_size: impl FnOnce() -> Result<u64>,
        open: RangeOpener,
    ) -> Result<Self> {
        Self::prepare_with_thresholds(
            known_size,
            discover_size,
            open,
            WHOLE_FILE_THRESHOLD,
            ROW_GROUP_THRESHOLD,
        )
    }
    pub fn prepare_with_thresholds(
        known_size: i64,
        discover_size: impl FnOnce() -> Result<u64>,
        open: RangeOpener,
        whole_threshold: u64,
        group_threshold: u64,
    ) -> Result<Self> {
        let whole = if known_size > 0 && known_size as u64 <= whole_threshold {
            Some(load(&open, 0, known_size as u64)?)
        } else {
            None
        };
        let size = if whole.is_some() {
            known_size as u64
        } else {
            discover_size()?
        };
        if size > i64::MAX as u64 {
            return Err(ParquetError::General("source file too large".into()));
        }
        let peak = whole.as_ref().map_or(0, |buffer| buffer.end);
        Ok(Self {
            size,
            open,
            state: Arc::new(Mutex::new(State {
                closed: false,
                whole,
                group: None,
                stream: None,
                ranges: vec![],
                column_ranges: vec![],
                group_threshold,
                peak_buffer: peak,
            })),
        })
    }
    /// Register metadata after reading the footer. Each column shares its
    /// current row-group buffer while retaining an independent read cursor.
    pub fn set_ranges(&self, ranges: Vec<(u64, u64)>, columns: Vec<(u64, u64)>) -> Result<()> {
        if ranges
            .iter()
            .chain(&columns)
            .any(|&(start, end)| start > end || end > self.size)
        {
            return Err(ParquetError::General("invalid column chunk range".into()));
        }
        let mut state = self.state.lock().unwrap();
        state.ranges = ranges;
        state.column_ranges = columns;
        Ok(())
    }
    pub fn close(&self) {
        let mut state = self.state.lock().unwrap();
        state.closed = true;
        state.whole = None;
        state.group = None;
        state.stream = None;
    }
    pub fn whole_file_preloaded(&self) -> bool {
        self.state.lock().unwrap().whole.is_some()
    }
    pub fn buffer_bytes(&self) -> u64 {
        let state = self.state.lock().unwrap();
        state
            .whole
            .as_ref()
            .or(state.group.as_ref())
            .map_or(0, |buffer| buffer.end - buffer.start)
    }
    pub fn peak_buffer_bytes(&self) -> u64 {
        self.state.lock().unwrap().peak_buffer
    }
    fn cached(&self, start: u64, length: Option<usize>) -> Result<Option<Bytes>> {
        let mut state = self.state.lock().unwrap();
        if state.closed {
            return Err(ParquetError::General("reader is closed".into()));
        }
        if state.whole.is_none() {
            if let Some(&(begin, end)) = state
                .ranges
                .iter()
                .find(|&&(begin, end)| start >= begin && start < end)
            {
                if end - begin <= state.group_threshold
                    && state
                        .group
                        .as_ref()
                        .is_none_or(|buffer| buffer.start != begin || buffer.end != end)
                {
                    // Release the previous group before allocating the next one.
                    state.group = None;
                    let buffer = load(&self.open, begin, end)?;
                    state.peak_buffer = state.peak_buffer.max(end - begin);
                    state.group = Some(buffer);
                }
            }
        }
        for buffer in [
            state.whole.as_ref(),
            state.group.as_ref(),
            state.stream.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            let end = length.map_or(buffer.end, |length| start.saturating_add(length as u64));
            if start >= buffer.start && start < buffer.end && end <= buffer.end {
                return Ok(Some(buffer.bytes.slice(
                    (start - buffer.start) as usize..(end - buffer.start) as usize,
                )));
            }
        }
        Ok(None)
    }
}
impl Length for SourceReader {
    fn len(&self) -> u64 {
        self.size
    }
}
impl ChunkReader for SourceReader {
    type T = Box<dyn Read>;
    fn get_read(&self, start: u64) -> Result<Self::T> {
        if start > self.size {
            return Err(ParquetError::EOF("range beyond file".into()));
        }
        if start == self.size {
            return Ok(Box::new(Cursor::new(Bytes::new())));
        }
        let end = self
            .state
            .lock()
            .unwrap()
            .column_ranges
            .iter()
            .find(|&&(begin, end)| start >= begin && start < end)
            .map_or(self.size, |&(_, end)| end);
        if let Some(bytes) = self.cached(start, None)? {
            let cached_end = start + bytes.len() as u64;
            if cached_end >= end {
                return Ok(Box::new(Cursor::new(bytes)));
            }
            return Ok(Box::new(
                Cursor::new(bytes).chain((self.open)(cached_end, end)?),
            ));
        }
        let is_column = self
            .state
            .lock()
            .unwrap()
            .column_ranges
            .iter()
            .any(|&(begin, end)| start >= begin && start < end);
        if !is_column {
            return (self.open)(start, end);
        }
        // Keep only a bounded prefix so page-header probes and their immediately
        // following payload read share a request for small column chunks.
        let mut reader = (self.open)(start, end)?;
        let length = (end - start).min(crate::parser::DEFAULT_BUFFER_SIZE as u64) as usize;
        let mut bytes = vec![0; length];
        reader.read_exact(&mut bytes)?;
        let bytes = Bytes::from(bytes);
        self.state.lock().unwrap().stream = Some(Buffer {
            start,
            end: start + length as u64,
            bytes: bytes.clone(),
        });
        Ok(Box::new(Cursor::new(bytes).chain(reader)))
    }
    fn get_bytes(&self, start: u64, length: usize) -> Result<Bytes> {
        let end = start
            .checked_add(length as u64)
            .ok_or_else(|| ParquetError::General("offset overflow".into()))?;
        if end > self.size {
            return Err(ParquetError::EOF("range beyond file".into()));
        }
        if length == 0 {
            return Ok(Bytes::new());
        }
        if let Some(bytes) = self.cached(start, Some(length))? {
            return Ok(bytes);
        }
        Ok(load(&self.open, start, end)?.bytes)
    }
}
