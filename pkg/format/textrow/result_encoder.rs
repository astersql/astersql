// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// Copyright 2013 The Go-MySQL-Driver Authors. All rights reserved.
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this file,
// You can obtain one at http://mozilla.org/MPL/2.0/.

// The MIT License (MIT)
//
// Copyright (c) 2014 wandoulabs
// Copyright (c) 2014 siddontang
//
// Permission is hereby granted, free of charge, to any person obtaining a copy of
// this software and associated documentation files (the "Software"), to deal in
// the Software without restriction, including without limitation the rights to
// use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of
// the Software, and to permit persons to whom the Software is furnished to do so,
// subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.

// 文本协议结果编码器：按会话结果字符集转换列名与行数据字节。
//
// MySQL 会话变量 `@@character_set_results` 决定客户端看到的字符集。
// 编码时：元数据（列名等）始终用结果字符集；行数据在非 binary 列上优先用结果字符集，
// 若列 collation 为 binary 或结果字符集本身为 binary/空，则改用列侧编码。

use crate::{charset, logutil, mysql};

/// Encodes text-protocol metadata and row data using the session result
/// charset, except where MySQL requires the column charset to win.
/// 按会话结果字符集编码文本协议元数据与行数据；列字符集在 MySQL 要求时优先。
pub struct ResultEncoder {
    /// 会话结果字符集对应的编码器。
    encoding: charset::EncodingRef,
    /// 当前列 collation 对应的编码器（可随列切换）。
    data_encoding: charset::EncodingRef,
    /// 可复用的转换输出缓冲；`Clean` 后为 None。
    buffer: Option<Vec<u8>>,
    /// 数值格式化等路径复用的 scratch 缓冲。
    pub(crate) scratch: Option<Vec<u8>>,
    /// 结果字符集名称（如 utf8mb4、gbk、binary）。
    chs_name: String,
    /// 结果字符集是否为 binary。
    is_binary: bool,
    /// 结果字符集名为空（等价于不改写元数据 charset）。
    is_null: bool,
    /// 当前列是否为 binary collation。
    data_is_binary: bool,
}

impl ResultEncoder {
    /// Creates an encoder for `@@character_set_results`.
    /// 按 `@@character_set_results` 构造编码器。
    pub fn NewResultEncoder(chs: &str) -> Self {
        Self {
            chs_name: chs.to_owned(),
            encoding: charset::FindEncodingTakeUTF8AsNoop(chs),
            data_encoding: charset::FindEncodingTakeUTF8AsNoop(""),
            buffer: Some(Vec::new()),
            scratch: Some(Vec::with_capacity(48)),
            is_binary: chs == charset::CharsetBin,
            is_null: chs.is_empty(),
            data_is_binary: false,
        }
    }

    /// Releases retained allocations at the end of a statement.
    /// 语句结束时释放持有的缓冲分配。
    pub fn Clean(&mut self) {
        self.buffer = None;
        self.scratch = None;
    }

    /// Updates the encoding associated with the current column collation.
    /// 按当前列 collation ID 更新数据侧编码器；未知 ID 仅打 warn 日志。
    pub fn UpdateDataEncoding(&mut self, chs_id: u16) {
        let (chs, _, error) = charset::GetCharsetInfoByID(i32::from(chs_id));
        if let Some(error) = error {
            logutil::BgLogger().warn(format!("unknown charset ID {chs_id}: {error}"));
        }
        self.data_encoding = charset::FindEncodingTakeUTF8AsNoop(&chs);
        self.data_is_binary = chs_id == u16::from(mysql::BinaryDefaultCollationID);
    }

    /// Returns the charset advertised in text-protocol column metadata.
    /// 返回文本协议列元数据中应对外声明的 charset ID。
    /// 空结果字符集或非字符串列沿用 dump_charset；binary 列保持 binary。
    pub fn ColumnCharsetID(&self, dump_charset: u16, is_string_col: bool) -> u16 {
        if self.is_null || self.chs_name.is_empty() || !is_string_col {
            return dump_charset;
        }
        let binary_id = u16::from(mysql::BinaryDefaultCollationID);
        if dump_charset == binary_id {
            return binary_id;
        }
        u16::from(mysql::CharsetNameToID(&self.chs_name))
    }

    /// Encodes metadata such as column names with the result charset.
    /// 用结果字符集编码元数据（如列名）。
    pub fn EncodeMeta(&mut self, src: &[u8]) -> Vec<u8> {
        self.encode_with(src, self.encoding)
    }

    /// Encodes row data with either the result or column charset.
    /// 编码行数据：结果字符集为空/binary 或列为 binary 时用列侧编码，否则用结果字符集。
    pub fn EncodeData(&mut self, src: &[u8]) -> Vec<u8> {
        let enc = if self.is_null || self.is_binary || self.data_is_binary {
            self.data_encoding
        } else {
            self.encoding
        };
        self.encode_with(src, enc)
    }

    /// 取出可复用的 scratch；若已 Clean 则返回新建空 Vec 且 reusable=false。
    pub(crate) fn take_scratch(&mut self) -> (Vec<u8>, bool) {
        match self.scratch.take() {
            Some(mut scratch) => {
                scratch.clear();
                (scratch, true)
            }
            None => (Vec::new(), false),
        }
    }

    /// 将本次输出写回 scratch 以便下次复用（仅当 reusable 为真）。
    pub(crate) fn recycle_scratch(&mut self, output: &[u8], reusable: bool) {
        if reusable {
            let mut scratch = Vec::with_capacity(output.len().max(48));
            scratch.extend_from_slice(output);
            self.scratch = Some(scratch);
        }
    }

    /// 用指定编码器转换字节；失败时记录 debug 并返回错误中的部分输出。
    fn encode_with(&mut self, src: &[u8], enc: charset::EncodingRef) -> Vec<u8> {
        let mut temporary = Vec::new();
        // Clean 后 buffer 为空，退回临时 Vec，避免 panic。
        let buffer = self.buffer.as_mut().unwrap_or(&mut temporary);
        match enc.Transform(buffer, src, charset::OpEncodeReplace) {
            Ok(data) => data,
            Err(error) => {
                logutil::BgLogger().debug(format!("encode error: {error}"));
                error.output().to_vec()
            }
        }
    }
}

/// Go-compatible package constructor.
/// 与 Go 包级构造函数对应的便捷入口。
pub fn NewResultEncoder(chs: &str) -> ResultEncoder {
    ResultEncoder::NewResultEncoder(chs)
}

/// Reports whether metadata charset rewriting applies to this column type.
/// 判断该列类型是否参与元数据字符集改写（字符串/Blob/Enum/Set/JSON/向量等）。
pub fn IsStringColumnType(tp: u8) -> bool {
    match tp {
        mysql::TypeString
        | mysql::TypeVarString
        | mysql::TypeVarchar
        | mysql::TypeBit
        | mysql::TypeTinyBlob
        | mysql::TypeMediumBlob
        | mysql::TypeLongBlob
        | mysql::TypeBlob
        | mysql::TypeEnum
        | mysql::TypeSet
        | mysql::TypeJSON => true,
        mysql::TypeTiDBVectorFloat32 => true,
        _ => false,
    }
}
