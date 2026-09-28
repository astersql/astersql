// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// dumpformat 行数据定义：复用的 `Row` 与日志数组序列化接口。
//
// 由 `parsedef/def.go` 迁移，保留 Datum 列值与 zapcore 风格数组编码语义。

// 本文件由 pkg/dumpformat/parsedef/def.go 迁移而来，保留行数据和日志数组序列化语义。

#![allow(non_snake_case)]

use std::convert::Infallible;

pub use types_crate::datum::Datum;

/// Rust 日志后端实现此接口即可接收 Go zapcore.ArrayEncoder 的 AppendString 调用。
pub trait ArrayEncoder {
    /// 追加一个字符串到日志数组编码器。
    fn AppendString(&mut self, value: &str);
}

// Row is the content of a row.
/// 一行导出/导入数据：行号、Datum 列值与估算长度。
///
/// 对象常被复用，读下一行时递增 `RowID`。
#[derive(Clone, Default)]
pub struct Row {
    // RowID is the row id of the row.
    // as objects of this struct is reused, this RowID is increased when reading
    // next row.
    /// 行标识；复用本结构时在读下一行时递增。
    pub RowID: i64,
    /// 本行列值，元素为 TiDB Datum（统一的 SQL 值容器）。
    pub Row: Vec<Datum>,
    /// 行内容估算长度（字节级），供缓冲与限流参考。
    pub Length: isize,
}

/// `Row` 的日志序列化实现。
impl Row {
    // MarshalLogArray implements the zapcore.ArrayMarshaler interface
    /// 将各列 Datum 的字符串形式按序写入日志数组编码器。
    pub fn MarshalLogArray(&self, encoder: &mut dyn ArrayEncoder) -> Result<(), Infallible> {
        // 按列顺序 AppendString，便于日志中还原行内容。
        for r in &self.Row {
            encoder.AppendString(&r.String());
        }
        Ok(())
    }
}
