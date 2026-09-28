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

// 规划器 core 包中的访问对象占位模块。
//
// Go 源文件仅保留 package 声明；真实的 Access Object 实现位于
// `planner/core/access`。本模块刻意保持空生产边界，避免在 core 根包
// 与 access 子包之间形成重复定义或循环依赖。

// The Go source intentionally contains only the package declaration. Access
// object implementations live in planner/core/access, so this module remains
// an intentionally empty production boundary.
// Go 源刻意只有 package 声明；访问对象实现在 planner/core/access，
// 因此本模块作为空的生产边界保留。
