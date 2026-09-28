// Copyright 2026 AsterSQL.

use crate::aggregate::{hashAggExec, streamAggExec};
use crate::copr_handler::{CopError, Datum, ExecDetail};
use crate::executor::{NextRow, executor};

struct EmptyExec;

impl executor for EmptyExec {
    fn SetSrcExec(&mut self, _source: Option<Box<dyn executor>>) {}

    fn GetSrcExec(&self) -> Option<&dyn executor> {
        None
    }

    fn ResetCounts(&mut self) {}

    fn Counts(&self) -> Vec<i64> {
        Vec::new()
    }

    fn Next(&mut self) -> Result<NextRow, CopError> {
        Ok(None)
    }

    fn ExecDetails(&self) -> Vec<ExecDetail> {
        Vec::new()
    }
}

struct FailingExec;

impl executor for FailingExec {
    fn SetSrcExec(&mut self, _source: Option<Box<dyn executor>>) {}

    fn GetSrcExec(&self) -> Option<&dyn executor> {
        None
    }

    fn ResetCounts(&mut self) {}

    fn Counts(&self) -> Vec<i64> {
        Vec::new()
    }

    fn Next(&mut self) -> Result<NextRow, CopError> {
        Err(CopError::EndOfStream)
    }

    fn ExecDetails(&self) -> Vec<ExecDetail> {
        Vec::new()
    }
}

#[test]
fn stream_aggregate_updates_exec_detail_on_empty_input() {
    let mut aggregate = streamAggExec::new(Vec::new(), Vec::new());
    aggregate.SetSrcExec(Some(Box::new(EmptyExec)));

    assert_eq!(aggregate.Next().unwrap(), Some(Vec::<Datum>::new()));
    assert_eq!(aggregate.execDetail.iterations, 1);
    assert_eq!(aggregate.execDetail.produced_rows, 1);

    assert_eq!(aggregate.Next().unwrap(), None);
    assert_eq!(aggregate.execDetail.iterations, 2);
    assert_eq!(aggregate.execDetail.produced_rows, 1);
}

#[test]
fn aggregates_update_exec_detail_when_the_source_errors() {
    let mut hash = hashAggExec::new(Vec::new(), Vec::new());
    hash.SetSrcExec(Some(Box::new(FailingExec)));
    assert_eq!(hash.Next().unwrap_err(), CopError::EndOfStream);
    assert_eq!(hash.execDetail.iterations, 1);
    assert_eq!(hash.execDetail.produced_rows, 0);

    let mut stream = streamAggExec::new(Vec::new(), Vec::new());
    stream.SetSrcExec(Some(Box::new(FailingExec)));
    assert_eq!(stream.Next().unwrap_err(), CopError::EndOfStream);
    assert_eq!(stream.execDetail.iterations, 1);
    assert_eq!(stream.execDetail.produced_rows, 0);
}
