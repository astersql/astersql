// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// 事务内轻量数据结构（TxStructure）的核心类型与工厂。
//
// `TxStructure` 在事务（transaction）或快照读路径上封装 KV Retriever/Mutator，
// 并通过 `prefix` 隔离不同命名空间；写操作在无 Mutator（只读快照）时返回
// `ErrWriteOnSnapshot`。

use std::sync::LazyLock;

/// Hash 键类型标志非法时的错误。
pub static ErrInvalidHashKeyFlag: LazyLock<Box<errors::Error>> =
    LazyLock::new(|| dbterror::ClassStructure.NewStd(mysql::ErrInvalidHashKeyFlag));
/// List 下标非法时的错误。
pub static ErrInvalidListIndex: LazyLock<Box<errors::Error>> =
    LazyLock::new(|| dbterror::ClassStructure.NewStd(mysql::ErrInvalidListIndex));
/// List 元数据非法时的错误。
pub static ErrInvalidListMetaData: LazyLock<Box<errors::Error>> =
    LazyLock::new(|| dbterror::ClassStructure.NewStd(mysql::ErrInvalidListMetaData));
/// 在只读快照上尝试写入时的错误。
pub static ErrWriteOnSnapshot: LazyLock<Box<errors::Error>> =
    LazyLock::new(|| dbterror::ClassStructure.NewStd(mysql::ErrWriteOnSnapshot));

/// 用 Retriever、可选 RetrieverMutator 与键前缀构造 `TxStructure`。
// NewStructure creates a TxStructure with Retriever, RetrieverMutator and key prefix.
pub fn NewStructure(
    reader: Box<dyn kv::Retriever>,
    readWriter: Option<Box<dyn kv::RetrieverMutator>>,
    prefix: Vec<u8>,
) -> TxStructure {
    TxStructure {
        reader,
        readWriter,
        prefix,
    }
}

/// 事务内可用的简单数据结构门面（string/hash/list 等）。
// TxStructure supports some simple data structures like string, hash, list, etc... and
// you can use these in a transaction.
pub struct TxStructure {
    /// 只读 KV 检索器。
    pub(crate) reader: Box<dyn kv::Retriever>,
    /// 可读写 Mutator；为 `None` 表示快照只读。
    pub(crate) readWriter: Option<Box<dyn kv::RetrieverMutator>>,
    /// 编码键时附加的命名空间前缀。
    pub(crate) prefix: Vec<u8>,
}

impl TxStructure {
    /// 取得可写 Mutator；快照路径无写能力时返回 `ErrWriteOnSnapshot`。
    pub(crate) fn writer(
        &mut self,
    ) -> Result<&mut (dyn kv::RetrieverMutator + '_), errors::SharedError> {
        match self.readWriter.as_deref_mut() {
            Some(writer) => Ok(writer),
            None => Err(ErrWriteOnSnapshot.FastGenByArgs(&[])),
        }
    }
}
