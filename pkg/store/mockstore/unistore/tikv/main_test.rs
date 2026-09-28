// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// unistore tikv 行为测试。
use crate::inner_server::{DatabaseBundle, InnerServer, StandAloneInnerServer};
use crate::mock_region::{MockRegionManager, Peer, Region, RegionEpoch, Store};
use crate::mvcc::{Lock, MutationOp, MvccStore, SafePoint};
use crate::region::{RegionManager, RegionOptions, RequestContext, StandAloneRegionManager};
use crate::server::Server;
use crate::server_batch::{BatchCommand, BatchCommandResponse, BatchRequestHandler};
use crate::write::{DbWriter, MemoryWriteBackend};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
/// Region 边界查找必须遵循 Go B-tree 的半开区间与 PrevRegion 规则。
fn region_lookup_handles_split_boundaries_and_previous_regions() {
    let manager = MockRegionManager::new(1, 1024);
    manager
        .bootstrap(
            vec![Store {
                id: 1,
                address: "store-1".into(),
                labels: Vec::new(),
            }],
            Region {
                id: 1,
                start_key: Vec::new(),
                end_key: Vec::new(),
                epoch: RegionEpoch::default(),
                peers: vec![Peer { id: 1, store_id: 1 }],
            },
        )
        .unwrap();
    manager.split(1, 2, b"m".to_vec(), &[3]).unwrap();

    assert_eq!(1, manager.get_region_by_key(b"a").unwrap().meta.id);
    assert_eq!(2, manager.get_region_by_key(b"m").unwrap().meta.id);
    assert_eq!(2, manager.get_region_by_key(b"z").unwrap().meta.id);
    assert_eq!(2, manager.get_region_by_end_key(b"z").unwrap().meta.id);
}

#[derive(Default)]
struct TestBundle {
    closes: AtomicUsize,
}

impl DatabaseBundle for TestBundle {
    fn close(&self) -> Result<(), String> {
        self.closes.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }
}

fn test_server() -> (Server, Arc<TestBundle>) {
    let bundle = Arc::new(TestBundle::default());
    let region_manager = Arc::new(StandAloneRegionManager::new(
        Store {
            id: 1,
            address: "store-1".into(),
            labels: Vec::new(),
        },
        Region {
            id: 1,
            start_key: Vec::new(),
            end_key: Vec::new(),
            epoch: RegionEpoch {
                conf_ver: 1,
                version: 1,
            },
            peers: vec![Peer { id: 1, store_id: 1 }],
        },
        RegionOptions {
            store_address: "store-1".into(),
            pd_address: "pd-1".into(),
            region_size: 1024,
        },
    ));
    let inner = Arc::new(StandAloneInnerServer::new(Arc::clone(&bundle)));
    let store = Arc::new(MvccStore::new(Arc::new(SafePoint::new(0))));
    (Server::new(region_manager, store, inner), bundle)
}

fn test_context() -> crate::server::RpcContext {
    crate::server::RpcContext {
        region: RequestContext {
            region_id: 1,
            store_id: Some(1),
            epoch: Some(RegionEpoch {
                conf_ver: 1,
                version: 1,
            }),
        },
        ..crate::server::RpcContext::default()
    }
}

#[test]
/// standalone 服务停止时应关闭 MVCC、RegionManager 与底层 bundle，且幂等。
fn standalone_server_stop_is_idempotent_and_closes_resources() {
    let (server, bundle) = test_server();
    assert!(!server.mvcc_store().is_closed());
    server.stop().unwrap();
    server.stop().unwrap();
    assert!(server.mvcc_store().is_closed());
    assert_eq!(1, bundle.closes.load(Ordering::Acquire));
    assert!(server.raft().is_ok());
    assert!(server.batch_raft().is_ok());
    assert!(server.snapshot().is_ok());
}

#[test]
/// MPP 任务应按创建、投递、取消、移除顺序维护状态并拒绝重复创建。
fn server_mpp_task_lifecycle_matches_mock_contract() {
    let (server, _) = test_server();
    server.create_mpp_task(1, 7, b"payload".to_vec()).unwrap();
    assert!(server.create_mpp_task(1, 7, Vec::new()).is_err());
    server
        .dispatch_mpp_packet(1, 7, b"packet".to_vec())
        .unwrap();
    assert_eq!(
        vec![b"packet".to_vec()],
        server.establish_mpp_connection(1, 7).unwrap()
    );
    server.cancel_mpp_task(1, 7).unwrap();
    assert!(server.dispatch_mpp_packet(1, 7, Vec::new()).is_err());
    assert!(server.establish_mpp_connection(1, 7).is_err());
    assert_eq!(7, server.remove_mpp_task(1, 7).unwrap().task_id);
    assert!(server.establish_mpp_connection(1, 7).is_err());
}

#[test]
/// Batch handler 必须保留完成顺序中每条 Raw 响应与 request id 的关联。
fn batch_handler_preserves_response_id_association() {
    let (server, _) = test_server();
    server.raw_put(b"k".to_vec(), b"v".to_vec(), None, 0);
    let handler = BatchRequestHandler::new(&server);
    let response = handler.dispatch_batch(vec![
        (
            20,
            BatchCommand::RawGet {
                key: b"k".to_vec(),
                now_ts: 0,
            },
        ),
        (
            10,
            BatchCommand::RawDelete {
                key: b"other".to_vec(),
            },
        ),
    ]);
    let mut pairs = response
        .request_ids
        .into_iter()
        .zip(response.responses)
        .collect::<Vec<_>>();
    pairs.sort_by_key(|(request_id, _)| *request_id);
    assert_eq!(
        vec![
            (10, BatchCommandResponse::Empty),
            (20, BatchCommandResponse::Value(Some(b"v".to_vec())),),
        ],
        pairs
    );
}

#[test]
/// WriteBatch 应先写入锁，再提交版本并删除锁。
fn memory_db_writer_preserves_lock_then_commit_order() {
    let backend = Arc::new(MemoryWriteBackend::new(1024));
    let writer = DbWriter::new(Arc::clone(&backend));
    writer.open();
    let lock = Lock {
        primary: b"k".to_vec(),
        start_ts: 10,
        ttl: 100,
        op: MutationOp::Put,
        value: b"v".to_vec(),
        for_update_ts: 0,
        min_commit_ts: 11,
        use_async_commit: false,
        secondaries: Vec::new(),
        rollback_ts: Vec::new(),
    };
    let mut prewrite = writer.new_write_batch(10, 0);
    prewrite.prewrite(b"k".to_vec(), lock.clone());
    writer.write(prewrite).unwrap();
    assert_eq!(Some(lock.clone()), backend.get_lock(b"k"));

    let mut commit = writer.new_write_batch(10, 20);
    commit.commit(b"k".to_vec(), lock);
    writer.write(commit).unwrap();
    assert!(backend.get_lock(b"k").is_none());
    assert_eq!(1, backend.versions(b"k").len());
    writer.close();
    assert!(writer.write(writer.new_write_batch(30, 31)).is_err());
}

#[test]
/// Raw 范围扫描的空 end 表示无界，TTL 到期后条目不可见。
fn server_raw_scan_supports_unbounded_end_and_ttl_expiry() {
    let (server, _) = test_server();
    server.raw_put(b"a".to_vec(), b"va".to_vec(), Some(10), 100);
    server.raw_put(b"b".to_vec(), b"vb".to_vec(), None, 100);
    assert_eq!(2, server.raw_scan(b"a", b"", 10, false, false, 105).len());
    assert_eq!(None, server.raw_get(b"a", 110));
    assert_eq!(Some(5), server.raw_get_key_ttl(b"a", 105));
}

#[test]
/// RegionManager 必须拒绝错误 Store 与过期 Epoch，并接受当前上下文。
fn region_manager_validates_store_and_epoch_context() {
    let (server, _) = test_server();
    let context = test_context();
    assert!(server.get_store_id_by_address("store-1").is_ok());
    assert!(server.get_store_id_by_address("other").is_err());

    let manager = StandAloneRegionManager::new(
        Store {
            id: 1,
            address: "store-1".into(),
            labels: Vec::new(),
        },
        Region {
            id: 1,
            start_key: Vec::new(),
            end_key: Vec::new(),
            epoch: RegionEpoch {
                conf_ver: 1,
                version: 1,
            },
            peers: vec![Peer { id: 1, store_id: 1 }],
        },
        RegionOptions::default(),
    );
    assert!(manager.get_region_from_context(&context.region).is_ok());
    assert!(
        manager
            .get_region_from_context(&RequestContext {
                store_id: Some(2),
                ..context.region.clone()
            })
            .is_err()
    );
    assert!(
        manager
            .get_region_from_context(&RequestContext {
                epoch: Some(RegionEpoch {
                    conf_ver: 0,
                    version: 1,
                }),
                ..context.region
            })
            .is_err()
    );
}

#[test]
/// 超过锁存储单条容量上限时，Write 必须返回错误而不报告成功。
fn memory_db_writer_rejects_oversized_lock_entries() {
    let backend = Arc::new(MemoryWriteBackend::new(4));
    let writer = DbWriter::new(Arc::clone(&backend));
    writer.open();
    let lock = Lock {
        primary: b"primary".to_vec(),
        start_ts: 1,
        ttl: 1,
        op: MutationOp::Put,
        value: b"value".to_vec(),
        for_update_ts: 0,
        min_commit_ts: 2,
        use_async_commit: false,
        secondaries: Vec::new(),
        rollback_ts: Vec::new(),
    };
    let mut batch = writer.new_write_batch(1, 0);
    batch.prewrite(b"k".to_vec(), lock);
    assert!(writer.write(batch).is_err());
    writer.close();
}
