// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::any::Any;
use std::sync::atomic::Ordering;
use std::time::Duration;

use astersql_config::diagnosticmode;
use tikv_client::proto::{coprocessor, debugpb, kvrpcpb, mpp};
use tikv_client::rpc_interceptor::RpcInterceptor;

use crate::DiagnosticRpcGuard;

fn assert_kv_allowed(guard: &DiagnosticRpcGuard, request: &dyn Any) {
    let mut timeout = Duration::from_secs(1);
    guard
        .before(request, &mut timeout)
        .expect("diagnostic read RPC must be forwarded");
}

fn assert_kv_blocked(guard: &DiagnosticRpcGuard, request: &dyn Any) {
    let mut timeout = Duration::from_secs(1);
    let error = guard
        .before(request, &mut timeout)
        .expect_err("diagnostic write or unknown RPC must be blocked");
    assert!(
        matches!(
            error,
            tikv_client::Error::GrpcAPI(ref status)
                if status.code() == tonic::Code::PermissionDenied
        ),
        "unexpected error: {error:?}"
    );
}

#[test]
fn diagnostic_rpc_guards_match_go_allowlists() {
    let previous_in_test = astersql_util_intest::InTest.swap(true, Ordering::SeqCst);
    struct RestoreInTest(bool);
    impl Drop for RestoreInTest {
        fn drop(&mut self) {
            astersql_util_intest::InTest.store(self.0, Ordering::SeqCst);
        }
    }
    let _restore_in_test = RestoreInTest(previous_in_test);
    let guard = DiagnosticRpcGuard;

    let restore = diagnosticmode::SetForTest(false);
    assert_kv_allowed(&guard, &kvrpcpb::PrewriteRequest::default());
    guard
        .before_pd("/pdpb.PD/UpdateGCSafePoint")
        .expect("default mode must preserve unrestricted clients");
    restore();

    let restore = diagnosticmode::SetForTest(true);
    for method in [
        "/pdpb.PD/GetMembers",
        "/pdpb.PD/GetClusterInfo",
        "/pdpb.PD/GetStore",
        "/routerpb.Router/GetStore",
        "/pdpb.PD/GetAllStores",
        "/routerpb.Router/GetAllStores",
        "/pdpb.PD/GetRegion",
        "/routerpb.Router/GetRegion",
        "/pdpb.PD/GetPrevRegion",
        "/routerpb.Router/GetPrevRegion",
        "/pdpb.PD/GetRegionByID",
        "/routerpb.Router/GetRegionByID",
        "/pdpb.PD/ScanRegions",
        "/pdpb.PD/BatchScanRegions",
        "/routerpb.Router/BatchScanRegions",
        "/pdpb.PD/GetOperator",
        "/pdpb.PD/LoadGlobalConfig",
        "/pdpb.PD/GetExternalTimestamp",
        "/keyspacepb.Keyspace/LoadKeyspace",
        "/keyspacepb.Keyspace/LoadKeyspaceByID",
        "/keyspacepb.Keyspace/GetAllKeyspaces",
        "/meta_storagepb.MetaStorage/Get",
        "/resource_manager.ResourceManager/GetResourceGroup",
        "/pdpb.PD/GetGCState",
        "/pdpb.PD/GetAllKeyspacesGCStates",
        "/tsopb.TSO/FindGroupByKeyspaceID",
        "/grpc.health.v1.Health/Check",
        "/pdpb.PD/Tso",
        "/tsopb.TSO/Tso",
        "/pdpb.PD/WatchGlobalConfig",
        "/pdpb.PD/QueryRegion",
        "/routerpb.Router/QueryRegion",
        "/meta_storagepb.MetaStorage/Watch",
    ] {
        guard
            .before_pd(method)
            .unwrap_or_else(|error| panic!("{method} must be allowed: {error:?}"));
    }
    for method in [
        "/pdpb.PD/UpdateGCSafePoint",
        "/resource_manager.ResourceManager/AcquireTokenBuckets",
        "/pdpb.PD/UnknownMethod",
    ] {
        let error = guard
            .before_pd(method)
            .expect_err("unsafe PD method must be blocked");
        assert!(
            matches!(
                error,
                tikv_client::Error::GrpcAPI(ref status)
                    if status.code() == tonic::Code::PermissionDenied
            ),
            "unexpected error for {method}: {error:?}"
        );
    }

    assert_kv_allowed(&guard, &kvrpcpb::GetRequest::default());
    assert_kv_allowed(&guard, &kvrpcpb::BatchGetRequest::default());
    assert_kv_allowed(&guard, &kvrpcpb::ScanRequest::default());
    assert_kv_allowed(&guard, &kvrpcpb::RawGetRequest::default());
    assert_kv_allowed(&guard, &kvrpcpb::RawBatchGetRequest::default());
    assert_kv_allowed(&guard, &kvrpcpb::RawScanRequest::default());
    assert_kv_allowed(&guard, &kvrpcpb::RawGetKeyTtlRequest::default());
    assert_kv_allowed(&guard, &kvrpcpb::RawChecksumRequest::default());
    assert_kv_allowed(&guard, &coprocessor::Request::default());
    assert_kv_allowed(&guard, &coprocessor::BatchRequest::default());
    assert_kv_allowed(&guard, &kvrpcpb::MvccGetByKeyRequest::default());
    assert_kv_allowed(&guard, &kvrpcpb::MvccGetByStartTsRequest::default());
    assert_kv_allowed(&guard, &kvrpcpb::StoreSafeTsRequest::default());
    assert_kv_allowed(&guard, &kvrpcpb::GetLockWaitInfoRequest::default());
    assert_kv_allowed(&guard, &debugpb::GetRegionPropertiesRequest::default());
    assert_kv_allowed(&guard, &kvrpcpb::GetHealthFeedbackRequest::default());
    assert_kv_allowed(&guard, &kvrpcpb::TiFlashSystemTableRequest::default());
    assert_kv_allowed(&guard, &mpp::IsAliveRequest::default());
    assert_kv_allowed(&guard, &kvrpcpb::ScanLockRequest::default());
    assert_kv_allowed(&guard, &kvrpcpb::PhysicalScanLockRequest::default());
    assert_kv_allowed(&guard, &kvrpcpb::BufferBatchGetRequest::default());

    assert_kv_blocked(&guard, &kvrpcpb::PrewriteRequest::default());
    assert_kv_blocked(&guard, &kvrpcpb::ResolveLockRequest::default());
    assert_kv_blocked(&guard, &0_u8);
    restore();
}
