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

// 批量点查执行器（Batch Point Get）。
//
// 对应 Go 的 `BatchPointGetExec`：按主键句柄或唯一索引值批量读取行。
// 支持分区表、全局索引、悲观锁（pessimistic lock）、可重复读一致性
// 以及 keep-order / 降序输出。存储访问通过 `BatchPointGetRuntime` 注入。

#![allow(non_camel_case_types, non_snake_case)]

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use astersql_util_chunk::Chunk;

/// 点查键的字节编码。
pub type PointGetKey = Vec<u8>;
/// 点查返回的原始值字节。
pub type PointGetValue = Vec<u8>;

/// 可重复读路径的 failpoint 注入点（测试用，对应 Go failpoint 名称）。
pub(crate) fn batch_point_get_repeatable_read_failpoint() {
    let _ = fail::eval(
        "github.com/pingcap/tidb/pkg/executor/batchPointGetRepeatableReadTest-step1",
        |_| (),
    );
    let _ = fail::eval(
        "github.com/pingcap/tidb/pkg/executor/batchPointGetRepeatableReadTest-step2",
        |_| (),
    );
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 批量点查所需的表侧元信息摘要。
pub struct BatchPointGetTableInfo {
    pub id: i64,
    pub partitioned: bool,
    pub primary_key_is_handle: bool,
    pub primary_key_is_unsigned: bool,
    pub common_handle: bool,
    pub read_locked_table: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 唯一索引侧元信息；`global` 表示全局索引。
pub struct BatchPointGetIndexInfo {
    pub id: i64,
    pub global: bool,
    pub primary: bool,
}

/// 批量点查运行时边界：打开 getter、编码键、batch_get、解码行与加锁。
pub trait BatchPointGetRuntime {
    type Context;
    type Handle: Clone;
    type IndexValue: Clone;
    type FieldType: Clone;
    type ColumnInfo: Clone;
    type RowDecoder;
    type Error;

    /// 从列定义构建虚拟列索引与字段类型。
    fn build_virtual_column_info(
        &self,
        columns: &[Self::ColumnInfo],
    ) -> (Vec<usize>, Vec<Self::FieldType>);
    /// 打开批量 getter；`lock` 表示需要悲观加锁。
    fn open_batch_getter(
        &mut self,
        lock: bool,
        read_locked_table: bool,
        table_id: i64,
    ) -> Result<(), Self::Error>;
    /// 关闭并汇总运行时统计。
    fn close_runtime_stats(
        &mut self,
        table: &BatchPointGetTableInfo,
        index: Option<&BatchPointGetIndexInfo>,
    );
    /// 重置快照侧运行时统计。
    fn reset_snapshot_runtime_stats(&mut self);
    /// 最大执行时间（毫秒），传给底层 batch_get。
    fn maximum_execution_time_ms(&self) -> u64;
    /// 是否启用悲观读一致性（先读后锁已存在键）。
    fn pessimistic_read_consistency(&self) -> bool;
    /// 编码唯一索引键；不满足条件时返回 None。
    fn encode_unique_index_key(
        &self,
        table: &BatchPointGetTableInfo,
        index: &BatchPointGetIndexInfo,
        values: &[Self::IndexValue],
        physical_id: i64,
    ) -> Result<Option<PointGetKey>, Self::Error>;
    /// 批量按键读取 KV 值。
    fn batch_get(
        &mut self,
        ctx: &mut Self::Context,
        keys: &[PointGetKey],
        maximum_execution_time_ms: u64,
    ) -> Result<BTreeMap<PointGetKey, PointGetValue>, Self::Error>;
    /// 从索引值解码出行句柄。
    fn decode_index_handle(&self, value: &[u8]) -> Result<Self::Handle, Self::Error>;
    /// 从全局索引值解析物理分区 ID。
    fn global_index_partition_id(&self, value: &[u8]) -> Result<i64, Self::Error>;
    /// 从本地索引键解析表/分区 ID。
    fn table_id_from_index_key(&self, key: &[u8]) -> i64;
    /// 物理分区是否匹配语句指定的分区名过滤。
    fn partition_matches(&self, physical_id: i64, partition_names: &[String]) -> bool;
    /// 比较两个句柄；`unsigned` 控制无符号整型主键比较。
    fn compare_handles(
        &self,
        left: &Self::Handle,
        right: &Self::Handle,
        unsigned: bool,
    ) -> Ordering;
    /// 编码行记录键。
    fn encode_row_key(&self, physical_id: i64, handle: &Self::Handle) -> PointGetKey;
    /// 对键加悲观锁，`wait_time_ms` 为锁等待超时。
    fn lock_keys(
        &mut self,
        ctx: &mut Self::Context,
        wait_time_ms: i64,
        keys: &[PointGetKey],
    ) -> Result<(), Self::Error>;
    /// 是否弱一致性读（跳过索引-行不一致报告）。
    fn weak_consistency(&self) -> bool;
    /// 报告索引查到句柄但行不存在的不一致。
    fn report_lookup_inconsistent(
        &mut self,
        ctx: &mut Self::Context,
        table: &BatchPointGetTableInfo,
        index: &BatchPointGetIndexInfo,
        key: &[u8],
        index_key: &[u8],
        handle: &Self::Handle,
    ) -> Result<(), Self::Error>;
    /// 更新表级 delta（加锁写路径统计）。
    fn update_delta_for_table_id(&mut self, table_id: i64);
    /// 重置输出 Chunk。
    fn reset_output_chunk(&self, chunk: &mut Chunk);
    /// 输出 Chunk 是否已满。
    fn output_chunk_is_full(&self, chunk: &Chunk) -> bool;
    /// 解码一行到输出 Chunk。
    fn decode_row(
        &mut self,
        handle: &Self::Handle,
        value: &[u8],
        chunk: &mut Chunk,
        decoder: &mut Self::RowDecoder,
    ) -> Result<(), Self::Error>;
    /// 填充行校验和列。
    fn fill_row_checksum(
        &mut self,
        start: usize,
        end: usize,
        values: &[PointGetValue],
        handles: &[Self::Handle],
        chunk: &mut Chunk,
    ) -> Result<(), Self::Error>;
    /// 填充虚拟列。
    fn fill_virtual_columns(
        &mut self,
        field_types: &[Self::FieldType],
        indices: &[usize],
        columns: &[Self::ColumnInfo],
        chunk: &mut Chunk,
    ) -> Result<(), Self::Error>;
}

/// 批量点查执行器状态：句柄/索引值、锁选项、输出游标与行解码器。
pub struct BatchPointGetExec<R: BatchPointGetRuntime> {
    pub runtime: R,
    pub table_info: BatchPointGetTableInfo,
    pub index_info: Option<BatchPointGetIndexInfo>,
    pub handles: Vec<R::Handle>,
    pub plan_physical_ids: Vec<i64>,
    pub single_partition_id: i64,
    pub partition_names: Vec<String>,
    pub index_values: Vec<Vec<R::IndexValue>>,
    pub lock: bool,
    pub wait_time_ms: i64,
    pub initialized: bool,
    pub values: Vec<PointGetValue>,
    pub cursor: usize,
    pub row_decoder: R::RowDecoder,
    pub keep_order: bool,
    pub descending: bool,
    pub columns: Vec<R::ColumnInfo>,
    pub virtual_column_indices: Vec<usize>,
    pub virtual_column_field_types: Vec<R::FieldType>,
}

impl<R: BatchPointGetRuntime> BatchPointGetExec<R> {
    /// 根据列定义初始化虚拟列元信息。
    pub fn buildVirtualColumnInfo(&mut self) {
        let (indices, field_types) = self.runtime.build_virtual_column_info(&self.columns);
        self.virtual_column_indices = indices;
        self.virtual_column_field_types = field_types;
    }

    /// 打开执行器并初始化 batch getter。
    pub fn Open<C>(&mut self, _ctx: C) -> Result<(), R::Error> {
        self.runtime.open_batch_getter(
            self.lock,
            self.table_info.read_locked_table,
            self.table_info.id,
        )
    }

    /// 关闭执行器并重置初始化状态与游标。
    pub fn Close(&mut self) {
        self.runtime
            .close_runtime_stats(&self.table_info, self.index_info.as_ref());
        self.runtime.reset_snapshot_runtime_stats();
        self.initialized = false;
        self.cursor = 0;
    }

    /// 拉取下一批结果行到 Chunk；首次调用时完成 initialize。
    pub fn Next(&mut self, ctx: &mut R::Context, request: &mut Chunk) -> Result<(), R::Error> {
        self.runtime.reset_output_chunk(request);
        // 惰性初始化：完成键收集、batch_get 与可选加锁
        if !self.initialized {
            self.initialized = true;
            self.initialize(ctx)?;
            if self.lock {
                self.runtime.update_delta_for_table_id(self.table_info.id);
            }
        }
        if self.cursor >= self.values.len() {
            return Ok(());
        }

        // 按 Chunk 容量逐行解码，再填充 checksum 与虚拟列
        let start = self.cursor;
        while !self.runtime.output_chunk_is_full(request) && self.cursor < self.values.len() {
            self.runtime.decode_row(
                &self.handles[self.cursor],
                &self.values[self.cursor],
                request,
                &mut self.row_decoder,
            )?;
            self.cursor += 1;
        }
        self.runtime
            .fill_row_checksum(start, self.cursor, &self.values, &self.handles, request)?;
        self.runtime.fill_virtual_columns(
            &self.virtual_column_field_types,
            &self.virtual_column_indices,
            &self.columns,
            request,
        )
    }

    /// 初始化：经唯一索引或主键收集句柄，再批量取行并按需加锁。
    pub fn initialize(&mut self, ctx: &mut R::Context) -> Result<(), R::Error> {
        let maximum_execution_time = self.runtime.maximum_execution_time_ms();
        let read_consistency = self.runtime.pessimistic_read_consistency();
        let mut index_keys = Vec::new();

        // 唯一索引路径：编码索引键、batch_get 得到句柄，再处理分区过滤
        if let Some(index_info) = self
            .index_info
            .as_ref()
            .filter(|index| !(self.table_info.common_handle && index.primary))
        {
            let mut unique_keys = BTreeSet::new();
            let mut keys_to_fetch = Vec::new();
            for (index, values) in self.index_values.iter().enumerate() {
                let physical_id = if self.single_partition_id != 0 {
                    self.single_partition_id
                } else {
                    self.plan_physical_ids
                        .get(index)
                        .copied()
                        .unwrap_or(self.table_info.id)
                };
                if let Some(key) = self.runtime.encode_unique_index_key(
                    &self.table_info,
                    index_info,
                    values,
                    physical_id,
                )? {
                    if unique_keys.insert(key.clone()) {
                        keys_to_fetch.push(key);
                    }
                }
            }
            // 保持索引键字节序（可降序）
            if self.keep_order {
                keys_to_fetch.sort();
                if self.descending {
                    keys_to_fetch.reverse();
                }
            }
            // 非读一致性：索引键稍后与行键一并加锁
            if !read_consistency {
                index_keys.clone_from(&keys_to_fetch);
            }
            if keys_to_fetch.is_empty() {
                return Ok(());
            }

            let handle_values =
                self.runtime
                    .batch_get(ctx, &keys_to_fetch, maximum_execution_time)?;
            self.handles.clear();
            if self.table_info.partitioned {
                self.plan_physical_ids.clear();
            }
            for key in &keys_to_fetch {
                let Some(value) = handle_values.get(key) else {
                    continue;
                };
                let handle = self.runtime.decode_index_handle(value)?;
                if self.table_info.partitioned {
                    // 分区表：全局索引从 value 取分区 ID，本地索引从 key 解析
                    let physical_id = if index_info.global {
                        self.runtime.global_index_partition_id(value)?
                    } else {
                        self.runtime.table_id_from_index_key(key)
                    };
                    if index_info.global {
                        if self.single_partition_id != 0 && self.single_partition_id != physical_id
                        {
                            continue;
                        }
                        if !self
                            .runtime
                            .partition_matches(physical_id, &self.partition_names)
                        {
                            continue;
                        }
                    }
                    self.plan_physical_ids.push(physical_id);
                    if self.lock {
                        self.runtime.update_delta_for_table_id(physical_id);
                    }
                }
                self.handles.push(handle);
                if read_consistency {
                    index_keys.push(key.clone());
                }
            }
            batch_point_get_repeatable_read_failpoint();
        // 主键路径：按句柄排序以满足 keep-order
        } else if self.keep_order {
            let unsigned =
                self.table_info.primary_key_is_handle && self.table_info.primary_key_is_unsigned;
            self.handles.sort_by(|left, right| {
                let order = self.runtime.compare_handles(left, right, unsigned);
                if self.descending {
                    order.reverse()
                } else {
                    order
                }
            });
        }

        // 由句柄编码行键，过滤无效 physical_id
        let mut row_keys = Vec::with_capacity(self.handles.len());
        let mut retained_handles = Vec::with_capacity(self.handles.len());
        for (index, handle) in self.handles.iter().enumerate() {
            let physical_id = if self.single_partition_id != 0 {
                self.single_partition_id
            } else {
                self.plan_physical_ids
                    .get(index)
                    .copied()
                    .unwrap_or(self.table_info.id)
            };
            if physical_id > 0 {
                row_keys.push(self.runtime.encode_row_key(physical_id, handle));
                retained_handles.push(handle.clone());
            }
        }
        self.handles = retained_handles;

        // 非读一致性：先对行键+索引键加锁再读行
        if self.lock && !read_consistency {
            let mut all_keys = row_keys.clone();
            all_keys.extend(index_keys.iter().cloned());
            self.runtime.lock_keys(ctx, self.wait_time_ms, &all_keys)?;
        }
        let row_values = self
            .runtime
            .batch_get(ctx, &row_keys, maximum_execution_time)?;
        let mut existing_handles = Vec::with_capacity(row_values.len());
        let mut existing_keys = Vec::with_capacity(row_values.len() * 2);
        self.values.clear();
        for (index, key) in row_keys.iter().enumerate() {
            // 索引命中但行缺失：非弱一致性时报告不一致
            let Some(value) = row_values.get(key) else {
                if let Some(index_info) = self.index_info.as_ref() {
                    if !(self.table_info.common_handle && index_info.primary)
                        && !self.runtime.weak_consistency()
                    {
                        self.runtime.report_lookup_inconsistent(
                            ctx,
                            &self.table_info,
                            index_info,
                            key,
                            &index_keys[index],
                            &self.handles[index],
                        )?;
                    }
                }
                continue;
            };
            self.values.push(value.clone());
            existing_handles.push(self.handles[index].clone());
            if self.lock && read_consistency {
                existing_keys.push(key.clone());
                if !index_keys.is_empty() {
                    existing_keys.push(index_keys[index].clone());
                }
            }
        }
        // 读一致性：仅对实际存在的行键（及对应索引键）加锁
        if self.lock && read_consistency {
            self.runtime
                .lock_keys(ctx, self.wait_time_ms, &existing_keys)?;
        }
        self.handles = existing_handles;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 单点 Get 选项；`return_commit_ts` 要求返回提交时间戳（多数缓存路径不支持）。
pub struct PointGetOptions {
    pub return_commit_ts: bool,
}

/// 点查快照接口。
pub trait PointGetSnapshot {
    type Context;
    type Error;

    /// 按键读取；不存在返回 None。
    fn get(
        &self,
        ctx: &mut Self::Context,
        key: &[u8],
        options: PointGetOptions,
    ) -> Result<Option<PointGetValue>, Self::Error>;
    /// `return_commit_ts` 不被支持时的错误。
    fn unsupported_return_commit_ts(&self) -> Self::Error;
}

/// 缓存表快照包装：BatchGet/Get 过滤空值，并拒绝 return_commit_ts。
pub struct cacheTableSnapshot<S: PointGetSnapshot> {
    pub snapshot: S,
}

impl<S: PointGetSnapshot> cacheTableSnapshot<S> {
    /// 批量点查；跳过空值结果。
    pub fn BatchGet(
        &self,
        ctx: &mut S::Context,
        keys: &[PointGetKey],
        options: PointGetOptions,
    ) -> Result<BTreeMap<PointGetKey, PointGetValue>, S::Error> {
        if options.return_commit_ts {
            return Err(self.snapshot.unsupported_return_commit_ts());
        }
        let mut values = BTreeMap::new();
        for key in keys {
            if let Some(value) = self.snapshot.get(ctx, key, options)? {
                if !value.is_empty() {
                    values.insert(key.clone(), value);
                }
            }
        }
        Ok(values)
    }

    /// 单点查询。
    /// 从锁缓存读取；不支持 return_commit_ts。
    pub fn Get(
        &self,
        ctx: &mut S::Context,
        key: &[u8],
        options: PointGetOptions,
    ) -> Result<Option<PointGetValue>, S::Error> {
        if options.return_commit_ts {
            return Err(self.snapshot.unsupported_return_commit_ts());
        }
        self.snapshot.get(ctx, key, options)
    }
}

/// 测试辅助：由底层快照构造缓存表快照。
pub fn MockNewCacheTableSnapShot<S: PointGetSnapshot>(snapshot: S) -> cacheTableSnapshot<S> {
    cacheTableSnapshot { snapshot }
}

/// 悲观锁运行时：检查超时、加锁并可选缓存锁值。
pub trait PessimisticLockRuntime {
    type Context;
    type Error;

    fn check_max_execution_time(&self) -> Result<(), Self::Error>;
    fn lock_keys(
        &mut self,
        ctx: &mut Self::Context,
        wait_time_ms: i64,
        keys: &[PointGetKey],
    ) -> Result<BTreeMap<PointGetKey, PointGetValue>, Self::Error>;
    fn pessimistic_transaction(&self) -> bool;
    fn cache_locked_value(&mut self, key: PointGetKey, value: PointGetValue);
}

/// 对键加悲观锁；悲观事务下缓存返回的锁值。
pub fn LockKeys<R: PessimisticLockRuntime>(
    runtime: &mut R,
    ctx: &mut R::Context,
    wait_time_ms: i64,
    keys: &[PointGetKey],
) -> Result<(), R::Error> {
    runtime.check_max_execution_time()?;
    let returned_values = runtime.lock_keys(ctx, wait_time_ms, keys)?;
    if runtime.pessimistic_transaction() {
        for (key, value) in returned_values {
            runtime.cache_locked_value(key, value);
        }
    }
    Ok(())
}

/// 悲观锁值缓存的 BatchGetter 视图。
pub struct PessimisticLockCacheGetter {
    pub values: BTreeMap<PointGetKey, PointGetValue>,
}

impl PessimisticLockCacheGetter {
    pub fn Get(
        &self,
        key: &[u8],
        options: PointGetOptions,
    ) -> Result<Option<PointGetValue>, String> {
        if options.return_commit_ts {
            return Err("WithReturnCommitTS option is not supported for pessimistic lock cacheBatchGetter.Get".to_owned());
        }
        Ok(self.values.get(key).cloned())
    }
}
