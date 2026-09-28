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

// 内存配额根（MemRoot）与磁盘风险判断相关单元测试。

use crate::disk_root::risk_of_disk_full;
use crate::mem_root::{MemRoot, MemRootImpl};

/// 覆盖配额设置、consume/release、按标签记账与预检边界。
#[test]
fn test_memory_root() {
    let memory = MemRootImpl::new(1024);
    assert_eq!(memory.max_memory_quota(), 1024);
    assert_eq!(memory.current_usage(), 0);

    assert!(memory.check_consume(1023));
    assert!(memory.check_consume(1024));
    assert!(!memory.check_consume(1025));

    memory.consume(512);
    assert_eq!(memory.current_usage(), 512);
    assert!(memory.check_consume(512));
    assert!(!memory.check_consume(513));
    assert_eq!(memory.max_memory_quota(), 1024);

    memory.release(10);
    assert_eq!(memory.current_usage(), 502);
    assert_eq!(memory.max_memory_quota(), 1024);
    memory.set_max_memory_quota(512);
    assert!(!memory.check_consume(20));
    memory.release(502);

    // 按标签占用：与 Go 一致，同一标签再次 consume 会累计。
    assert_eq!(memory.current_usage(), 0);
    memory.set_max_memory_quota(1024);
    memory.consume_with_tag("a", 512);
    memory.consume_with_tag("b", 512);
    assert_eq!(memory.current_usage(), 1024);
    assert_eq!(memory.current_usage_with_tag("a"), 512);
    memory.consume_with_tag("a", 10);
    assert_eq!(memory.current_usage(), 1034);
    assert_eq!(memory.current_usage_with_tag("a"), 522);
    assert!(!memory.check_consume(1));
    memory.release_with_tag("a");
    assert_eq!(memory.current_usage(), 512);

    // 重复释放同一标签应无额外效果。
    memory.release_with_tag("a");
    assert_eq!(memory.current_usage(), 512);
    assert!(memory.check_consume(10));
    memory.consume(10);
    assert_eq!(memory.current_usage(), 522);
}

/// Go 使用原始 int64 算术，不钳制负配额、负消费或超额释放。
#[test]
fn test_memory_root_preserves_signed_accounting() {
    let memory = MemRootImpl::new(-1);
    assert_eq!(memory.max_memory_quota(), -1);
    assert!(!memory.check_consume(0));

    memory.consume(-5);
    assert_eq!(memory.current_usage(), -5);
    assert!(memory.check_consume(4));

    memory.release(3);
    assert_eq!(memory.current_usage(), -8);
    memory.set_max_memory_quota(-10);
    assert!(!memory.check_consume(0));

    memory.consume_with_tag("signed", -2);
    assert_eq!(memory.current_usage_with_tag("signed"), -2);
    assert_eq!(memory.current_usage(), -10);
    memory.release_with_tag("signed");
    assert_eq!(memory.current_usage(), -8);

    memory.refresh_consumption();
}

/// 验证磁盘即将用尽的风险阈值：可用空间占比过低时返回 true。
#[test]
fn test_risk_of_disk_full() {
    assert!(!risk_of_disk_full(11, 100));
    assert!(!risk_of_disk_full(10, 100));
    assert!(risk_of_disk_full(9, 100));
}
