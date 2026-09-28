// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// cgmon 单元测试：cgroup 探针失败时仍回退到主机默认 CPU/内存值。

#![allow(non_snake_case)]

use super::CgroupMonitor;

/// 模拟无 cgroup：CPU/内存探针报错，但 last_* 仍应等于主机探测值。
#[test]
fn test_upload_default_value_without_cgroup() {
    use anyhow::anyhow;
    use std::thread;
    use std::time::Duration;
    use sysinfo::System;

    let runtime_cpu = thread::available_parallelism()
        .expect("runtime CPU count must be available")
        .get();
    let mut system = System::new();
    system.refresh_memory();
    let total_memory = system.total_memory();

    // 长刷新间隔避免后台干扰；cgroup 探针故意失败。
    let monitor = CgroupMonitor::with_probes(
        Duration::from_secs(3600),
        move || Ok(runtime_cpu),
        || Err(anyhow!("mock error")),
        move || Ok(total_memory),
        || Err(anyhow!("mock error")),
    );

    assert!(monitor.refresh_cgroup_cpu().is_err());
    assert!(monitor.refresh_cgroup_memory().is_err());
    assert_eq!(
        i32::try_from(runtime_cpu).expect("runtime CPU count must fit i32"),
        monitor.last_cpu()
    );
    assert_eq!(total_memory, monitor.last_memory_limit());
}
