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

// KV 键（Key）、行句柄（Handle）及句柄映射实现。
//
// 提供字节序键运算（Next/PrefixNext）、左闭右开 KeyRange、整数/复合/分区句柄，
// 以及 HandleMap / MemAwareHandleMap 的查找与内存估算。
// 对齐 pkg/kv/key.go 的键、句柄、句柄映射及内存计量逻辑。

// 对齐 pkg/kv/key.go 的键、句柄、句柄映射及内存计量逻辑。

use std::any::Any;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::mem::size_of;

/// Key 对应 Go 的 []byte 高层键类型；元组结构避免丢失独立方法集合。
// Key 对应 Go 的 []byte 高层键类型；元组结构避免丢失独立方法集合。
#[derive(Clone, Debug, Default, Eq, PartialEq, Hash)]
pub struct Key(pub Vec<u8>);

impl AsRef<[u8]> for Key {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl Key {
    // Next 在键尾追加 0，得到严格大于当前键的最小字节序键。
    pub fn Next(&self) -> Key {
        let mut buf = Vec::with_capacity(self.0.len() + 1);
        buf.extend_from_slice(&self.0);
        buf.push(0);
        Key(buf)
    }

    // PrefixNext 对应 Go 的前缀上界算法：从末尾进位，全部为 0xff 时追加 0。
    pub fn PrefixNext(&self) -> Key {
        let mut buf = self.0.clone();
        let mut i = buf.len() as isize - 1;
        while i >= 0 {
            let idx = i as usize;
            buf[idx] = buf[idx].wrapping_add(1);
            if buf[idx] != 0 {
                break;
            }
            i -= 1;
        }
        if i == -1 {
            // Go 对空键或全 0xff 键恢复原内容后追加 0；这里保持相同边界行为。
            buf.clone_from(&self.0);
            buf.push(0);
        }
        Key(buf)
    }

    // Cmp 返回 -1、0、1，保持 bytes.Compare 的返回约定。
    pub fn Cmp(&self, another: &Key) -> i32 {
        match self.0.cmp(&another.0) {
            Ordering::Less => -1,
            Ordering::Equal => 0,
            Ordering::Greater => 1,
        }
    }

    // HasPrefix 判断当前键是否以给定字节序列开头。
    pub fn HasPrefix(&self, prefix: &Key) -> bool {
        self.0.starts_with(&prefix.0)
    }

    // Clone 对应 Go Clone；即使 Rust 另有 Clone trait，也保留来源方法名便于逐项核对。
    pub fn Clone(&self) -> Key {
        Key(self.0.clone())
    }

    // String 对应 hex.EncodeToString，逐字节生成小写十六进制文本。
    pub fn String(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }
}

// KeyRange 表示左闭右开区间 StartKey <= key < EndKey。
// Go 依赖它与 kvproto KeyRange 的布局兼容；Rust 表示保留字段顺序，但不承诺跨语言 ABI。
#[derive(Clone, Debug, Default)]
pub struct KeyRange {
    pub StartKey: Key,
    pub EndKey: Key,
}

// KeyRangeSliceMemUsage 按切片容量和两个键的容量估算内存，沿用 Go 的统计口径。
pub fn KeyRangeSliceMemUsage(k: &Vec<KeyRange>) -> i64 {
    let mut res = (size_of::<KeyRange>() * k.capacity()) as i64;
    for range in k {
        res += range.StartKey.0.capacity() as i64 + range.EndKey.0.capacity() as i64;
    }
    res
}

impl KeyRange {
    // IsPoint 判断区间是否恰好覆盖一个键，避免真正分配 Next/PrefixNext 的结果。
    pub fn IsPoint(&self) -> bool {
        if self.StartKey.0.len() != self.EndKey.0.len() {
            let start_len = self.StartKey.0.len();
            return start_len + 1 == self.EndKey.0.len()
                && self.EndKey.0[start_len] == 0
                && self.StartKey.0 == self.EndKey.0[..start_len];
        }

        let mut i = self.StartKey.0.len() as isize - 1;
        while i >= 0 {
            let idx = i as usize;
            if self.StartKey.0[idx] != 255 {
                break;
            }
            if self.EndKey.0[idx] != 0 {
                return false;
            }
            i -= 1;
        }
        // 全部起始字节均为 0xff 时，等长结束键不可能是合法的前缀后继。
        if i < 0 {
            return false;
        }
        let idx = i as usize;
        self.StartKey.0[idx].wrapping_add(1) == self.EndKey.0[idx]
            && self.StartKey.0[..idx] == self.EndKey.0[..idx]
    }
}

// Entry 对应单个键值项；Value 保持 Go []byte 的拥有型语义。
#[derive(Clone, Debug, Default)]
pub struct Entry {
    pub Key: Key,
    pub Value: Vec<u8>,
}

// Handle 对应 Go 行句柄接口。Datum、codec 错误及动态分发需要后续跨文件接线。
pub trait Handle: Any {
    fn as_any(&self) -> &dyn Any;
    fn IsInt(&self) -> bool;
    fn IntValue(&self) -> i64;
    fn Next(&self) -> Box<dyn Handle>;
    fn Equal(&self, h: &dyn Handle) -> bool;
    fn Compare(&self, h: &dyn Handle) -> i32;
    fn Encoded(&self) -> Vec<u8>;
    fn Len(&self) -> usize;
    fn NumCols(&self) -> usize;
    fn EncodedCol(&self, idx: usize) -> Vec<u8>;
    fn Data(&self) -> Result<Vec<types::Datum>, codec::Error>;
    fn String(&self) -> String;
    fn MemUsage(&self) -> u64;
    fn ExtraMemSize(&self) -> u64;
    fn Copy(&self) -> Box<dyn Handle>;
}

// IntHandle 以 i64 实现 Handle，保持整数句柄的定长编码语义。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct IntHandle(pub i64);

impl Handle for IntHandle {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn Copy(&self) -> Box<dyn Handle> {
        Box::new(*self)
    }
    fn IsInt(&self) -> bool {
        true
    }
    fn IntValue(&self) -> i64 {
        self.0
    }
    fn Next(&self) -> Box<dyn Handle> {
        Box::new(IntHandle(self.0.wrapping_add(1)))
    }

    fn Equal(&self, h: &dyn Handle) -> bool {
        h.IsInt() && self.0 == h.IntValue()
    }

    fn Compare(&self, h: &dyn Handle) -> i32 {
        if !h.IsInt() {
            // Go 在混合句柄类型比较时 panic；Rust 保留同样的失败方式。
            panic!("IntHandle compares to CommonHandle");
        }
        self.0.cmp(&h.IntValue()) as i32
    }

    fn Encoded(&self) -> Vec<u8> {
        codec::EncodeInt(Vec::new(), self.0)
    }
    fn Len(&self) -> usize {
        8
    }
    fn NumCols(&self) -> usize {
        panic!("not supported in IntHandle")
    }
    fn EncodedCol(&self, _idx: usize) -> Vec<u8> {
        panic!("not supported in IntHandle")
    }
    fn Data(&self) -> Result<Vec<types::Datum>, codec::Error> {
        Ok(vec![types::NewIntDatum(self.0)])
    }
    fn String(&self) -> String {
        self.0.to_string()
    }
    fn MemUsage(&self) -> u64 {
        size_of::<IntHandle>() as u64
    }
    fn ExtraMemSize(&self) -> u64 {
        0
    }
}

// CommonHandle 对应非整数复合句柄；colEndOffsets 保存每列编码的累计结束位置。
#[derive(Clone, Debug, Default)]
pub struct CommonHandle {
    encoded: Vec<u8>,
    colEndOffsets: Vec<u16>,
}

// NewCommonHandle 从 codec.EncodeKey 的结果切分列边界。
pub fn NewCommonHandle(encoded: Vec<u8>) -> Result<CommonHandle, codec::Error> {
    let mut ch = CommonHandle {
        encoded: encoded.clone(),
        colEndOffsets: Vec::new(),
    };
    if encoded.len() < 9 {
        // Go 为短编码补到 9 字节，但仍用原始 encoded 扫描列，二者不可混用。
        ch.encoded.resize(9, 0);
    }
    let mut remain = encoded;
    let mut end_off = 0u16;
    while !remain.is_empty() {
        if remain[0] == 0 {
            break;
        }
        let (col, rest) = codec::CutOne(remain)?;
        end_off = end_off.wrapping_add(col.len() as u16);
        ch.colEndOffsets.push(end_off);
        remain = rest;
    }
    Ok(ch)
}

impl Handle for CommonHandle {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn Copy(&self) -> Box<dyn Handle> {
        Box::new(self.clone())
    }
    fn IsInt(&self) -> bool {
        false
    }
    fn IntValue(&self) -> i64 {
        panic!("not supported in CommonHandle")
    }

    fn Next(&self) -> Box<dyn Handle> {
        Box::new(CommonHandle {
            encoded: Key(self.encoded.clone()).PrefixNext().0,
            colEndOffsets: self.colEndOffsets.clone(),
        })
    }

    fn Equal(&self, h: &dyn Handle) -> bool {
        !h.IsInt() && self.encoded == h.Encoded()
    }

    fn Compare(&self, h: &dyn Handle) -> i32 {
        if h.IsInt() {
            panic!("CommonHandle compares to IntHandle");
        }
        match self.encoded.cmp(&h.Encoded()) {
            Ordering::Less => -1,
            Ordering::Equal => 0,
            Ordering::Greater => 1,
        }
    }

    fn Encoded(&self) -> Vec<u8> {
        self.encoded.clone()
    }
    fn Len(&self) -> usize {
        self.encoded.len()
    }
    fn NumCols(&self) -> usize {
        self.colEndOffsets.len()
    }

    fn EncodedCol(&self, idx: usize) -> Vec<u8> {
        let start = if idx > 0 {
            self.colEndOffsets[idx - 1] as usize
        } else {
            0
        };
        self.encoded[start..self.colEndOffsets[idx] as usize].to_vec()
    }

    fn Data(&self) -> Result<Vec<types::Datum>, codec::Error> {
        let mut data = Vec::with_capacity(self.NumCols());
        for i in 0..self.NumCols() {
            // DecodeOne 的剩余字节在 Go 中被忽略，只保留当前列 Datum。
            let (_, datum) = codec::DecodeOne(&self.EncodedCol(i))?;
            data.push(datum);
        }
        Ok(data)
    }

    fn String(&self) -> String {
        let data = match self.Data() {
            Ok(data) => data,
            Err(err) => return err.to_string(),
        };
        let mut strs = Vec::with_capacity(self.NumCols());
        for datum in data {
            match datum.ToString() {
                Ok(text) => strs.push(text),
                Err(err) => return err.to_string(),
            }
        }
        format!("{{{}}}", strs.join(", "))
    }

    fn MemUsage(&self) -> u64 {
        size_of::<CommonHandle>() as u64 + self.ExtraMemSize()
    }

    fn ExtraMemSize(&self) -> u64 {
        self.encoded.capacity() as u64 + (self.colEndOffsets.capacity() * size_of::<u16>()) as u64
    }
}

// StrHandleVal 为字符串键保留原 Handle，以便 Range 时还原具体动态类型。
struct StrHandleVal {
    h: Box<dyn Handle>,
    val: Box<dyn Any>,
}

// HandleMap 按整数/复合键及普通/分区句柄拆成四组映射，保持 Go 的查找布局。
pub struct HandleMap {
    ints: HashMap<i64, Box<dyn Any>>,
    strs: HashMap<Vec<u8>, StrHandleVal>,
    partitionInts: HashMap<i64, HashMap<i64, Box<dyn Any>>>,
    partitionStrs: HashMap<i64, HashMap<Vec<u8>, StrHandleVal>>,
}

pub const SizeofHandleMap: i64 = size_of::<HandleMap>() as i64;
pub const SizeofStrHandleVal: i64 = size_of::<StrHandleVal>() as i64;

// NewHandleMap 初始化所有映射，避免 Set 时处理普通映射的空值。
pub fn NewHandleMap() -> HandleMap {
    HandleMap {
        ints: HashMap::new(),
        strs: HashMap::new(),
        partitionInts: HashMap::new(),
        partitionStrs: HashMap::new(),
    }
}

impl HandleMap {
    // Get 按 PartitionHandle 的 pid 先选择子映射，再按底层句柄类型查找。
    pub fn Get(&self, h: &dyn Handle) -> Option<&dyn Any> {
        if let Some(ph) = h.as_any().downcast_ref::<PartitionHandle>() {
            if h.IsInt() {
                return self
                    .partitionInts
                    .get(&ph.PartitionID)?
                    .get(&h.IntValue())
                    .map(|v| v.as_ref());
            }
            let key = h.Encoded();
            return self
                .partitionStrs
                .get(&ph.PartitionID)?
                .get(&key)
                .map(|v| v.val.as_ref());
        }
        if h.IsInt() {
            self.ints.get(&h.IntValue()).map(|v| v.as_ref())
        } else {
            let key = h.Encoded();
            self.strs.get(&key).map(|v| v.val.as_ref())
        }
    }

    // MemUsage 只按 map 元素、字符串容量及结构体大小估算，不追踪 value 指向对象。
    pub fn MemUsage(&self) -> i64 {
        let mut res = SizeofHandleMap;
        res += self.partitionInts.len() as i64 * (size::SizeOfInt64 + size::SizeOfMap);
        for values in self.partitionInts.values() {
            res += calcIntsMemUsage(values);
        }
        res += self.partitionStrs.len() as i64 * (size::SizeOfInt64 + size::SizeOfMap);
        for values in self.partitionStrs.values() {
            res += calcStrsMemUsage(values);
        }
        res + calcIntsMemUsage(&self.ints) + calcStrsMemUsage(&self.strs)
    }

    // Set 为分区句柄按需创建二级 map；复合句柄同时保存编码字符串和句柄副本。
    pub fn Set(&mut self, h: &dyn Handle, val: Box<dyn Any>) {
        if let Some(ph) = h.as_any().downcast_ref::<PartitionHandle>() {
            if h.IsInt() {
                self.partitionInts
                    .entry(ph.PartitionID)
                    .or_default()
                    .insert(h.IntValue(), val);
            } else {
                let key = h.Encoded();
                self.partitionStrs
                    .entry(ph.PartitionID)
                    .or_default()
                    .insert(key, StrHandleVal { h: h.Copy(), val });
            }
            return;
        }
        if h.IsInt() {
            self.ints.insert(h.IntValue(), val);
        } else {
            let key = h.Encoded();
            self.strs.insert(key, StrHandleVal { h: h.Copy(), val });
        }
    }

    // Delete 对不存在的分区子映射直接返回，与 Go nil map 检查一致。
    pub fn Delete(&mut self, h: &dyn Handle) {
        if let Some(ph) = h.as_any().downcast_ref::<PartitionHandle>() {
            if h.IsInt() {
                if let Some(values) = self.partitionInts.get_mut(&ph.PartitionID) {
                    values.remove(&h.IntValue());
                }
            } else if let Some(values) = self.partitionStrs.get_mut(&ph.PartitionID) {
                values.remove(&h.Encoded());
            }
            return;
        }
        if h.IsInt() {
            self.ints.remove(&h.IntValue());
        } else {
            self.strs.remove(&h.Encoded());
        }
    }

    // Len 汇总四组 map 中的实际条目数。
    pub fn Len(&self) -> usize {
        self.ints.len()
            + self.strs.len()
            + self.partitionInts.values().map(HashMap::len).sum::<usize>()
            + self.partitionStrs.values().map(HashMap::len).sum::<usize>()
    }

    // Range 按 Go 源码顺序遍历；回调返回 false 时立即结束。
    pub fn Range(&self, mut f: impl FnMut(&dyn Handle, &dyn Any) -> bool) {
        for (h, val) in &self.ints {
            if !f(&IntHandle(*h), val.as_ref()) {
                return;
            }
        }
        for val in self.strs.values() {
            if !f(val.h.as_ref(), val.val.as_ref()) {
                return;
            }
        }
        for (pid, values) in &self.partitionInts {
            for (h, val) in values {
                let ph = NewPartitionHandle(*pid, Box::new(IntHandle(*h)));
                if !f(&ph, val.as_ref()) {
                    return;
                }
            }
        }
        for values in self.partitionStrs.values() {
            for val in values.values() {
                if !f(val.h.as_ref(), val.val.as_ref()) {
                    return;
                }
            }
        }
    }
}

fn calcStrsMemUsage(strs: &HashMap<Vec<u8>, StrHandleVal>) -> i64 {
    strs.keys()
        .map(|key| size::SizeOfString + key.len() as i64 + SizeofStrHandleVal)
        .sum()
}

fn calcIntsMemUsage<V>(ints: &HashMap<i64, V>) -> i64 {
    ints.len() as i64 * (size::SizeOfInt64 + size::SizeOfInterface)
}

// StrHandleValue 是泛型内存感知映射的复合句柄值。
struct StrHandleValue<V> {
    h: Box<dyn Handle>,
    val: V,
}

// MemAwareHandleMap 对应 Go 泛型结构；底层 MemAwareMap.Set 返回本次写入的内存变化量。
pub struct MemAwareHandleMap<V> {
    ints: hack::MemAwareMap<i64, V>,
    strs: hack::MemAwareMap<Vec<u8>, StrHandleValue<V>>,
    partitionInts: HashMap<i64, hack::MemAwareMap<i64, V>>,
    partitionStrs: HashMap<i64, hack::MemAwareMap<Vec<u8>, StrHandleValue<V>>>,
}

// NewMemAwareHandleMap 初始化普通映射；分区映射仍在首次 Set 时创建。
pub fn NewMemAwareHandleMap<V>() -> MemAwareHandleMap<V> {
    MemAwareHandleMap {
        ints: *hack::NewMemAwareMap(0),
        strs: *hack::NewMemAwareMap(0),
        partitionInts: HashMap::new(),
        partitionStrs: HashMap::new(),
    }
}

impl<V> MemAwareHandleMap<V> {
    // Get 的选择规则与 HandleMap 相同，但返回泛型值引用。
    pub fn Get(&self, h: &dyn Handle) -> Option<&V> {
        if let Some(ph) = h.as_any().downcast_ref::<PartitionHandle>() {
            if h.IsInt() {
                return self
                    .partitionInts
                    .get(&ph.PartitionID)?
                    .M
                    .get(&h.IntValue());
            }
            let key = h.Encoded();
            return self
                .partitionStrs
                .get(&ph.PartitionID)?
                .M
                .get(&key)
                .map(|v| &v.val);
        }
        if h.IsInt() {
            self.ints.M.get(&h.IntValue())
        } else {
            let key = h.Encoded();
            self.strs.M.get(&key).map(|v| &v.val)
        }
    }

    // Set 保留 Go 返回内存增量的约定，分区子映射使用固定初始开销 0。
    pub fn Set(&mut self, h: &dyn Handle, val: V) -> i64 {
        if let Some(ph) = h.as_any().downcast_ref::<PartitionHandle>() {
            if h.IsInt() {
                return self
                    .partitionInts
                    .entry(ph.PartitionID)
                    .or_insert_with(newMemAwareMap)
                    .Set(h.IntValue(), val);
            }
            let key = h.Encoded();
            return self
                .partitionStrs
                .entry(ph.PartitionID)
                .or_insert_with(newMemAwareMap)
                .Set(key, StrHandleValue { h: h.Copy(), val });
        }
        if h.IsInt() {
            self.ints.Set(h.IntValue(), val)
        } else {
            let key = h.Encoded();
            self.strs.Set(key, StrHandleValue { h: h.Copy(), val })
        }
    }

    // Range 同样支持回调提前终止，且为整数分区项临时恢复 PartitionHandle。
    pub fn Range(&self, mut f: impl FnMut(&dyn Handle, &V) -> bool) {
        for (h, val) in &self.ints.M {
            if !f(&IntHandle(*h), val) {
                return;
            }
        }
        for val in self.strs.M.values() {
            if !f(val.h.as_ref(), &val.val) {
                return;
            }
        }
        for (pid, values) in &self.partitionInts {
            for (h, val) in &values.M {
                let ph = NewPartitionHandle(*pid, Box::new(IntHandle(*h)));
                if !f(&ph, val) {
                    return;
                }
            }
        }
        for values in self.partitionStrs.values() {
            for val in values.M.values() {
                if !f(val.h.as_ref(), &val.val) {
                    return;
                }
            }
        }
    }
}

// PartitionHandle 将底层句柄与分区 ID 组合，用于定位分区表中的行。
pub struct PartitionHandle {
    pub Handle: Box<dyn Handle>,
    pub PartitionID: i64,
}

pub fn NewPartitionHandle(pid: i64, h: Box<dyn Handle>) -> PartitionHandle {
    PartitionHandle {
        Handle: h,
        PartitionID: pid,
    }
}

impl Handle for PartitionHandle {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn Copy(&self) -> Box<dyn Handle> {
        Box::new(PartitionHandle {
            Handle: self.Handle.Copy(),
            PartitionID: self.PartitionID,
        })
    }
    fn IsInt(&self) -> bool {
        self.Handle.IsInt()
    }
    fn IntValue(&self) -> i64 {
        self.Handle.IntValue()
    }
    fn Next(&self) -> Box<dyn Handle> {
        self.Handle.Next()
    }

    fn Equal(&self, h: &dyn Handle) -> bool {
        if let Some(other) = h.as_any().downcast_ref::<PartitionHandle>() {
            return self.PartitionID == other.PartitionID
                && self.Handle.Equal(other.Handle.as_ref());
        }
        // Go 允许与非分区句柄比较相等，此时只比较底层句柄。
        self.Handle.Equal(h)
    }

    fn Compare(&self, h: &dyn Handle) -> i32 {
        if let Some(other) = h.as_any().downcast_ref::<PartitionHandle>() {
            return match self.PartitionID.cmp(&other.PartitionID) {
                Ordering::Less => -1,
                Ordering::Greater => 1,
                Ordering::Equal => self.Handle.Compare(other.Handle.as_ref()),
            };
        }
        panic!("PartitonHandle compares to non-parition Handle")
    }

    fn Encoded(&self) -> Vec<u8> {
        self.Handle.Encoded()
    }
    fn Len(&self) -> usize {
        self.Handle.Len()
    }
    fn NumCols(&self) -> usize {
        self.Handle.NumCols()
    }
    fn EncodedCol(&self, idx: usize) -> Vec<u8> {
        self.Handle.EncodedCol(idx)
    }
    fn Data(&self) -> Result<Vec<types::Datum>, codec::Error> {
        self.Handle.Data()
    }
    fn String(&self) -> String {
        self.Handle.String()
    }
    fn MemUsage(&self) -> u64 {
        self.Handle.MemUsage() + size_of::<PartitionHandle>() as u64
    }
    fn ExtraMemSize(&self) -> u64 {
        self.Handle.ExtraMemSize()
    }
}

// memAwareMap 保留 Go 泛型别名，使本文件的名称和 hack 实现之间关系清晰可查。
type memAwareMap<K, V> = hack::MemAwareMap<K, V>;

// newMemAwareMap 对应 Go hack.NewMemAwareMap(0)，0 表示不附加初始内存计数。
fn newMemAwareMap<K: Eq + std::hash::Hash, V>() -> memAwareMap<K, V> {
    *hack::NewMemAwareMap(0)
}
