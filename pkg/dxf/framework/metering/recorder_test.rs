// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// Metering Recorder 单元测试。
//
// 验证 `Recorder` 对对象存储（object store）请求次数/读写字节，
// 以及集群（cluster）读写流量的累加是否正确汇总到 `Data` 快照。

use crate::data::{Data, DataValues};
use crate::recorder::Recorder;

/// 依次记录 get/put 请求、对象存储读写字节与集群读写字节，
/// 断言 `curr_data()` 返回的计量快照与输入一致。
#[test]
fn test_recorder() {
    // 构造绑定 task_id / keyspace / task_type 的计量记录器。
    let recorder = Recorder::new(1, "ks", "tt");
    // 对象存储访问：GET/PUT 请求次数与读写字节。
    recorder.record_obj_store_get(100);
    recorder.record_obj_store_put(200);
    recorder.record_obj_store_read(11);
    recorder.record_obj_store_write(22);
    // 集群侧流量：读/写字节（与对象存储计量分开累计）。
    recorder.IncClusterReadBytes(300);
    recorder.IncClusterWriteBytes(400);

    // 汇总快照应包含全部六类计量字段。
    assert_eq!(
        recorder.curr_data(),
        Data::new(
            1,
            "ks",
            "tt",
            DataValues {
                get_requests: 100,
                put_requests: 200,
                obj_store_read_bytes: 11,
                obj_store_write_bytes: 22,
                cluster_read_bytes: 300,
                cluster_write_bytes: 400,
            },
        )
    );
}
