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
//! 对应 Go `main_test.go` 的包级测试生命周期：共享 mock 集群只初始化一次。
//! Rust 侧用内存 PD/Domain 桩代替真实 Cluster 拉起，避免依赖 kv/domain 进程。
//! 断言关注 cluster_id 与 empty Domain，证明 fixture 可被同包测试复用。

use std::sync::{Mutex, OnceLock};

use crate::stubs::{MemDomain, MemPdClient, metapb};

/// Package-level fixture analogous to Go `var mc *mock.Cluster`.
/// 包级共享 fixture：一把 Mutex 保护的 MemPdClient + MemDomain。
pub struct MockCluster {
    pub pd: MemPdClient,
    pub domain: MemDomain,
}

impl MockCluster {
    /// 构造最小可用集群：单 store Up、空 Domain，与 Go NewCluster 语义对齐。
    pub fn new() -> Self {
        Self {
            pd: MemPdClient {
                cluster_id: 1,
                stores: vec![metapb::Store {
                    Id: 1,
                    State: metapb::StoreState::Up,
                    ..Default::default()
                }],
            },
            domain: MemDomain {
                empty: true,
                ..Default::default()
            },
        }
    }

    pub fn start(&mut self) {
        // Go mc.Start(); Mem fixtures need no process bring-up.
        // 内存桩无需真正拉起进程，保留空实现以对齐 Go Start 调用点。
    }

    pub fn stop(&mut self) {
        // Go mc.Stop().
        // 停止同样是幂等空操作，供 TestMain 清理路径复用。
    }
}

static MC: OnceLock<Mutex<MockCluster>> = OnceLock::new();

/// Corresponds to Go TestMain initialization of shared `mc`.
/// OnceLock 保证多测试并发下只初始化一次，等价于 Go TestMain 里的全局 `mc`。
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
    // 验证共享集群已就绪：cluster_id=1 且 Domain 仍为空（未注入真实 schema）。
    let guard = shared_cluster().lock().unwrap();
    assert_eq!(guard.pd.cluster_id, 1);
    assert!(guard.domain.empty);
    drop(guard);
    // Cleanup path: Stop is idempotent for Mem fixtures.
    // 清理路径调用 stop，确认幂等且不 panic。
    shared_cluster().lock().unwrap().stop();
}
