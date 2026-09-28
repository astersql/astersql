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

// TiKV 快照扫描器（scanner）适配层。
//
// 将已物化的键值行序列适配为 `KvIterator`，对应 client-go 侧的 snapshot scanner。
// 支持在指定游标位置注入确定性后端错误，便于驱动层单测模拟扫描失败。

use std::collections::HashMap;

use crate::{DriverError, Key, KvIterator};

/// Materialized adapter for the client-go snapshot scanner.
/// 物化键值行上的扫描器适配器：按 `position` 前进，空值行仍作为合法条目暴露给上层。
pub struct tikvScanner {
    rows: Vec<(Key, Vec<u8>)>,
    position: usize,
    closed: bool,
    next_errors: HashMap<usize, DriverError>,
}

impl tikvScanner {
    /// 用已排序（或调用方给定顺序）的键值行构造扫描器，游标置于首行之前等待首次 `Next`。
    pub fn new(rows: Vec<(Key, Vec<u8>)>) -> Self {
        Self {
            rows,
            position: 0,
            closed: false,
            next_errors: HashMap::new(),
        }
    }

    /// Installs a deterministic backend error for the next call at `position`.
    /// 在指定 `position` 上安装确定性后端错误，供下一次 `next` 返回。
    pub fn with_next_error(mut self, position: usize, error: DriverError) -> Self {
        self.next_errors.insert(position, error);
        self
    }

    pub fn Next(&mut self) -> Result<(), DriverError> {
        self.next()
    }

    pub fn Key(&self) -> Key {
        self.key().to_vec()
    }

    pub fn Value(&self) -> Vec<u8> {
        self.value().to_vec()
    }

    pub fn Valid(&self) -> bool {
        self.valid()
    }

    pub fn Close(&mut self) {
        self.close();
    }
}

impl KvIterator for tikvScanner {
    fn next(&mut self) -> Result<(), DriverError> {
        // 关闭或已越过末尾行后，与 Go iterator 一致返回 invalid。
        if self.closed || self.position >= self.rows.len() {
            return Err(DriverError::Backend("iterator is invalid".to_owned()));
        }
        // 优先消费该位置注入的错误，再前进游标。
        if let Some(error) = self.next_errors.remove(&self.position) {
            return Err(error);
        }
        self.position += 1;
        Ok(())
    }

    fn key(&self) -> &[u8] {
        self.rows
            .get(self.position)
            .map(|row| row.0.as_slice())
            .unwrap_or_default()
    }

    fn value(&self) -> &[u8] {
        self.rows
            .get(self.position)
            .map(|row| row.1.as_slice())
            .unwrap_or_default()
    }

    fn valid(&self) -> bool {
        !self.closed && self.position < self.rows.len()
    }

    fn close(&mut self) {
        self.closed = true;
    }
}
