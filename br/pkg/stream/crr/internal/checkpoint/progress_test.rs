// Copyright 2026 AsterSQL.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use astersql_br_pkg_streamhelper::Store;

use crate::{
    CalculatorDeps, CheckpointCalculatorConfig, Context, Error, NewCalculator, ObjectSyncChecker,
    PDMetaReader, UpstreamStorageReader, WalkOption,
};

const FAILING_META: &str = "v1/backupmeta/00000000000000010000000000000001-d0000000000000001l0000000000000001u0000000000000001.meta";
const BLOCKING_META: &str = "v1/backupmeta/00000000000000020000000000000001-d0000000000000002l0000000000000002u0000000000000002.meta";

struct UnusedPD;

impl PDMetaReader for UnusedPD {
    fn GetGlobalCheckpointForTask(&self, _: &Context, _: &str) -> Result<u64, Error> {
        unreachable!("plan_round does not query PD")
    }

    fn Stores(&self, _: &Context) -> Result<Vec<Store>, Error> {
        unreachable!("plan_round does not query PD")
    }
}

struct UnusedSync;

impl ObjectSyncChecker for UnusedSync {
    fn FileSynced(&self, _: &Context, _: &str) -> Result<bool, Error> {
        unreachable!("plan_round does not query downstream sync")
    }
}

struct FailThenBlockStorage {
    saw_round_cancellation: Arc<AtomicBool>,
}

impl UpstreamStorageReader for FailThenBlockStorage {
    fn WalkDir(
        &self,
        _: &Context,
        _: &WalkOption,
        callback: &mut dyn FnMut(&str, i64) -> Result<(), Error>,
    ) -> Result<(), Error> {
        callback(FAILING_META, 1)?;
        callback(BLOCKING_META, 1)
    }

    fn ReadFile(&self, ctx: &Context, name: &str) -> Result<Vec<u8>, Error> {
        if name == FAILING_META {
            return Err(Error::new("injected meta read failure"));
        }
        while !ctx.Done() {
            thread::sleep(Duration::from_millis(1));
        }
        if ctx
            .Err()
            .is_some_and(|err| err.message() == "context canceled")
        {
            self.saw_round_cancellation.store(true, Ordering::SeqCst);
        }
        Err(ctx.Err().expect("done context has an error"))
    }

    fn URI(&self) -> String {
        "file:///tmp/upstream".to_string()
    }
}

#[test]
fn plan_round_cancels_sibling_meta_reads_after_first_failure() {
    let saw_round_cancellation = Arc::new(AtomicBool::new(false));
    let calculator = NewCalculator(
        CalculatorDeps {
            PD: Box::new(UnusedPD),
            Upstream: Box::new(FailThenBlockStorage {
                saw_round_cancellation: Arc::clone(&saw_round_cancellation),
            }),
            Sync: Box::new(UnusedSync),
        },
        CheckpointCalculatorConfig {
            TaskName: "cancel-sibling-read".to_string(),
            MetaReadConcurrency: 2,
            ..Default::default()
        },
        None,
    )
    .expect("calculator");
    let parent = Context::Background();
    let (ctx, _cancel) = Context::WithTimeout(&parent, Duration::from_millis(200));

    let err = match calculator.plan_round(&ctx) {
        Ok(_) => panic!("injected meta read failure must fail the round"),
        Err(err) => err,
    };

    assert!(err.message().contains("injected meta read failure"));
    assert!(
        saw_round_cancellation.load(Ordering::SeqCst),
        "the first load error must cancel sibling reads through a round-local context"
    );
}
