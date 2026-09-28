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

// 脏写迭代器与快照迭代器的合并扫描（Union Iterator）。
//
// 事务读需要同时看到 memBuffer 中的未提交写入与快照中的已提交数据。
// 空 dirty 值表示删除 tombstone，会屏蔽同键的快照值。

use std::cmp::Ordering;

use crate::{DriverError, Key, KvIterator};

/// Merge iterator over transaction-local writes and an immutable snapshot.
/// Empty dirty values are deletion tombstones and suppress snapshot values.
/// 合并迭代器：优先 dirty，同键时 dirty 覆盖 snapshot；空 dirty 为删除。
pub struct UnionIter {
    dirty_it: Option<Box<dyn KvIterator>>,
    snapshot_it: Option<Box<dyn KvIterator>>,
    dirty_valid: bool,
    snapshot_valid: bool,
    cur_is_dirty: bool,
    is_valid: bool,
    reverse: bool,
}

/// 构造合并迭代器并定位到第一个有效当前位置；失败时丢弃内部持有但不关闭调用方迭代器。
pub fn NewUnionIter(
    dirty_it: Box<dyn KvIterator>,
    snapshot_it: Box<dyn KvIterator>,
    reverse: bool,
) -> Result<UnionIter, DriverError> {
    let dirty_valid = dirty_it.valid();
    let snapshot_valid = snapshot_it.valid();
    let mut iterator = UnionIter {
        dirty_it: Some(dirty_it),
        snapshot_it: Some(snapshot_it),
        dirty_valid,
        snapshot_valid,
        cur_is_dirty: false,
        is_valid: false,
        reverse,
    };
    // Go returns (nil, err) without closing the caller-owned iterators.
    // 初始化定位失败时 take 掉迭代器字段，避免 Drop 时 Close 调用方仍持有的迭代器。
    if let Err(error) = iterator.update_cur() {
        let _ = iterator.dirty_it.take();
        let _ = iterator.snapshot_it.take();
        return Err(error);
    }
    Ok(iterator)
}

impl UnionIter {
    /// 推进 dirty 侧并刷新 dirty_valid。
    fn dirty_next(&mut self) -> Result<(), DriverError> {
        let iterator = self.dirty_it.as_mut().expect("dirty iterator is open");
        let result = iterator.next();
        self.dirty_valid = iterator.valid();
        result
    }

    /// 推进 snapshot 侧并刷新 snapshot_valid。
    fn snapshot_next(&mut self) -> Result<(), DriverError> {
        let iterator = self
            .snapshot_it
            .as_mut()
            .expect("snapshot iterator is open");
        let result = iterator.next();
        self.snapshot_valid = iterator.valid();
        result
    }

    /// 根据两侧有效性与键序选定当前条目；跳过空 dirty（删除）并处理同键覆盖。
    fn update_cur(&mut self) -> Result<(), DriverError> {
        self.is_valid = true;
        loop {
            if !self.dirty_valid && !self.snapshot_valid {
                self.is_valid = false;
                return Ok(());
            }
            if !self.dirty_valid {
                self.cur_is_dirty = false;
                return Ok(());
            }
            if !self.snapshot_valid {
                self.cur_is_dirty = true;
                // 仅剩 dirty 时，空值 tombstone 继续跳过。
                if self.dirty_value().is_empty() {
                    self.dirty_next()?;
                    continue;
                }
                return Ok(());
            }

            let mut ordering = self.dirty_key().cmp(self.snapshot_key());
            if self.reverse {
                ordering = ordering.reverse();
            }
            match ordering {
                Ordering::Equal => {
                    if self.dirty_value().is_empty() {
                        // 同键删除：两侧都前进，快照值被屏蔽。
                        self.dirty_next()?;
                        self.snapshot_next()?;
                        continue;
                    }
                    // The dirty value wins; advance the duplicate snapshot now.
                    // dirty 覆盖：推进 snapshot 去重后停留在 dirty。
                    self.snapshot_next()?;
                    self.cur_is_dirty = true;
                    return Ok(());
                }
                Ordering::Greater => {
                    // 正向时 snapshot 键更小，选 snapshot。
                    self.cur_is_dirty = false;
                    return Ok(());
                }
                Ordering::Less => {
                    if self.dirty_value().is_empty() {
                        self.dirty_next()?;
                        continue;
                    }
                    self.cur_is_dirty = true;
                    return Ok(());
                }
            }
        }
    }

    fn dirty_key(&self) -> &[u8] {
        self.dirty_it
            .as_ref()
            .expect("dirty iterator is open")
            .key()
    }

    fn dirty_value(&self) -> &[u8] {
        self.dirty_it
            .as_ref()
            .expect("dirty iterator is open")
            .value()
    }

    fn snapshot_key(&self) -> &[u8] {
        self.snapshot_it
            .as_ref()
            .expect("snapshot iterator is open")
            .key()
    }

    /// Go 风格 Next。
    pub fn Next(&mut self) -> Result<(), DriverError> {
        self.next()
    }

    /// Go 风格 Value（拷贝）。
    pub fn Value(&self) -> Vec<u8> {
        self.value().to_vec()
    }

    /// Go 风格 Key（拷贝）。
    pub fn Key(&self) -> Key {
        self.key().to_vec()
    }

    /// Go 风格 Valid。
    pub fn Valid(&self) -> bool {
        self.valid()
    }

    /// Go 风格 Close。
    pub fn Close(&mut self) {
        self.close();
    }
}

impl KvIterator for UnionIter {
    fn next(&mut self) -> Result<(), DriverError> {
        if !self.is_valid {
            return Err(DriverError::Backend("iterator is invalid".to_owned()));
        }
        // 只推进当前选中的那一侧，再重新定位。
        if self.cur_is_dirty {
            self.dirty_next()?;
        } else {
            self.snapshot_next()?;
        }
        self.update_cur()
    }

    fn key(&self) -> &[u8] {
        if !self.is_valid {
            return &[];
        }
        if self.cur_is_dirty {
            self.dirty_key()
        } else {
            self.snapshot_key()
        }
    }

    fn value(&self) -> &[u8] {
        if !self.is_valid {
            return &[];
        }
        if self.cur_is_dirty {
            self.dirty_value()
        } else {
            self.snapshot_it
                .as_ref()
                .expect("snapshot iterator is open")
                .value()
        }
    }

    fn valid(&self) -> bool {
        self.is_valid
    }

    fn close(&mut self) {
        if let Some(mut iterator) = self.snapshot_it.take() {
            iterator.close();
        }
        if let Some(mut iterator) = self.dirty_it.take() {
            iterator.close();
        }
        self.dirty_valid = false;
        self.snapshot_valid = false;
        self.is_valid = false;
    }
}

impl Drop for UnionIter {
    fn drop(&mut self) {
        self.close();
    }
}
