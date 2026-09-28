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

// 内核类型判定模块的 NextGen（下一代内核）实现。
//
// 数据库内核存在两种形态：Classic（经典内核，即传统的 TiDB 部署形态）与
// NextGen（下一代内核，面向云原生/存算分离等新架构的形态）。
// 该判定在编译期完成：本文件仅在启用 `nextgen` feature 时参与编译，
// 与之对应的 classic 实现文件则在未启用该 feature 时生效，
// 二者提供同名函数 `IsNextGen` / `IsClassic`，从而让上层代码
// 无需运行时开销即可按内核类型走不同逻辑分支。

// 本文件由 pkg/config/kerneltype/nextgen.go 迁移而来，保留 Go 实现结构。

// Go build tag: nextgen。
// Rust 使用 cfg(feature = "nextgen") 保留 Go 的编译期选择语义。

#![allow(non_snake_case)]

// IsNextGen returns true if the current kernel type is NextGen.
// see doc.go for more info.
/// 判断当前内核类型是否为 NextGen（下一代内核）。
///
/// 本文件仅在启用 `nextgen` feature 时编译，因此这里恒返回 `true`；
/// 编译期即可确定结果，编译器可据此消除死代码分支。
/// 更多背景说明见同目录的 doc 模块（对应 Go 侧 doc.go）。
#[cfg(feature = "nextgen")]
pub fn IsNextGen() -> bool {
    true
}

// IsClassic returns true if the current kernel type is Classic.
/// 判断当前内核类型是否为 Classic（经典内核）。
///
/// 定义为 `IsNextGen` 的取反，在 NextGen 构建下恒返回 `false`，
/// 保证两个判定函数的结果始终互斥且自洽。
#[cfg(feature = "nextgen")]
pub fn IsClassic() -> bool {
    !IsNextGen()
}
