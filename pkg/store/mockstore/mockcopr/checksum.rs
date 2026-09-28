// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// mock Coprocessor Checksum（校验和）请求处理。
//
// Checksum 用于校验 Region 数据完整性；本 mock 固定返回非零常量，
// 仅验证响应管道，不计算真实存储校验和。

use crate::copr_handler::{Request, Response, coprHandler};

impl coprHandler {
    /// The Go mock intentionally returns the same fixed non-zero checksum for
    /// every request; tests use it to exercise response plumbing, not storage.
    ///
    /// 故意对任意请求返回相同的固定非零校验和（三个 protobuf uint64 字段），用于测试响应链路。
    pub fn handleCopChecksumRequest(&self, _request: &Request) -> Response {
        // tipb.ChecksumResponse has three uint64 varint fields: checksum (1),
        // total_kvs (2), and total_bytes (3).  Because the Go mock fixes every
        // value to one, its protobuf representation is the six bytes below.
        // Encoding the wire form directly also mirrors that marshaling this
        // concrete response cannot fail.
        let data = vec![0x08, 0x01, 0x10, 0x01, 0x18, 0x01];
        Response {
            data,
            ..Response::default()
        }
    }
}
