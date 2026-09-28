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

// KV 迭代器辅助：按键比较条件推进，直到命中或迭代结束。
//
// 对齐 pkg/kv/iter.go 的迭代停止条件和错误传播顺序。

/// NextUntil 对迭代器的每个有效条目应用 FnKeyCmp，直到比较函数返回 true。
/// 迭代器失效或 Next 返回错误也会停止；函数不会替调用方关闭迭代器。
pub fn NextUntil(
    it: &mut dyn Iterator,
    mut fnKeyCmp: impl FnMut(Key) -> bool,
) -> Result<(), Error> {
    // 先检查 Valid，再读取 Key，严格避免在无效迭代器上访问当前键。
    while it.Valid() && !fnKeyCmp(it.Key()) {
        // 每轮只推进一次；底层迭代可能执行 IO，错误原样立即返回。
        it.Next()?;
    }

    // 比较函数命中或迭代自然结束都对应 Go 的 nil error。
    Ok(())
}
