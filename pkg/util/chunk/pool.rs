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

// Chunk 列对象池：按定长宽度分桶复用 `Column`，降低执行器频繁分配开销。
//
// 对应 Go `pkg/util/chunk/pool.go`。列宽映射到五个池（变长 + 4/8/16/40
// 字节定长），与 Go 五个 `sync.Pool` 对齐；另提供按 `initCap` 索引的全局池入口。

use crate::{Chunk, Column, VarElemLen, getFixedLen, newFixedLenColumn, newVarLenColumn, types};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// 全局池表：按初始行容量 `initCap` 缓存 `Pool` 实例。
static globalChunkPool: OnceLock<Mutex<HashMap<usize, Arc<Pool>>>> = OnceLock::new();

/// 按 `initCap` 取得（或懒创建）全局共享的列对象池。
fn global_pool(initCap: usize) -> Arc<Pool> {
    let pools = globalChunkPool.get_or_init(|| Mutex::new(HashMap::new()));
    let mut pools = pools
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    pools
        .entry(initCap)
        .or_insert_with(|| Arc::new(*NewPool(initCap)))
        .clone()
}

#[cfg(test)]
pub(crate) fn global_cached_columns_for_test(initCap: usize) -> usize {
    global_pool(initCap).cached_columns()
}

/// 从全局池取出匹配字段布局的空 Chunk。
pub fn getChunkFromPool(initCap: usize, fields: &[types::FieldType]) -> Box<Chunk> {
    global_pool(initCap).GetChunk(fields)
}

/// 将 Chunk 的列 reset 后归还全局池。
pub fn putChunkFromPool(initCap: usize, fields: &[types::FieldType], chunk: &mut Chunk) {
    global_pool(initCap).PutChunk(fields, chunk);
}

/// Pools columns by fixed-width layout, matching the five Go `sync.Pool`s.
///
/// 按列宽分桶的 Column 对象池；五个内部 `Mutex<Vec<Column>>` 对应 Go 的五个 sync.Pool。
pub struct Pool {
    /// 新建列时的初始行容量。
    initCap: usize,
    /// 变长列（VARCHAR/BLOB 等，`VarElemLen`）缓存。
    varLenColPool: Mutex<Vec<Column>>,
    /// 4 字节定长列缓存。
    fixLenColPool4: Mutex<Vec<Column>>,
    /// 8 字节定长列缓存。
    fixLenColPool8: Mutex<Vec<Column>>,
    /// 16 字节定长列缓存。
    fixLenColPool16: Mutex<Vec<Column>>,
    /// 40 字节定长列缓存（如 Decimal 等）。
    fixLenColPool40: Mutex<Vec<Column>>,
}

/// 创建指定初始容量的列对象池。
pub fn NewPool(initCap: usize) -> Box<Pool> {
    Box::new(Pool {
        initCap,
        varLenColPool: Mutex::new(Vec::new()),
        fixLenColPool4: Mutex::new(Vec::new()),
        fixLenColPool8: Mutex::new(Vec::new()),
        fixLenColPool16: Mutex::new(Vec::new()),
        fixLenColPool40: Mutex::new(Vec::new()),
    })
}

impl Pool {
    /// 按列宽选择对应缓存桶；不支持的宽度直接 panic。
    fn cache(&self, width: usize) -> &Mutex<Vec<Column>> {
        match width {
            VarElemLen => &self.varLenColPool,
            4 => &self.fixLenColPool4,
            8 => &self.fixLenColPool8,
            16 => &self.fixLenColPool16,
            40 => &self.fixLenColPool40,
            _ => panic!("unsupported chunk column width {width}"),
        }
    }

    /// 从对应桶弹出一列；桶空时按宽度新建定长或变长列。
    fn get_column(&self, width: usize) -> Column {
        let mut cache = self
            .cache(width)
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        cache.pop().unwrap_or_else(|| {
            if width == VarElemLen {
                *newVarLenColumn(self.initCap)
            } else {
                *newFixedLenColumn(width, self.initCap)
            }
        })
    }

    /// 按字段类型列表组装 Chunk，每列从池中取回。
    pub fn GetChunk(&self, fields: &[types::FieldType]) -> Box<Chunk> {
        Box::new(Chunk {
            sel: None,
            capacity: self.initCap,
            requiredRows: self.initCap,
            numVirtualRows: 0,
            inCompleteChunk: false,
            columns: fields
                .iter()
                .map(|field| self.get_column(getFixedLen(field)))
                .collect(),
        })
    }

    /// 清空 Chunk 列向量：逐列 reset 后按字段宽度归还对应桶。
    pub fn PutChunk(&self, fields: &[types::FieldType], chunk: &mut Chunk) {
        assert_eq!(
            fields.len(),
            chunk.columns.len(),
            "field and column counts must match"
        );
        for (field, mut column) in fields.iter().zip(chunk.columns.drain(..)) {
            column.reset();
            self.cache(getFixedLen(field))
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(column);
        }
    }

    /// 统计五个桶中缓存列的总数（测试/诊断用）。
    pub fn cached_columns(&self) -> usize {
        [
            &self.varLenColPool,
            &self.fixLenColPool4,
            &self.fixLenColPool8,
            &self.fixLenColPool16,
            &self.fixLenColPool40,
        ]
        .into_iter()
        .map(|pool| {
            pool.lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .len()
        })
        .sum()
    }
}
