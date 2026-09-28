// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// 测试导出入口：向测试暴露时间戳校验函数。
//
// 对照 Go `export_test.go`，将生产代码中的
// `CheckTimestampTypeForTest` 再导出供集成测试调用。

// 对照 pkg/types/export_test.go，导出时间戳检查函数供测试使用。

#![allow(dead_code)]
#![allow(non_upper_case_globals)]

// 对应 Go 的测试导出变量，复用生产 crate 中已经按 CoreTime/Tz 接线的校验入口。
/// 测试用时间戳类型校验入口（再导出）。
pub use crate::time::CheckTimestampTypeForTest;
