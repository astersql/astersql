// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 规划器 base 接口设计约束说明。
//
// 本文件不导出可执行 API，只记录扩展 `Plan` / `LogicalPlan` / `PhysicalPlan`
// 等抽象时必须遵守的约定，避免接口膨胀、循环依赖与跨包实现顺序错乱。

// 修改 base 接口定义前应先阅读以下约束：
//
// 1. 接口应包含多数实现者共享的抽象逻辑；仅少数类型需要的能力不应加入公共接口。
//
// 2. 在此新增接口方法意味着所有实现类型都要补齐对应实现。与 Go 相同，方法实现应留在
//    实现类型所属包内；Rust 后续接线时也应避免把具体实现反向塞入本抽象层。
//
// 3. 接口必须依赖抽象语义，不能引用具体实现类型或 core 包的处理细节，否则容易形成循环依赖。
//
// 4. 确需新增方法时应追加到方法列表末尾，使不同包中的实现者可以按相同顺序定位和核对。
