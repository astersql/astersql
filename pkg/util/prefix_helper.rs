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

// Copyright 2014 The ql Authors. All rights reserved.
// Use of this source code is governed by a BSD-style
// license that can be found in the LICENSES/QL-LICENSE file.

// Key 前缀扫描与批量删除辅助。
//
// 提供字节序 Key 的 `PrefixNext` / `HasPrefix`，以及基于 Retriever
// 的前缀区间扫描（ScanMetaWithPrefix）与按前缀删除（DelKeyWithPrefix）。
// RowKeyPrefixFilter 用于在迭代中跳过仍带指定行键前缀的条目。

#![allow(non_snake_case)]

use anyhow::Error;

/// 字节键包装类型，对应存储层/KV 中的 Key。
#[derive(Clone, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct Key(pub Vec<u8>);

impl Key {
    /// 计算严格大于本前缀、且仍覆盖「同前缀区间」的上界键（前缀递增）。
    /// 从末字节进位；全 0xff 溢出时在末尾追加 0，与 Go/ql 语义一致。
    pub fn PrefixNext(&self) -> Self {
        let mut next = self.0.clone();
        for index in (0..next.len()).rev() {
            next[index] = next[index].wrapping_add(1);
            if next[index] != 0 {
                return Self(next);
            }
        }
        next.clone_from(&self.0);
        next.push(0);
        Self(next)
    }

    /// 判断本键是否以 `prefix` 为字节前缀。
    pub fn HasPrefix(&self, prefix: &Key) -> bool {
        self.0.starts_with(&prefix.0)
    }
}

impl From<Vec<u8>> for Key {
    fn from(value: Vec<u8>) -> Self {
        Self(value)
    }
}

impl From<&[u8]> for Key {
    fn from(value: &[u8]) -> Self {
        Self(value.to_vec())
    }
}

/// KV 迭代器抽象：valid/key/value/next/close，对应 Go 侧类似接口。
pub trait KvIterator: Send {
    /// 当前位置是否仍有效。
    fn valid(&self) -> bool;
    /// 当前键。
    fn key(&self) -> &Key;
    /// 当前值。
    fn value(&self) -> &[u8];
    /// 前进到下一项。
    fn next(&mut self) -> Result<(), Error>;
    /// 关闭迭代器；默认空实现。
    fn close(&mut self) {}
}

/// 只读 KV 检索：按 `[start, end)` 打开迭代器。
pub trait Retriever {
    /// 在半开区间 `[start, end)` 上创建迭代器。
    fn iter(&self, start: &Key, end: &Key) -> Result<Box<dyn KvIterator>, Error>;
}

/// 可写检索：在 Retriever 基础上支持按键删除。
pub trait RetrieverMutator: Retriever {
    /// 删除指定键。
    fn delete(&mut self, key: Key) -> Result<(), Error>;
}

/// 键比较闭包类型，供过滤/seek 等场景复用。
pub type FnKeyCmp = Box<dyn Fn(&Key) -> bool + Send + Sync>;

/// 扫描带给定前缀的全部 KV，对每对调用 `filter`；返回 false 则提前停止。
pub fn ScanMetaWithPrefix(
    retriever: &dyn Retriever,
    prefix: Key,
    mut filter: impl FnMut(&Key, &[u8]) -> bool,
) -> Result<(), Error> {
    // 上界用 PrefixNext，保证只覆盖同前缀区间。
    let mut iter = retriever.iter(&prefix, &prefix.PrefixNext())?;
    let result = (|| {
        while iter.valid() && iter.key().HasPrefix(&prefix) {
            if !filter(iter.key(), iter.value()) {
                break;
            }
            iter.next()?;
        }
        Ok(())
    })();
    iter.close();
    result
}

/// 先收集前缀下全部键，再逐个 delete；任一次失败则中止并返回错误。
pub fn DelKeyWithPrefix(rm: &mut dyn RetrieverMutator, prefix: Key) -> Result<(), Error> {
    let mut iter = rm.iter(&prefix, &prefix.PrefixNext())?;
    let collected = (|| {
        let mut keys = Vec::new();
        while iter.valid() && iter.key().HasPrefix(&prefix) {
            keys.push(iter.key().clone());
            iter.next()?;
        }
        Ok::<_, Error>(keys)
    })();

    // 先关迭代器再删，避免边扫边改同一 store 的迭代器失效问题。
    let result = match collected {
        Ok(keys) => {
            let mut result = Ok(());
            for key in keys {
                if let Err(error) = rm.delete(key) {
                    result = Err(error);
                    break;
                }
            }
            result
        }
        Err(error) => Err(error),
    };
    iter.close();
    result
}

/// 构造「当前键是否仍带 rowKeyPrefix」的取反过滤器：有前缀返回 false。
pub fn RowKeyPrefixFilter(rowKeyPrefix: Key) -> FnKeyCmp {
    Box::new(move |currentKey: &Key| !currentKey.HasPrefix(&rowKeyPrefix))
}
