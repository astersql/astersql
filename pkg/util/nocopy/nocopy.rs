// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 禁止值拷贝的零大小标记（对齐 Go NoCopy + sync.Locker）。
//
// Go 中嵌入该类型并实现空 Lock/Unlock，可让 `go vet -copylocks` 发现意外拷贝；
// Rust 默认不可 Copy，本类型亦不实现 Clone，语义上等价于“不可拷贝”。

// NoCopy implements sync.Locker to make an object no copy
//
// Rust values are not copyable unless they explicitly implement `Copy` or
// `Clone`, so this zero-sized marker deliberately implements neither trait.
/// 零大小不可拷贝标记；嵌入其他结构以表达“勿拷贝”意图。
#[derive(Default)]
pub struct NoCopy;

impl NoCopy {
    // Lock is an empty function to implement sync.Locker interface
    /// 空操作，对齐 Go sync.Locker::Lock。
    pub fn lock(&self) {}

    // Unlock is an empty function to implement sync.Locker interface
    /// 空操作，对齐 Go sync.Locker::Unlock。
    pub fn unlock(&self) {}

    // Keep the Go method spelling for legacy callers.
    /// 保留 Go 方法名拼写的 Lock，供遗留调用方使用。
    #[allow(non_snake_case)]
    pub fn Lock(&self) {}

    // Keep the Go method spelling for legacy callers.
    /// 保留 Go 方法名拼写的 Unlock，供遗留调用方使用。
    #[allow(non_snake_case)]
    pub fn Unlock(&self) {}
}
