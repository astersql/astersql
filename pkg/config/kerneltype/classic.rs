// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 本文件由 pkg/config/kerneltype/classic.go 迁移而来，保留 Go 实现结构。

// Go build tag: !nextgen。
// Rust 使用 cfg(feature = "nextgen") 的反向条件保留 Go 的编译期选择语义。

#![allow(non_snake_case)]

// 内核类型（kernel type）判定模块 —— Classic（经典）内核分支。
//
// 数据库内核存在两种形态：
// - Classic：经典架构，即传统的存算一体部署形态；
// - NextGen：下一代架构，通常指存算分离等新形态。
//
// 本文件对应未启用 `nextgen` feature 时的编译分支（等价于 Go 的
// `//go:build !nextgen` 构建标签），此时内核类型固定为 Classic。
// 全部判定在编译期确定，运行时无任何开销。

// IsNextGen returns true if the current kernel type is NextGen.
// see doc.go for more info.
/// 判断当前内核类型是否为 NextGen（下一代架构）。
/// 在 Classic 编译分支下恒返回 `false`。
#[cfg(not(feature = "nextgen"))]
pub fn IsNextGen() -> bool {
    false
}

// IsClassic returns true if the current kernel type is Classic.
/// 判断当前内核类型是否为 Classic（经典架构）。
/// 定义为 NextGen 的取反，在本编译分支下恒返回 `true`。
#[cfg(not(feature = "nextgen"))]
pub fn IsClassic() -> bool {
    !IsNextGen()
}
