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

// 集群日志表（cluster log / information_schema 相关）归并堆的单元测试。
//
// `logResponseHeap` 按消息时间戳对多路日志流做最小堆排序，保证跨节点
// （TiDB / TiKV 等）日志按时间升序输出。

use crate::memtable_reader::{
    MemTableResult, logMessage, logResponseHeap, logStream, logStreamResult,
};

/// 空流：`next_messages` 立即返回结束，仅占位以满足 trait。
struct EmptyStream;
impl logStream for EmptyStream {
    fn next_messages(&mut self) -> MemTableResult<Option<Vec<logMessage>>> {
        Ok(None)
    }
}

fn stream_result(addr: &str, typ: &str, time: i64) -> logStreamResult {
    logStreamResult {
        addr: addr.into(),
        typ: typ.into(),
        messages: vec![logMessage {
            time,
            level: "info".into(),
            message: addr.into(),
        }],
        stream: Box::new(EmptyStream),
    }
}

/// 验证 Go `logResponseHeap.Less` 的完整排序契约：先按 time，同时按 typ。
#[test]
fn cluster_log_heap_orders_streams_by_timestamp() {
    let mut heap = logResponseHeap(Vec::new());
    assert_eq!(heap.Len(), 0);
    assert!(heap.Pop().is_none());

    heap.Push(stream_result("late", "tidb", 20));
    heap.Push(stream_result("same-time-tikv", "tikv", 10));
    heap.Push(stream_result("same-time-tidb", "tidb", 10));

    assert_eq!(heap.Len(), 3);
    assert_eq!(heap.Pop().unwrap().addr, "same-time-tidb");
    assert_eq!(heap.Pop().unwrap().addr, "same-time-tikv");
    assert_eq!(heap.Pop().unwrap().addr, "late");
    assert_eq!(heap.Len(), 0);
}
