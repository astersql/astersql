// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0

// Cascades 上下文 crate 入口。
//
// 再导出 `cascades_ctx` 中的 `Context` / `RuleMask`，并在测试配置下挂载
// `cascades_ctx_aster_unit_test`。`non_snake_case` 允许保留与 Go 对齐的方法名。

#![allow(non_snake_case)]

mod cascades_ctx;
pub use cascades_ctx::*;

#[cfg(test)]
mod cascades_ctx_aster_unit_test;
