// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

//! Pointer helpers ported from `br/pkg/utils/pointer.go`.
//! 可空指针取值辅助；Go 用泛型指针，Rust 映射为 `Option<&T>`。

/// Returns the value pointed to by `p`, or the zero value of `T` when `p` is `None`.
/// `None` 返回 `Default`，有值则 clone，避免对调用方强制解引用。
pub fn GetOrZero<T>(p: Option<&T>) -> T
where
    T: Default + Clone,
{
    match p {
        None => T::default(),
        Some(value) => value.clone(),
    }
}
