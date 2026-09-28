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

// MppCoordinatorManager 过期清理单元测试。

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use astersql_store_copr::TiFlashReadTimeoutUltraLong;

use crate::{
    CoordinatorUniqueID, InstanceMPPCoordinatorManager, MppCoordinator, MppQueryId,
    ReportTaskStatusRequest, detectFrequency,
};

/// 空闲协调器：`is_closed` 恒为 true，便于测试过期删除路径。
#[allow(dead_code)]
struct IdleCoordinator;

impl IdleCoordinator {
    // Execute implements MppCoordinator interface function.
    /// 占位：对应 Go Execute 接口。
    fn execute(&self) -> (Option<()>, Vec<()>, Option<()>) {
        (None, Vec::new(), None)
    }

    // Next implements MppCoordinator interface function.
    /// 占位：对应 Go Next 接口。
    fn next(&self) -> (Option<()>, Option<()>) {
        (None, None)
    }

    // Close implements MppCoordinator interface function.
    /// 占位：对应 Go Close 接口。
    fn close(&self) -> Result<(), ()> {
        Ok(())
    }

    // GetNodeCnt implements MppCoordinator interface function.
    /// 占位：对应 Go GetNodeCnt 接口。
    fn get_node_cnt(&self) -> usize {
        0
    }
}

impl MppCoordinator for IdleCoordinator {
    // ReportStatus implements MppCoordinator interface function.
    /// 空实现：上报始终成功。
    fn report_status(
        &self,
        _request: &ReportTaskStatusRequest,
    ) -> Result<(), crate::CoordinatorError> {
        Ok(())
    }

    // IsClosed implements MppCoordinator interface function.
    /// 恒为已关闭，满足 detect_and_delete 的删除前置条件。
    fn is_closed(&self) -> bool {
        true
    }
}

/// 注册多个不同 query_ts 的协调器，断言超时检测只删掉过期项。
#[test]
fn test_detect_and_delete() {
    // Isolate from leftover global registrations (Go mutates the same singleton map).
    // 清空单例上可能残留的注册（与 Go 共用全局 map 语义一致）。
    for id in InstanceMPPCoordinatorManager.coordinator_ids() {
        InstanceMPPCoordinatorManager.unregister(id);
    }

    let start_ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_nanos()
        .min(u64::MAX as u128) as u64;
    let max_life_time = (TiFlashReadTimeoutUltraLong + detectFrequency)
        .as_nanos()
        .min(u64::MAX as u128) as u64;
    InstanceMPPCoordinatorManager.set_max_lifetime_nanos(max_life_time);

    // OverTime One with GatherID 1
    // query_ts = start_ts，在后续 now 下已超时。
    let query_id1 = MppQueryId {
        query_ts: start_ts,
        ..MppQueryId::default()
    };
    let unique_id1 = CoordinatorUniqueID {
        mpp_query_id: query_id1,
        gather_id: 1,
    };
    InstanceMPPCoordinatorManager
        .register(unique_id1, Arc::new(IdleCoordinator))
        .expect("register gather 1");

    // Not OverTime One with GatherID 2
    // query_ts 推到 max_lifetime，相对检测时刻仍未超时。
    let query_id2 = MppQueryId {
        query_ts: start_ts + InstanceMPPCoordinatorManager.max_lifetime_nanos(),
        ..MppQueryId::default()
    };
    let unique_id2 = CoordinatorUniqueID {
        mpp_query_id: query_id2,
        gather_id: 2,
    };
    InstanceMPPCoordinatorManager
        .register(unique_id2, Arc::new(IdleCoordinator))
        .expect("register gather 2");

    // Not OverTime One with GatherID 3
    let query_id3 = MppQueryId {
        query_ts: start_ts + detectFrequency.as_nanos() as u64,
        ..MppQueryId::default()
    };
    let unique_id3 = CoordinatorUniqueID {
        mpp_query_id: query_id3,
        gather_id: 3,
    };
    InstanceMPPCoordinatorManager
        .register(unique_id3, Arc::new(IdleCoordinator))
        .expect("register gather 3");

    // OverTime One with GatherID 4
    let query_id4 = MppQueryId {
        query_ts: start_ts + Duration::from_secs(60).as_nanos() as u64,
        ..MppQueryId::default()
    };
    let unique_id4 = CoordinatorUniqueID {
        mpp_query_id: query_id4,
        gather_id: 4,
    };
    InstanceMPPCoordinatorManager
        .register(unique_id4, Arc::new(IdleCoordinator))
        .expect("register gather 4");

    // OverTime One with GatherID 5
    let query_id5 = MppQueryId {
        query_ts: start_ts + Duration::from_secs(20).as_nanos() as u64,
        ..MppQueryId::default()
    };
    let unique_id5 = CoordinatorUniqueID {
        mpp_query_id: query_id5,
        gather_id: 5,
    };
    InstanceMPPCoordinatorManager
        .register(unique_id5, Arc::new(IdleCoordinator))
        .expect("register gather 5");

    // now = start + max_lifetime + 120s：仅 gather 2/3 仍存活。
    InstanceMPPCoordinatorManager.detect_and_delete(
        start_ts
            + InstanceMPPCoordinatorManager.max_lifetime_nanos()
            + Duration::from_secs(120).as_nanos() as u64,
    );
    assert_eq!(InstanceMPPCoordinatorManager.coordinator_count(), 2);
    for id in InstanceMPPCoordinatorManager.coordinator_ids() {
        assert!(id.gather_id == 2 || id.gather_id == 3);
    }
}

/// Go's uint64 deadline calculation wraps on overflow; preserve that boundary behavior.
#[test]
fn detect_and_delete_wraps_query_deadline_like_go() {
    let manager = crate::MppCoordinatorManager::new(Duration::from_secs(1));
    manager.set_max_lifetime_nanos(10);
    let id = CoordinatorUniqueID {
        mpp_query_id: MppQueryId {
            query_ts: u64::MAX - 5,
            ..MppQueryId::default()
        },
        gather_id: 1,
    };
    manager
        .register(id, Arc::new(IdleCoordinator))
        .expect("register overflow-boundary coordinator");

    assert_eq!(manager.detect_and_delete(10), vec![id]);
    assert_eq!(manager.coordinator_count(), 0);
}
