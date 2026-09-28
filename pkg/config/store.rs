// Copyright 2024 PingCAP, Inc.
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

// 存储类型定义模块。
//
// 本模块定义了 `StoreType`（存储引擎类型）及其合法取值常量，
// 用于在配置层面标识 AsterSQL 底层使用的存储引擎：
// - `tikv`：生产环境使用的分布式 KV 存储（可能组合 TiKV/TiFlash/TiDB 等引擎）；
// - `unistore`：基于 badger 实现的单机存储，仅用于测试；
// - `mocktikv`：基于 goleveldb 实现的模拟 TiKV，仅用于测试。
//
// 原 Go 版本中该类型本应放在 pkg/store，但为避免循环依赖而放在 config 中。

#![allow(non_snake_case, non_upper_case_globals, dead_code)]

use std::borrow::Cow;

// StoreType is the type of storage.
// TODO maybe put it inside pkg/store, but it introduces a cycle import.
/// StoreType 表示存储引擎类型，本质是对字符串的新类型（newtype）封装。
///
/// 内部使用 `Cow<'static, str>`：常量取值时借用静态字符串（零拷贝），
/// 从运行时字符串构造时则持有所有权，兼顾两种来源。
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct StoreType(pub Cow<'static, str>);

// StoreTypeTiKV is TiKV type. the underlying storage engines might be one or
// multiple of TiKV/TiFlash/TiDB, see kv.StoreType for more details.
/// TiKV 存储类型。TiKV 是分布式事务型 KV 存储（行存），底层实际引擎可能是
/// TiKV/TiFlash（列存副本，用于分析型查询）/TiDB 之一或多者组合。
pub const StoreTypeTiKV: StoreType = StoreType(Cow::Borrowed("tikv"));
// StoreTypeUniStore is UniStore type which we implemented using badger, for test only.
/// UniStore 存储类型：基于 badger（Go 的 LSM-tree KV 库）实现的单机内嵌存储，仅用于测试。
pub const StoreTypeUniStore: StoreType = StoreType(Cow::Borrowed("unistore"));
// StoreTypeMockTiKV is MockTiKV type which we implemented using goleveldb, for test only.
/// MockTiKV 存储类型：基于 goleveldb 实现的模拟 TiKV，仅用于测试。
pub const StoreTypeMockTiKV: StoreType = StoreType(Cow::Borrowed("mocktikv"));

impl StoreType {
    // String implements fmt.Stringer interface.
    // String 对应 Go 的 Stringer 方法，直接返回底层存储类型字符串。
    /// 返回存储类型的字符串表示（如 "tikv"）。
    pub fn String(&self) -> &str {
        self.0.as_ref()
    }

    // Valid returns true if the storage type is valid.
    // Valid 对应 Go switch：只接受三个已声明常量。
    /// 判断当前存储类型是否为合法取值（tikv/unistore/mocktikv 之一）。
    pub fn Valid(&self) -> bool {
        self == &StoreTypeTiKV || self == &StoreTypeUniStore || self == &StoreTypeMockTiKV
    }
}

/// 从运行时拥有所有权的 `String` 构造 `StoreType`（如解析配置文件时）。
impl From<String> for StoreType {
    fn from(value: String) -> Self {
        Self(Cow::Owned(value))
    }
}

/// 从静态字符串字面量构造 `StoreType`，直接借用避免分配。
impl From<&'static str> for StoreType {
    fn from(value: &'static str) -> Self {
        Self(Cow::Borrowed(value))
    }
}

// StoreTypeList returns all valid storage types.
// StoreTypeList 对应 Go 的字面量切片返回；调用方可据此枚举合法值。
/// 返回全部合法存储类型的列表，用于校验或枚举场景。
pub fn StoreTypeList() -> Vec<StoreType> {
    vec![StoreTypeTiKV, StoreTypeUniStore, StoreTypeMockTiKV]
}
