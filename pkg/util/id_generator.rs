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

// 自增 ID 生成器工具。
//
// 对应 Go `pkg/util` 中的 `IDGenerator`：维护一个单调递增的整数计数器，
// 每次调用返回当前值并将内部计数加一，常用于测试或需要简单唯一序号的场景。

// IDGenerator util class used for generate auto-increasing id
// IDGenerator 对应 Go 结构体 `type IDGenerator struct`，内部只保存下一个待返回的整数。
/// 自增整数 ID 生成器；`nextID` 为下一次将返回的值。
#[derive(Clone, Debug, Default)]
pub struct IDGenerator {
    /// 下一个待分配的 ID（尚未返回）。
    pub nextID: isize,
}

impl IDGenerator {
    // GetNextID return the id++
    // GetNextID 保留 Go 的后缀自增语义：先返回旧 nextID，再把计数器加一。
    /// 返回当前 `nextID`，再将计数器加一（后缀自增语义）。
    pub fn GetNextID(&mut self) -> isize {
        let curID = self.nextID;
        // Go 中 `g.nextID++` 会原地更新接收者；用 &mut self 表达同样的可变方法。
        self.nextID = self.nextID.wrapping_add(1);
        curID
    }
}
