// Copyright 2026 AsterSQL.

//! Focused parity regressions for `env.rs`.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use crate::env::{
    AdaptForGRPCInTest, CliEnv, Context, Env, LimitedBackoff, PrepareClient, Region,
    RegionCacheLike, StoreManagerLike, StringifyRangeOf, WithRetryV2, brpb, metapb,
};
use crate::errors::{Error, Result};

struct UnusedCache;

impl RegionCacheLike for UnusedCache {
    fn GetAllStores(&self, _ctx: &Context) -> Result<Vec<metapb::Store>> {
        panic!("unused")
    }

    fn LoadRegionsInKeyRange(
        &self,
        _ctx: &Context,
        _startKey: &[u8],
        _endKey: &[u8],
    ) -> Result<Vec<Box<dyn Region>>> {
        panic!("unused")
    }
}

struct FailingManager;

impl StoreManagerLike for FailingManager {
    fn ConnectPrepareClient(
        &self,
        _ctx: &Context,
        _storeID: u64,
    ) -> Result<Arc<dyn PrepareClient>> {
        Err(Error::new("dial failed"))
    }
}

#[test]
fn cli_env_preserves_store_manager_errors() {
    let env = CliEnv {
        Cache: Arc::new(UnusedCache),
        Mgr: Arc::new(FailingManager),
    };

    let err = match env.ConnectToStore(&Context::background(), 42) {
        Ok(_) => panic!("dial should fail"),
        Err(err) => err,
    };
    assert_eq!(err.to_string(), "dial failed");
}

struct PanicOnceClient {
    panic_send: AtomicBool,
    panic_recv: AtomicBool,
    sends: AtomicUsize,
    recvs: AtomicUsize,
}

impl PrepareClient for PanicOnceClient {
    fn Send(&self, _req: &brpb::PrepareSnapshotBackupRequest) -> Result<()> {
        if self.panic_send.swap(false, Ordering::SeqCst) {
            panic!("send panic")
        }
        self.sends.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn Recv(&self) -> Result<brpb::PrepareSnapshotBackupResponse> {
        if self.panic_recv.swap(false, Ordering::SeqCst) {
            panic!("recv panic")
        }
        self.recvs.fetch_add(1, Ordering::SeqCst);
        Ok(brpb::PrepareSnapshotBackupResponse::default())
    }
}

#[test]
fn grpc_adapter_unlocks_each_direction_after_inner_panic() {
    let inner = Arc::new(PanicOnceClient {
        panic_send: AtomicBool::new(true),
        panic_recv: AtomicBool::new(true),
        sends: AtomicUsize::new(0),
        recvs: AtomicUsize::new(0),
    });
    let adapter = AdaptForGRPCInTest(inner.clone() as Arc<dyn PrepareClient>);
    let request = brpb::PrepareSnapshotBackupRequest::default();

    assert!(catch_unwind(AssertUnwindSafe(|| adapter.Send(&request))).is_err());
    adapter.Send(&request).expect("send mutex must be reusable");

    assert!(catch_unwind(AssertUnwindSafe(|| adapter.Recv())).is_err());
    adapter.Recv().expect("recv mutex must be reusable");

    assert_eq!(inner.sends.load(Ordering::SeqCst), 1);
    assert_eq!(inner.recvs.load(Ordering::SeqCst), 1);
}

#[test]
fn with_retry_v2_formats_multiple_errors_like_go_multierr() {
    let mut attempt = 0;
    let result: Result<()> = WithRetryV2(
        &Context::background(),
        Box::new(LimitedBackoff {
            remaining: 2,
            delay: Duration::ZERO,
        }),
        |_| {
            attempt += 1;
            Err(Error::new(format!("failure-{attempt}")))
        },
    );

    assert_eq!(
        result.expect_err("all attempts should fail").to_string(),
        "failure-1; failure-2"
    );
}

#[test]
fn stringify_range_matches_go_logutil_contract() {
    assert_eq!(StringifyRangeOf(&[0x0a, 0xff], &[]), "[0AFF, inf)");
    assert_eq!(StringifyRangeOf(&[0x00], &[0x10]), "[00, 10)");
}
