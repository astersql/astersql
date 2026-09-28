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

// 索引 KV 数量检查测试工具。
//
// 通过可注入的事务/快照迭代运行时，扫描某二级索引（secondary index）前缀下的
// KV 对数量并与期望值比对，用于 ADMIN CHECK INDEX 类回归。

use std::fmt;

/// 索引检查失败错误，消息为人可读描述。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexCheckError(pub String);
impl fmt::Display for IndexCheckError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
impl std::error::Error for IndexCheckError {}

/// 索引元数据：表 ID、索引 ID 与扫描起始最小键。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexMeta {
    pub table_id: i64,
    pub index_id: i64,
    pub minimum_key: Vec<u8>,
}

/// 快照（snapshot）键值迭代器：支持有效性判断、取键、前进与关闭。
pub trait SnapshotIterator {
    fn valid(&self) -> bool;
    fn key(&self) -> &[u8];
    fn next(&mut self) -> Result<(), IndexCheckError>;
    fn close(&mut self) -> Result<(), IndexCheckError>;
}

/// 索引检查所需运行时：解析索引元数据、开启事务、构造快照迭代器并解码索引 ID。
pub trait IndexCheckRuntime {
    fn index_meta(
        &mut self,
        database: &str,
        table: &str,
        index: &str,
    ) -> Result<IndexMeta, IndexCheckError>;
    fn begin(&mut self) -> Result<(), IndexCheckError>;
    fn commit(&mut self) -> Result<(), IndexCheckError>;
    fn snapshot_iter(&mut self, start: &[u8])
    -> Result<Box<dyn SnapshotIterator>, IndexCheckError>;
    fn decode_index_id(&self, key: &[u8]) -> Result<Option<i64>, IndexCheckError>;
}

/// 统计 `test` 库下指定表索引的 KV 数量，并断言等于 `expected`。
///
/// 扫描从 `minimum_key` 起连续属于该 `index_id` 的键；遇到其它索引或无效键则停止。
/// 无论扫描成败都会尝试 commit，最终错误合并扫描与提交结果。
pub fn CheckIndexKVCount(
    runtime: &mut dyn IndexCheckRuntime,
    table_name: &str,
    index_name: &str,
    expected: usize,
) -> Result<(), IndexCheckError> {
    let meta = runtime.index_meta("test", table_name, index_name)?;
    runtime.begin()?;
    let scan_result = (|| {
        let mut iterator = runtime.snapshot_iter(&meta.minimum_key)?;
        let mut count = 0;
        let iteration_result = (|| {
            // 仅累计当前索引 ID 的键；一旦 decode 不到匹配 ID 即结束前缀扫描。
            while iterator.valid() {
                match runtime.decode_index_id(iterator.key())? {
                    Some(index_id) if index_id == meta.index_id => count += 1,
                    _ => break,
                }
                iterator.next()?;
            }
            Ok(())
        })();
        // Go 使用 `defer iter.Close()`，其返回值不会参与测试结果；保持该语义，
        // 同时无论循环因何退出都关闭迭代器。
        let _ = iterator.close();
        iteration_result?;
        if count != expected {
            return Err(IndexCheckError(format!(
                "index {table_name}.{index_name} contains {count} KV pairs, expected {expected}"
            )));
        }
        Ok(())
    })();
    let commit_result = runtime.commit();
    scan_result.and(commit_result)
}
