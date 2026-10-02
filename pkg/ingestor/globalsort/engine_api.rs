// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::*;
use astersql_ingestor_engineapi as api;
use std::sync::mpsc::{SyncSender, TrySendError};
use std::time::Duration;

struct MonitorStop<'a>(&'a AtomicBool);
impl Drop for MonitorStop<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

/// The native region-job pipeline consumes the original external engine through
/// its common API. Only the loader owns mutable buffers; resource controls and
/// imported counters remain accessible while it loads or waits for consumers.
pub struct ExternalEngineAdapter {
    engine: Mutex<Engine>,
    controls: Arc<EngineResource>,
    size: i64,
    count: i64,
    imported_size: Arc<AtomicI64>,
    imported_count: Arc<AtomicI64>,
    range: (Vec<u8>, Vec<u8>),
    splits: Vec<Vec<u8>>,
    token: CancellationToken,
}
impl ExternalEngineAdapter {
    pub fn new(engine: Engine, token: CancellationToken) -> Self {
        Self {
            controls: engine.ResourceHandle(),
            size: engine.total_kv_size,
            count: engine.total_kv_count,
            imported_size: engine.imported_kv_size.clone(),
            imported_count: engine.imported_kv_count.clone(),
            range: engine.GetKeyRange(),
            splits: engine.GetRegionSplitKeys(),
            engine: Mutex::new(engine),
            token,
        }
    }
    pub fn ResourceHandle(&self) -> Arc<EngineResource> {
        self.controls.clone()
    }
    pub fn GetTotalLoadedKVsCount(&self) -> i64 {
        self.engine.lock().unwrap().GetTotalLoadedKVsCount()
    }
    pub fn RecordedDuplicateSize(&self) -> i64 {
        self.engine.lock().unwrap().recorded_duplicate_size
    }
    pub fn CloseShared(&self) -> Result<()> {
        self.engine.lock().map_err(|_| Error::Poisoned)?.Close()
    }
}
impl api::Engine for ExternalEngineAdapter {
    fn ID(&self) -> String {
        "external".into()
    }
    fn LoadIngestData(
        &self,
        context: &api::Context,
        out: &SyncSender<api::DataAndRanges>,
    ) -> std::result::Result<(), api::EngineError> {
        let done = AtomicBool::new(false);
        std::thread::scope(|scope| {
            let monitor = scope.spawn(|| {
                while !done.load(Ordering::Acquire) {
                    if context.is_cancelled() {
                        self.token.cancel();
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
            });
            let _stop = MonitorStop(&done);
            let result = self
                .engine
                .lock()
                .map_err(|_| Error::Poisoned)
                .and_then(|mut engine| {
                    engine.LoadIngestDataWith(&self.token, |batch| {
                        let mut outgoing = api::DataAndRanges {
                            Data: Box::new(DataAdapter(batch.data.clone())),
                            SortedRanges: batch
                                .sorted_ranges
                                .into_iter()
                                .map(|range| api::Range {
                                    Start: range.start,
                                    End: range.end,
                                })
                                .collect(),
                        };
                        loop {
                            if context.is_cancelled() || self.token.is_cancelled() {
                                batch.data.release();
                                return Err(Error::Cancelled);
                            }
                            match out.try_send(outgoing) {
                                Ok(()) => return Ok(()),
                                Err(TrySendError::Full(value)) => outgoing = value,
                                Err(TrySendError::Disconnected(_)) => {
                                    batch.data.release();
                                    return Err(Error::Closed);
                                }
                            }
                            std::thread::sleep(Duration::from_millis(1));
                        }
                    })
                });
            done.store(true, Ordering::Release);
            monitor.join().unwrap();
            result.map_err(|error| Box::new(error) as api::EngineError)
        })
    }
    fn KVStatistics(&self) -> (i64, i64) {
        (self.size, self.count)
    }
    fn ImportedStatistics(&self) -> (i64, i64) {
        (
            self.imported_size.load(Ordering::Relaxed),
            self.imported_count.load(Ordering::Relaxed),
        )
    }
    fn ConflictInfo(&self) -> api::ConflictInfo {
        let info = self.engine.lock().unwrap().ConflictInfo();
        api::ConflictInfo {
            Count: info.count,
            Files: info.files,
        }
    }
    fn GetKeyRange(&self) -> std::result::Result<(Vec<u8>, Vec<u8>), api::EngineError> {
        Ok(self.range.clone())
    }
    fn GetRegionSplitKeys(&self) -> std::result::Result<Vec<Vec<u8>>, api::EngineError> {
        Ok(self.splits.clone())
    }
    fn Close(&mut self) -> std::result::Result<(), api::EngineError> {
        self.engine
            .get_mut()
            .map_err(|_| Error::Poisoned)?
            .Close()?;
        Ok(())
    }
}
struct DataAdapter(MemoryIngestData);
impl api::IngestData for DataAdapter {
    fn GetFirstAndLastKey(
        &self,
        lower: &[u8],
        upper: &[u8],
    ) -> std::result::Result<(Option<Vec<u8>>, Option<Vec<u8>>), api::EngineError> {
        let (first, last) = self.0.GetFirstAndLastKey(lower, upper)?;
        Ok((
            (!first.is_empty()).then_some(first),
            (!last.is_empty()).then_some(last),
        ))
    }
    fn NewIter(
        &self,
        context: &api::Context,
        lower: &[u8],
        upper: &[u8],
        _: &mut astersql_lightning_membuf::Pool,
    ) -> Box<dyn api::ForwardIter> {
        let (iter, error) = match self.0.NewIter(lower, upper) {
            Ok(iter) => (Some(iter), None),
            Err(error) => (None, Some(Box::new(error) as api::EngineError)),
        };
        Box::new(IterAdapter {
            iter,
            error,
            context: context.clone(),
        })
    }
    fn GetTS(&self) -> u64 {
        self.0.GetTS()
    }
    fn IncRef(&self) {
        self.0.IncRef();
    }
    fn DecRef(&self) {
        self.0.DecRef();
    }
    fn Finish(&self, size: i64, count: i64) {
        self.0.Finish(size, count);
    }
}
struct IterAdapter {
    iter: Option<MemoryDataIter>,
    error: Option<api::EngineError>,
    context: api::Context,
}
impl IterAdapter {
    fn check(&mut self) -> bool {
        if self.context.is_cancelled() {
            self.error = Some(Box::new(Error::Cancelled));
            return false;
        }
        self.error.is_none()
    }
}
impl api::ForwardIter for IterAdapter {
    fn First(&mut self) -> bool {
        self.check() && self.iter.as_mut().is_some_and(|iter| iter.First())
    }
    fn Valid(&self) -> bool {
        !self.context.is_cancelled()
            && self.error.is_none()
            && self.iter.as_ref().is_some_and(|iter| iter.Valid())
    }
    fn Next(&mut self) -> bool {
        self.check() && self.iter.as_mut().is_some_and(|iter| iter.Next())
    }
    fn Key(&self) -> &[u8] {
        self.iter.as_ref().expect("iterator is closed").Key()
    }
    fn Value(&self) -> &[u8] {
        self.iter.as_ref().expect("iterator is closed").Value()
    }
    fn Close(&mut self) -> std::result::Result<(), api::EngineError> {
        if let Some(mut iter) = self.iter.take() {
            iter.Close()?;
        }
        Ok(())
    }
    fn Error(&self) -> Option<&api::EngineError> {
        self.error.as_ref()
    }
    fn ReleaseBuf(&mut self) {
        if let Some(iter) = &mut self.iter {
            iter.ReleaseBuf();
        }
    }
}

#[cfg(test)]
#[path = "engine_api_test.rs"]
mod tests;
