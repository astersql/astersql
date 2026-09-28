// Copyright 2026 AsterSQL.

use std::any::Any;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tikv_client::proto::kvrpcpb;
use tikv_client::rpc_interceptor::{InterceptorGuard, RpcInterceptor, register};

struct ReadQueueDelay;
impl RpcInterceptor for ReadQueueDelay {
    fn delay(&self, request: &dyn Any) -> Duration {
        if !(request.is::<kvrpcpb::GetRequest>()
            || request.is::<kvrpcpb::BatchGetRequest>()
            || request.is::<kvrpcpb::ScanRequest>())
        {
            return Duration::ZERO;
        }
        Duration::from_millis(
            astersql_testkit_testfailpoint::eval_string("tikvclient/mockBatchClientSendDelay")
                .and_then(|value| value.parse().ok())
                .unwrap_or_default(),
        )
    }
}

pub(crate) fn install_read_queue_delay() {
    static GUARD: OnceLock<InterceptorGuard> = OnceLock::new();
    GUARD.get_or_init(|| register(Arc::new(ReadQueueDelay)));
}
