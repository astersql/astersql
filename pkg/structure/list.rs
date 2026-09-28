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

// List（列表）语义：在 TxStructure 上实现类 Redis List 的双端推入/弹出。
//
// 元数据保存半开区间 `[LIndex, RIndex)`，元素按整数下标编码为独立 KV。
// 本文件经 `include!` 编入 list_impl 模块。

// valid index: [LIndex, RIndex)
/// 列表元数据：有效下标为半开区间 `[LIndex, RIndex)`。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct listMeta {
    /// 左端（表头）下标。
    LIndex: i64,
    /// 右端（表尾）开区间上界。
    RIndex: i64,
}

impl listMeta {
    /// 序列化为 16 字节大端：LIndex || RIndex。
    fn Value(self) -> Vec<u8> {
        let mut value = Vec::with_capacity(16);
        value.extend_from_slice(&(self.LIndex as u64).to_be_bytes());
        value.extend_from_slice(&(self.RIndex as u64).to_be_bytes());
        value
    }

    /// 列表是否为空（LIndex >= RIndex）。
    fn IsEmpty(self) -> bool {
        self.LIndex >= self.RIndex
    }
}

impl TxStructure {
    // LPush prepends one or multiple values to a list.
    /// 从左侧（表头）推入一个或多个元素。
    pub fn LPush(&mut self, key: &[u8], values: &[Vec<u8>]) -> Result<(), errors::SharedError> {
        self.listPush(key, true, values)
    }

    // RPush appends one or multiple values to a list.
    /// 从右侧（表尾）追加一个或多个元素。
    pub fn RPush(&mut self, key: &[u8], values: &[Vec<u8>]) -> Result<(), errors::SharedError> {
        self.listPush(key, false, values)
    }

    /// 统一推入逻辑：`left` 为真则递减 LIndex，否则递增 RIndex。
    fn listPush(
        &mut self,
        key: &[u8],
        left: bool,
        values: &[Vec<u8>],
    ) -> Result<(), errors::SharedError> {
        if self.readWriter.is_none() {
            return Err(ErrWriteOnSnapshot.FastGenByArgs(&[]));
        }
        if values.is_empty() {
            return Ok(());
        }

        let metaKey = self.encodeListMetaKey(key);
        let mut meta = self.loadListMeta(metaKey.clone())?;
        for value in values {
            let index = if left {
                meta.LIndex = meta.LIndex.wrapping_sub(1);
                meta.LIndex
            } else {
                let index = meta.RIndex;
                meta.RIndex = meta.RIndex.wrapping_add(1);
                index
            };
            let dataKey = self.encodeListDataKey(key, index);
            self.writer()?.Set(dataKey, value.clone())?;
        }
        self.writer()?.Set(metaKey, meta.Value())
    }

    // LPop removes and gets the first element in a list.
    /// 弹出并返回表头元素；空表返回 None。
    pub fn LPop(&mut self, key: &[u8]) -> Result<Option<Vec<u8>>, errors::SharedError> {
        self.listPop(key, true)
    }

    // RPop removes and gets the last element in a list.
    /// 弹出并返回表尾元素；空表返回 None。
    pub fn RPop(&mut self, key: &[u8]) -> Result<Option<Vec<u8>>, errors::SharedError> {
        self.listPop(key, false)
    }

    /// 统一弹出逻辑：更新元数据，删数据键；空后删除 meta。
    fn listPop(&mut self, key: &[u8], left: bool) -> Result<Option<Vec<u8>>, errors::SharedError> {
        if self.readWriter.is_none() {
            return Err(ErrWriteOnSnapshot.FastGenByArgs(&[]));
        }
        let metaKey = self.encodeListMetaKey(key);
        let mut meta = self.loadListMeta(metaKey.clone())?;
        if meta.IsEmpty() {
            return Ok(None);
        }

        let index = if left {
            let index = meta.LIndex;
            meta.LIndex = meta.LIndex.wrapping_add(1);
            index
        } else {
            meta.RIndex = meta.RIndex.wrapping_sub(1);
            meta.RIndex
        };
        let dataKey = self.encodeListDataKey(key, index);
        let data = kv::GetValue(&kv::Context::todo(), self.reader.as_ref(), dataKey.clone())?;
        self.writer()?.Delete(dataKey)?;
        if meta.IsEmpty() {
            self.writer()?.Delete(metaKey)?;
        } else {
            self.writer()?.Set(metaKey, meta.Value())?;
        }
        Ok(Some(data))
    }

    // LLen gets the length of a list.
    /// 返回列表长度 `RIndex - LIndex`。
    pub fn LLen(&self, key: &[u8]) -> Result<i64, errors::SharedError> {
        let meta = self.loadListMeta(self.encodeListMetaKey(key))?;
        Ok(meta.RIndex.wrapping_sub(meta.LIndex))
    }

    // LGetAll gets all elements of this list in order from right to left.
    /// 从右到左取出全部元素；空表返回 None。
    pub fn LGetAll(&self, key: &[u8]) -> Result<Option<Vec<Vec<u8>>>, errors::SharedError> {
        let meta = self.loadListMeta(self.encodeListMetaKey(key))?;
        if meta.IsEmpty() {
            return Ok(None);
        }

        let mut elements = Vec::with_capacity(meta.RIndex.wrapping_sub(meta.LIndex) as usize);
        let mut index = meta.RIndex.wrapping_sub(1);
        loop {
            elements.push(kv::GetValue(
                &kv::Context::todo(),
                self.reader.as_ref(),
                self.encodeListDataKey(key, index),
            )?);
            if index == meta.LIndex {
                break;
            }
            index = index.wrapping_sub(1);
        }
        Ok(Some(elements))
    }

    // LIndex gets an element from a list by its index.
    /// 按下标取元素；支持负下标（相对右端）。越界返回 None。
    pub fn LIndex(&self, key: &[u8], index: i64) -> Result<Option<Vec<u8>>, errors::SharedError> {
        let meta = self.loadListMeta(self.encodeListMetaKey(key))?;
        if meta.IsEmpty() {
            return Ok(None);
        }
        let index = adjustIndex(index, meta.LIndex, meta.RIndex);
        if index < meta.LIndex || index >= meta.RIndex {
            return Ok(None);
        }
        kv::GetValue(
            &kv::Context::todo(),
            self.reader.as_ref(),
            self.encodeListDataKey(key, index),
        )
        .map(Some)
    }

    // LSet updates an element in the list by its index.
    /// 按下标更新元素；空表静默成功，越界返回无效下标错误。
    pub fn LSet(
        &mut self,
        key: &[u8],
        index: i64,
        value: &[u8],
    ) -> Result<(), errors::SharedError> {
        if self.readWriter.is_none() {
            return Err(ErrWriteOnSnapshot.FastGenByArgs(&[]));
        }
        let meta = self.loadListMeta(self.encodeListMetaKey(key))?;
        if meta.IsEmpty() {
            return Ok(());
        }
        let index = adjustIndex(index, meta.LIndex, meta.RIndex);
        if index >= meta.LIndex && index < meta.RIndex {
            let dataKey = self.encodeListDataKey(key, index);
            return self.writer()?.Set(dataKey, value.to_vec());
        }
        Err(ErrInvalidListIndex.GenWithStack(&format!("invalid list index {index}"), &[]))
    }

    // LClear removes the list of the key.
    /// 删除列表全部元素及元数据。
    pub fn LClear(&mut self, key: &[u8]) -> Result<(), errors::SharedError> {
        if self.readWriter.is_none() {
            return Err(ErrWriteOnSnapshot.FastGenByArgs(&[]));
        }
        let metaKey = self.encodeListMetaKey(key);
        let meta = self.loadListMeta(metaKey.clone())?;
        if meta.IsEmpty() {
            return Ok(());
        }
        let mut index = meta.LIndex;
        while index < meta.RIndex {
            let dataKey = self.encodeListDataKey(key, index);
            self.writer()?.Delete(dataKey)?;
            index = index.wrapping_add(1);
        }
        self.writer()?.Delete(metaKey)
    }

    /// 加载列表元数据；不存在返回默认空 meta；长度非法则报错。
    fn loadListMeta(&self, metaKey: kv::Key) -> Result<listMeta, errors::SharedError> {
        let value = match kv::GetValue(&kv::Context::todo(), self.reader.as_ref(), metaKey) {
            Ok(value) => value,
            Err(error) if kv::IsErrNotFound(&error) => return Ok(listMeta::default()),
            Err(error) => return Err(error),
        };
        if value.len() != 16 {
            return Err(ErrInvalidListMetaData.FastGenByArgs(&[]));
        }
        Ok(listMeta {
            LIndex: u64::from_be_bytes(value[..8].try_into().unwrap()) as i64,
            RIndex: u64::from_be_bytes(value[8..].try_into().unwrap()) as i64,
        })
    }
}

/// 将逻辑下标转为物理下标：非负相对 LIndex，负相对 RIndex。
pub fn adjustIndex(index: i64, minv: i64, maxv: i64) -> i64 {
    if index >= 0 {
        index.wrapping_add(minv)
    } else {
        index.wrapping_add(maxv)
    }
}
