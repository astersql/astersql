// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

//! Go `main_test.go` / TestMain lifecycle stand-in.
//! Go starts mock.Cluster + goleak; Rust uses Mem* fixtures (no kv/domain).
//! 提供包级共享 MockCluster，替代 Go TestMain 的 NewCluster/Start/Stop 流程。
//! 不引入 goleak：Rust 侧靠 Mem 夹具与进程退出回收，避免原生泄漏检测依赖。

use std::sync::{Mutex, OnceLock};

use crate::stubs::domain::Domain;
use crate::stubs::metapb;
use crate::stubs::pd::MemPdClient;

/// Package-level fixture analogous to Go `var mc *mock.Cluster`.
/// 内存 PD + 空 Domain，足够本包测试路由到 store 1，无需真实 TiKV。
pub struct MockCluster {
    pub pd: MemPdClient,
    pub domain: Domain,
}

impl MockCluster {
    pub fn new() -> Self {
        Self {
            // cluster_id=1、单 Up store，与 Go mock 默认拓扑对齐。
            pd: MemPdClient {
                cluster_id: 1,
                stores: vec![metapb::Store {
                    Id: 1,
                    State: metapb::StoreState::Up,
                    ..Default::default()
                }],
            },
            domain: Domain::default(),
        }
    }

    // start/stop 在 Mem 夹具上为空操作，保留 API 以对照 Go 生命周期调用点。
    pub fn start(&mut self) {}
    pub fn stop(&mut self) {}
}

// OnceLock 保证进程内只初始化一次，模拟 Go 包级 `mc`。
static MC: OnceLock<Mutex<MockCluster>> = OnceLock::new();

/// Corresponds to Go TestMain initialization of shared `mc`.
/// 首次调用时 start，之后各测试共享同一 Mutex 保护的集群实例。
pub fn shared_cluster() -> &'static Mutex<MockCluster> {
    MC.get_or_init(|| {
        let mut mc = MockCluster::new();
        mc.start();
        Mutex::new(mc)
    })
}

#[test]
fn test_main_initializes_shared_cluster() {
    // Go TestMain: SetupForCommonTest + NewCluster/Start + m.Run + Stop + goleak.
    // 断言共享集群已就绪，并调用 stop 覆盖生命周期对称性。
    let guard = shared_cluster().lock().unwrap();
    assert_eq!(guard.pd.cluster_id, 1);
    drop(guard);
    shared_cluster().lock().unwrap().stop();
}
