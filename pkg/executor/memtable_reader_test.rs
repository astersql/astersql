// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// memtable_reader 中 failpoint 服务地址解析的单元测试。
//
// Failpoint（故障注入点）用于在测试中模拟节点故障；服务信息字符串
// 描述 TiDB/TiKV 等组件的类型与监听地址，供内存表读取器定位探测目标。

use crate::memtable_reader::parseFailpointServerInfo;

/// 验证多段地址均被保留，且字段不足的行被拒绝。
#[test]
fn memtable_failpoint_server_parser_preserves_all_addresses_and_rejects_short_rows() {
    // 分号分隔多台服务器；逗号分隔类型与地址字段。
    let servers =
        parseFailpointServerInfo("tidb,db:4000,status:10080;tikv,kv:20160,kv:20180").unwrap();
    assert_eq!(servers.len(), 2);
    assert_eq!(servers[0].serverType, "tidb");
    assert_eq!(servers[1].statusAddr, "kv:20180");
    // 缺少必需字段时应返回错误。
    assert!(parseFailpointServerInfo("tidb,missing").is_err());
}
