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

// Mock KV 迭代器（Iterator）：切片游标与可注入错误的包装器。
//
// 对应 Go `iter.go`，供 util 包单元测试确定性遍历键值对。

use crate::kv;

/// 将消息包装为 KV 层共享错误。
fn iterator_error(message: impl Into<String>) -> kv::errors::SharedError {
    kv::errors::SharedError::new(std::io::Error::other(message.into()))
}

/// 基于自有 KV Entry 切片副本的迭代器。
/// SliceIter iterates over an owned copy of a KV entry slice.
pub struct SliceIter {
    data: Vec<kv::Entry>,
    cur: isize,
}

/// 创建定位在首条记录的切片迭代器。
/// NewSliceIter creates an iterator positioned at the first entry.
pub fn NewSliceIter(data: Vec<kv::Entry>) -> Box<SliceIter> {
    Box::new(SliceIter { data, cur: 0 })
}

impl SliceIter {
    /// 返回原始条目切片，不改变顺序。
    /// GetSlice returns the original entries without changing their order.
    pub fn GetSlice(&self) -> &[kv::Entry] {
        &self.data
    }

    /// 游标是否仍指向有效条目。
    /// Valid reports whether the cursor points at an entry.
    pub fn Valid(&self) -> bool {
        self.cur >= 0 && (self.cur as usize) < self.data.len()
    }

    /// 返回当前键；无效游标时为空键（对齐 Go 的 nil key）。
    /// Key returns an empty key for an invalid cursor, matching Go's nil key.
    pub fn Key(&self) -> kv::Key {
        if !self.Valid() {
            return kv::Key::default();
        }
        self.data[self.cur as usize].Key.clone()
    }

    /// 返回当前值；无效游标时为空切片（对齐 Go 的 nil slice）。
    /// Value returns an empty value for an invalid cursor, matching Go's nil slice.
    pub fn Value(&self) -> Vec<u8> {
        if !self.Valid() {
            return Vec::new();
        }
        self.data[self.cur as usize].Value.clone()
    }

    /// 前进一次；已无效则报错。
    /// Next advances exactly once and rejects an already-invalid iterator.
    pub fn Next(&mut self) -> Result<(), kv::errors::SharedError> {
        if !self.Valid() {
            return Err(iterator_error("iterator is invalid"));
        }
        self.cur += 1;
        Ok(())
    }

    /// 关闭并使游标失效，不修改条目内容。
    /// Close invalidates the cursor without modifying the entries.
    pub fn Close(&mut self) {
        self.cur = -1;
    }
}

impl kv::Iterator for SliceIter {
    fn Valid(&self) -> bool {
        SliceIter::Valid(self)
    }

    fn Key(&self) -> kv::Key {
        SliceIter::Key(self)
    }

    fn Value(&self) -> Vec<u8> {
        SliceIter::Value(self)
    }

    fn Next(&mut self) -> Result<(), kv::errors::SharedError> {
        SliceIter::Next(self)
    }

    fn Close(&mut self) {
        SliceIter::Close(self)
    }
}

/// 包装底层 KV 迭代器，支持确定性注入 Next 错误与多次 Close 检测。
/// MockedIter wraps a KV iterator and supports deterministic error injection.
pub struct MockedIter {
    iterator: Box<dyn kv::Iterator>,
    next_err: Option<kv::errors::SharedError>,
    closed: bool,
    fail_on_multi_close: bool,
}

impl MockedIter {
    /// 构造包装迭代器；`fail_on_multi_close` 为真时重复 Close 会 panic。
    pub fn new(iterator: Box<dyn kv::Iterator>, fail_on_multi_close: bool) -> Self {
        Self {
            iterator,
            next_err: None,
            closed: false,
            fail_on_multi_close,
        }
    }

    /// 注入下一次 `Next` 将返回的错误消息。
    pub fn InjectNextError(&mut self, message: impl Into<String>) {
        self.next_err = Some(iterator_error(message));
    }

    /// 注入已构造好的共享错误值。
    pub fn InjectNextErrorValue(&mut self, error: kv::errors::SharedError) {
        self.next_err = Some(error);
    }

    /// 清除已注入的 Next 错误。
    pub fn ClearInjectedNextError(&mut self) {
        self.next_err = None;
    }

    /// 查看当前注入的 Next 错误。
    pub fn GetInjectedNextError(&self) -> Option<&kv::errors::SharedError> {
        self.next_err.as_ref()
    }

    /// 设置是否在多次 Close 时失败。
    pub fn FailOnMultiClose(&mut self, fail: bool) {
        self.fail_on_multi_close = fail;
    }

    /// 委托底层 `Valid`。
    pub fn Valid(&self) -> bool {
        self.iterator.Valid()
    }

    /// 委托底层 `Key`。
    pub fn Key(&self) -> kv::Key {
        self.iterator.Key()
    }

    /// 委托底层 `Value`。
    pub fn Value(&self) -> Vec<u8> {
        self.iterator.Value()
    }

    /// 若有注入错误则直接返回，否则委托底层 `Next`。
    pub fn Next(&mut self) -> Result<(), kv::errors::SharedError> {
        if let Some(error) = &self.next_err {
            return Err(error.clone());
        }
        self.iterator.Next()
    }

    /// 关闭包装器；可选检测重复关闭。
    pub fn Close(&mut self) {
        assert!(
            !(self.closed && self.fail_on_multi_close),
            "Multi close iter"
        );
        self.closed = true;
        self.iterator.Close();
    }

    /// 是否已经调用过 Close。
    pub fn Closed(&self) -> bool {
        self.closed
    }
}

/// 由记录切片构造 Mock 迭代器；重复 Close 会 panic 以失败当前测试。
/// NewMockIterFromRecords mirrors the Go helper without a `testing.T` argument;
/// a repeated close panics, which directly fails the active Rust test.
pub fn NewMockIterFromRecords(
    records: Vec<kv::Entry>,
    fail_on_multi_close: bool,
) -> Box<MockedIter> {
    Box::new(MockedIter::new(NewSliceIter(records), fail_on_multi_close))
}

impl kv::Iterator for MockedIter {
    fn Valid(&self) -> bool {
        MockedIter::Valid(self)
    }

    fn Key(&self) -> kv::Key {
        MockedIter::Key(self)
    }

    fn Value(&self) -> Vec<u8> {
        MockedIter::Value(self)
    }

    fn Next(&mut self) -> Result<(), kv::errors::SharedError> {
        MockedIter::Next(self)
    }

    fn Close(&mut self) {
        MockedIter::Close(self)
    }
}
