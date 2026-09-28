// Copyright 2026 AsterSQL.

use super::mpp_gather::{MPPChunk, MPPGather, MPPGatherRuntime, PhysicalPlan};

#[derive(Clone)]
struct Plan;

impl PhysicalPlan for Plan {
    type ExchangeSender = ();

    fn id(&self) -> i32 {
        1
    }

    fn plan_type(&self) -> &str {
        "ExchangeSender"
    }

    fn children(&self) -> Vec<Self> {
        Vec::new()
    }

    fn as_exchange_sender(&self) -> Option<&Self::ExchangeSender> {
        static SENDER: () = ();
        Some(&SENDER)
    }
}

#[derive(Default)]
struct Runtime {
    close_calls: usize,
}

impl MPPGatherRuntime for Runtime {
    type Error = String;
    type Context = ();
    type Plan = Plan;
    type InfoSchema = ();
    type MPPQueryID = u64;
    type SelectResult = ();
    type MPPExecutor = ();
    type MemoryTracker = ();
    type ColumnInfo = ();
    type FieldType = ();
    type SchemaColumn = ();
    type Table = ();
    type KeyRange = ();

    fn error(&self, message: String) -> Self::Error {
        message
    }

    fn generate_root_mpp_tasks(
        &mut self,
        _: u64,
        _: Self::MPPQueryID,
        _: &<Self::Plan as PhysicalPlan>::ExchangeSender,
        _: &Self::InfoSchema,
    ) -> Result<Vec<Self::KeyRange>, Self::Error> {
        Ok(Vec::new())
    }

    fn new_executor_with_retry(
        &mut self,
        _: &Self::Context,
        _: &mut Self::MemoryTracker,
        _: &[i32],
        _: Self::Plan,
        _: u64,
        _: Self::MPPQueryID,
        _: &Self::InfoSchema,
    ) -> Result<Self::MPPExecutor, (Option<Self::MPPExecutor>, Self::Error)> {
        Ok(())
    }

    fn executor_key_ranges(&self, _: &Self::MPPExecutor) -> Vec<Self::KeyRange> {
        Vec::new()
    }

    fn select_result_from_mpp_response(
        &mut self,
        _: &[i32],
        _: i32,
        _: &mut Self::MPPExecutor,
    ) -> Self::SelectResult {
    }

    fn next_result<Q: MPPChunk>(
        &mut self,
        _: &mut Self::SelectResult,
        _: &Self::Context,
        _: &mut Q,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn close_result(&mut self, _: &mut Self::SelectResult) -> Result<(), Self::Error> {
        self.close_calls += 1;
        Ok(())
    }

    fn close_mpp_executor(&mut self, _: &mut Self::MPPExecutor) -> Result<(), Self::Error> {
        Ok(())
    }

    fn fill_virtual_column_values<Q: MPPChunk>(
        &mut self,
        _: &[Self::FieldType],
        _: &[usize],
        _: &[Self::ColumnInfo],
        _: &mut Q,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn executor_id(&self) -> i32 {
        1
    }
}

fn gather(dummy: bool) -> MPPGather<Runtime> {
    MPPGather {
        BaseExecutor: Runtime::default(),
        is: (),
        originalPlan: Plan,
        startTS: 1,
        mppQueryID: 2,
        respIter: Some(()),
        memTracker: (),
        columns: Vec::new(),
        virtualColumnIndex: Vec::new(),
        virtualColumnRetFieldTypes: Vec::new(),
        table: (),
        kvRanges: Vec::new(),
        dummy,
        mppExec: None,
    }
}

#[test]
fn close_preserves_response_iterator_like_go() {
    let mut gather = gather(false);

    gather.Close().unwrap();
    gather.Close().unwrap();

    assert!(gather.respIter.is_some());
    assert_eq!(gather.BaseExecutor.close_calls, 2);
}

#[test]
fn dummy_close_preserves_response_iterator_and_repeats_go_error() {
    let mut gather = gather(true);

    let first = gather.Close().unwrap_err();
    let second = gather.Close().unwrap_err();

    assert_eq!(first, "e.respIter != nil when e.dummy is set");
    assert_eq!(second, first);
    assert!(gather.respIter.is_some());
    assert_eq!(gather.BaseExecutor.close_calls, 2);
}
