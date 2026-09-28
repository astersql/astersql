// Copyright 2020 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// `utf8mb4_zh_pinyin_tidb_as_cs` 拼音排序规则 Collator 占位实现。
//
// 对应 Go `pinyin_tidb_as_cs.go`：接口形状已迁入，比较 / 排序 key / 通配符等
// 业务逻辑在 Go 侧仍为 `panic("implement me")`，此处保持相同未实现语义。
// AS CS 表示 accent-sensitive、case-sensitive（区分音调与大小写）。

// 这段逻辑记录 utf8mb4_zh_pinyin_tidb_as_cs Collator 的接口形状；Go 源文件本身仍是未实现 panic。

use std::any::Any;

use crate::{Collator, WildcardPattern};

// Collation of utf8mb4_zh_pinyin_tidb_as_cs
/// 拼音排序 Collator 空结构体；方法全部 panic，避免误用未实现规则。
// zhPinyinTiDBASCSCollator 对应 Go 的空结构体，当前功能仍在开发中。
#[derive(Default)]
pub struct zhPinyinTiDBASCSCollator {}

impl zhPinyinTiDBASCSCollator {
    // Compare is not implemented.
    // Go 源码直接 panic("implement me")；这里不虚构比较规则。
    pub fn Compare(&self, _a: &str, _b: &str) -> i32 {
        panic!("implement me")
    }

    // Key is not implemented.
    // Go 未定义排序 key 生成逻辑，保留 panic 作为占位语义。
    pub fn Key(&self, _str_: &str) -> Vec<u8> {
        panic!("implement me")
    }

    // ImmutableKey implement Collator interface.
    // Go 源文件同样 panic，说明暂未提供不可变 key 优化路径。
    pub fn ImmutableKey(&self, _str_: &str) -> Vec<u8> {
        panic!("implement me")
    }

    // KeyWithoutTrimRightSpace is not implemented.
    // padding/非 padding 的差异尚无 Go 实现，不补业务逻辑。
    pub fn KeyWithoutTrimRightSpace(&self, _str_: &str) -> Vec<u8> {
        panic!("implement me")
    }

    // MaxKeyLen implements Collator interface.
    // Go 没有给出最大 key 长度公式，因此继续保持 panic。
    pub fn MaxKeyLen(&self, _s: &str) -> usize {
        panic!("implement me")
    }

    // Pattern is not implemented.
    // 通配符匹配依赖排序规则，Go 侧未实现时这里也不构造虚假的 Pattern。
    pub fn Pattern(&self) -> Box<dyn WildcardPattern> {
        panic!("implement me")
    }

    // Clone is not implemented.
    // Go 侧连 Clone 也保留 panic，说明该 Collator 不应被实际启用。
    pub fn Clone(&self) -> Box<dyn Collator> {
        panic!("implement me")
    }
}

/// 将固有方法转发到 `Collator` trait，保持与其它 Collator 一致的动态分派入口。
impl Collator for zhPinyinTiDBASCSCollator {
    fn Compare(&self, a: &str, b: &str) -> i32 {
        zhPinyinTiDBASCSCollator::Compare(self, a, b)
    }

    fn Key(&self, value: &str) -> Vec<u8> {
        zhPinyinTiDBASCSCollator::Key(self, value)
    }

    fn ImmutableKey(&self, value: &str) -> Vec<u8> {
        zhPinyinTiDBASCSCollator::ImmutableKey(self, value)
    }

    fn KeyWithoutTrimRightSpace(&self, value: &str) -> Vec<u8> {
        zhPinyinTiDBASCSCollator::KeyWithoutTrimRightSpace(self, value)
    }

    fn Pattern(&self) -> Box<dyn WildcardPattern> {
        zhPinyinTiDBASCSCollator::Pattern(self)
    }

    fn Clone(&self) -> Box<dyn Collator> {
        zhPinyinTiDBASCSCollator::Clone(self)
    }

    fn MaxKeyLen(&self, value: &str) -> i32 {
        zhPinyinTiDBASCSCollator::MaxKeyLen(self, value) as i32
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
