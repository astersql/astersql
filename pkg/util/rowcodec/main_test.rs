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

// rowcodec 包级测试入口与旧行格式转码辅助。
//
//（将旧 datum 成对编码转成新 rowcodec 字节）。

#![allow(non_snake_case)]

use super::{encode_from_old_row, rowcodec, time};

// Rust's native test harness owns process setup and thread cleanup. Keep the

// EncodeFromOldRow encodes a row from an old-format row. It preserves the Go
// helper's fast path and column-pair decode order while using Encoder's public
// entry point instead of reaching through Rust privacy boundaries.
/// 将旧格式行字节经 `Encoder` 转成新 rowcodec 编码；已是新格式则原样返回。
pub fn EncodeFromOldRow(
    encoder: &mut rowcodec::Encoder,
    loc: Option<&time::Location>,
    old_row: &[u8],
    buf: Vec<u8>,
) -> Result<Vec<u8>, String> {
    encode_from_old_row(encoder, loc, old_row, buf)
}
