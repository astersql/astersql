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

//! 中文说明开始（自动生成）
//! 中文总览：`import_into_test.rs` 只补充中文说明，不改任何可执行逻辑。
//! 该文件围绕 `import_into_test` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、对象存储、任务状态或锁语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 195 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `fmap` 是当前文件里的辅助函数。
//! 阅读 `fmap` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `fmap` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `fmap`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `fmap` 的重要阅读参照。
//! 理解 `fmap` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `fmap` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 把 `fmap` 当成当前主题的局部锚点，有助于快速定位相关逻辑。
//! 符号 `auth_as` 是当前文件里的辅助函数。
//! 阅读 `auth_as` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `auth_as` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `auth_as`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `auth_as` 的重要阅读参照。
//! 理解 `auth_as` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `auth_as` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 把 `auth_as` 当成当前主题的局部锚点，有助于快速定位相关逻辑。
//! 符号 `test_import_into_privilege_positive_case` 是当前文件里的辅助函数。
//! 阅读 `test_import_into_privilege_positive_case` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_import_into_privilege_positive_case` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_import_into_privilege_positive_case`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `test_import_into_privilege_positive_case` 的重要阅读参照。
//! 理解 `test_import_into_privilege_positive_case` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `test_import_into_privilege_positive_case` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 把 `test_import_into_privilege_positive_case` 当成当前主题的局部锚点，有助于快速定位相关逻辑。
//! 符号 `test_import_into_stats_update` 是当前文件里的辅助函数。
//! 阅读 `test_import_into_stats_update` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_import_into_stats_update` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_import_into_stats_update`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `test_import_into_stats_update` 的重要阅读参照。
//! 理解 `test_import_into_stats_update` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `test_import_into_stats_update` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 把 `test_import_into_stats_update` 当成当前主题的局部锚点，有助于快速定位相关逻辑。
//! 符号 `test_basic_import_into` 是当前文件里的辅助函数。
//! 阅读 `test_basic_import_into` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_basic_import_into` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_basic_import_into`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `test_basic_import_into` 的重要阅读参照。
//! 理解 `test_basic_import_into` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `test_basic_import_into` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 把 `test_basic_import_into` 当成当前主题的局部锚点，有助于快速定位相关逻辑。
//! 关注点 001：继续从前置准备的角度理解这一段逻辑，避免把环境搭建误读成业务断言。（主题：`import_into_test`）。
//! 关注点 002：继续从主路径执行的角度理解这一段逻辑，确认核心动作发生在何处。（主题：`import_into_test`）。
//! 关注点 003：继续从结果断言的角度理解这一段逻辑，重点关注稳定可观察输出。（主题：`import_into_test`）。
//! 关注点 004：继续从资源收尾的角度理解这一段逻辑，留意清理和状态回收何时发生。（主题：`import_into_test`）。
//! 关注点 005：继续对照 Go 同名场景阅读这一段逻辑，避免把桩行为当成真实实现。（主题：`import_into_test`）。
//! 关注点 006：继续关注错误文本、结果行和状态枚举，这些通常是回归时最稳定的观察点。（主题：`import_into_test`）。
//! 关注点 007：继续把 SQL、对象存储和任务状态放在同一条数据流里理解。（主题：`import_into_test`）。
//! 关注点 008：继续优先相信现有断言与调用顺序，注释只负责补足阅读背景。（主题：`import_into_test`）。
//! 关注点 009：继续从前置准备的角度理解这一段逻辑，避免把环境搭建误读成业务断言。（主题：`import_into_test`）。
//! 关注点 010：继续从主路径执行的角度理解这一段逻辑，确认核心动作发生在何处。（主题：`import_into_test`）。
//! 关注点 011：继续从结果断言的角度理解这一段逻辑，重点关注稳定可观察输出。（主题：`import_into_test`）。
//! 关注点 012：继续从资源收尾的角度理解这一段逻辑，留意清理和状态回收何时发生。（主题：`import_into_test`）。
//! 关注点 013：继续对照 Go 同名场景阅读这一段逻辑，避免把桩行为当成真实实现。（主题：`import_into_test`）。
//! 关注点 014：继续关注错误文本、结果行和状态枚举，这些通常是回归时最稳定的观察点。（主题：`import_into_test`）。
//! 关注点 015：继续把 SQL、对象存储和任务状态放在同一条数据流里理解。（主题：`import_into_test`）。
//! 关注点 016：继续优先相信现有断言与调用顺序，注释只负责补足阅读背景。（主题：`import_into_test`）。
//! 关注点 017：继续从前置准备的角度理解这一段逻辑，避免把环境搭建误读成业务断言。（主题：`import_into_test`）。
//! 关注点 018：继续从主路径执行的角度理解这一段逻辑，确认核心动作发生在何处。（主题：`import_into_test`）。
//! 关注点 019：继续从结果断言的角度理解这一段逻辑，重点关注稳定可观察输出。（主题：`import_into_test`）。
//! 关注点 020：继续从资源收尾的角度理解这一段逻辑，留意清理和状态回收何时发生。（主题：`import_into_test`）。
//! 关注点 021：继续对照 Go 同名场景阅读这一段逻辑，避免把桩行为当成真实实现。（主题：`import_into_test`）。
//! 关注点 022：继续关注错误文本、结果行和状态枚举，这些通常是回归时最稳定的观察点。（主题：`import_into_test`）。
//! 关注点 023：继续把 SQL、对象存储和任务状态放在同一条数据流里理解。（主题：`import_into_test`）。
//! 关注点 024：继续优先相信现有断言与调用顺序，注释只负责补足阅读背景。（主题：`import_into_test`）。
//! 关注点 025：继续从前置准备的角度理解这一段逻辑，避免把环境搭建误读成业务断言。（主题：`import_into_test`）。
//! 关注点 026：继续从主路径执行的角度理解这一段逻辑，确认核心动作发生在何处。（主题：`import_into_test`）。
//! 关注点 027：继续从结果断言的角度理解这一段逻辑，重点关注稳定可观察输出。（主题：`import_into_test`）。
//! 关注点 028：继续从资源收尾的角度理解这一段逻辑，留意清理和状态回收何时发生。（主题：`import_into_test`）。
//! 关注点 029：继续对照 Go 同名场景阅读这一段逻辑，避免把桩行为当成真实实现。（主题：`import_into_test`）。
//! 关注点 030：继续关注错误文本、结果行和状态枚举，这些通常是回归时最稳定的观察点。（主题：`import_into_test`）。
//! 关注点 031：继续把 SQL、对象存储和任务状态放在同一条数据流里理解。（主题：`import_into_test`）。
//! 关注点 032：继续优先相信现有断言与调用顺序，注释只负责补足阅读背景。（主题：`import_into_test`）。
//! 关注点 033：继续从前置准备的角度理解这一段逻辑，避免把环境搭建误读成业务断言。（主题：`import_into_test`）。
//! 关注点 034：继续从主路径执行的角度理解这一段逻辑，确认核心动作发生在何处。（主题：`import_into_test`）。
//! 关注点 035：继续从结果断言的角度理解这一段逻辑，重点关注稳定可观察输出。（主题：`import_into_test`）。
//! 关注点 036：继续从资源收尾的角度理解这一段逻辑，留意清理和状态回收何时发生。（主题：`import_into_test`）。
//! 关注点 037：继续对照 Go 同名场景阅读这一段逻辑，避免把桩行为当成真实实现。（主题：`import_into_test`）。
//! 关注点 038：继续关注错误文本、结果行和状态枚举，这些通常是回归时最稳定的观察点。（主题：`import_into_test`）。
//! 关注点 039：继续把 SQL、对象存储和任务状态放在同一条数据流里理解。（主题：`import_into_test`）。
//! 关注点 040：继续优先相信现有断言与调用顺序，注释只负责补足阅读背景。（主题：`import_into_test`）。
//! 关注点 041：继续从前置准备的角度理解这一段逻辑，避免把环境搭建误读成业务断言。（主题：`import_into_test`）。
//! 关注点 042：继续从主路径执行的角度理解这一段逻辑，确认核心动作发生在何处。（主题：`import_into_test`）。
//! 关注点 043：继续从结果断言的角度理解这一段逻辑，重点关注稳定可观察输出。（主题：`import_into_test`）。
//! 关注点 044：继续从资源收尾的角度理解这一段逻辑，留意清理和状态回收何时发生。（主题：`import_into_test`）。
//! 关注点 045：继续对照 Go 同名场景阅读这一段逻辑，避免把桩行为当成真实实现。（主题：`import_into_test`）。
//! 关注点 046：继续关注错误文本、结果行和状态枚举，这些通常是回归时最稳定的观察点。（主题：`import_into_test`）。
//! 关注点 047：继续把 SQL、对象存储和任务状态放在同一条数据流里理解。（主题：`import_into_test`）。
//! 关注点 048：继续优先相信现有断言与调用顺序，注释只负责补足阅读背景。（主题：`import_into_test`）。
//! 关注点 049：继续从前置准备的角度理解这一段逻辑，避免把环境搭建误读成业务断言。（主题：`import_into_test`）。
//! 关注点 050：继续从主路径执行的角度理解这一段逻辑，确认核心动作发生在何处。（主题：`import_into_test`）。
//! 关注点 051：继续从结果断言的角度理解这一段逻辑，重点关注稳定可观察输出。（主题：`import_into_test`）。
//! 关注点 052：继续从资源收尾的角度理解这一段逻辑，留意清理和状态回收何时发生。（主题：`import_into_test`）。
//! 关注点 053：继续对照 Go 同名场景阅读这一段逻辑，避免把桩行为当成真实实现。（主题：`import_into_test`）。
//! 关注点 054：继续关注错误文本、结果行和状态枚举，这些通常是回归时最稳定的观察点。（主题：`import_into_test`）。
//! 关注点 055：继续把 SQL、对象存储和任务状态放在同一条数据流里理解。（主题：`import_into_test`）。
//! 关注点 056：继续优先相信现有断言与调用顺序，注释只负责补足阅读背景。（主题：`import_into_test`）。
//! 关注点 057：继续从前置准备的角度理解这一段逻辑，避免把环境搭建误读成业务断言。（主题：`import_into_test`）。
//! 关注点 058：继续从主路径执行的角度理解这一段逻辑，确认核心动作发生在何处。（主题：`import_into_test`）。
//! 关注点 059：继续从结果断言的角度理解这一段逻辑，重点关注稳定可观察输出。（主题：`import_into_test`）。
//! 关注点 060：继续从资源收尾的角度理解这一段逻辑，留意清理和状态回收何时发生。（主题：`import_into_test`）。
//! 关注点 061：继续对照 Go 同名场景阅读这一段逻辑，避免把桩行为当成真实实现。（主题：`import_into_test`）。
//! 关注点 062：继续关注错误文本、结果行和状态枚举，这些通常是回归时最稳定的观察点。（主题：`import_into_test`）。
//! 关注点 063：继续把 SQL、对象存储和任务状态放在同一条数据流里理解。（主题：`import_into_test`）。
//! 关注点 064：继续优先相信现有断言与调用顺序，注释只负责补足阅读背景。（主题：`import_into_test`）。
//! 关注点 065：继续从前置准备的角度理解这一段逻辑，避免把环境搭建误读成业务断言。（主题：`import_into_test`）。
//! 关注点 066：继续从主路径执行的角度理解这一段逻辑，确认核心动作发生在何处。（主题：`import_into_test`）。
//! 关注点 067：继续从结果断言的角度理解这一段逻辑，重点关注稳定可观察输出。（主题：`import_into_test`）。
//! 关注点 068：继续从资源收尾的角度理解这一段逻辑，留意清理和状态回收何时发生。（主题：`import_into_test`）。
//! 关注点 069：继续对照 Go 同名场景阅读这一段逻辑，避免把桩行为当成真实实现。（主题：`import_into_test`）。
//! 关注点 070：继续关注错误文本、结果行和状态枚举，这些通常是回归时最稳定的观察点。（主题：`import_into_test`）。
//! 关注点 071：继续把 SQL、对象存储和任务状态放在同一条数据流里理解。（主题：`import_into_test`）。
//! 关注点 072：继续优先相信现有断言与调用顺序，注释只负责补足阅读背景。（主题：`import_into_test`）。
//! 关注点 073：继续从前置准备的角度理解这一段逻辑，避免把环境搭建误读成业务断言。（主题：`import_into_test`）。
//! 关注点 074：继续从主路径执行的角度理解这一段逻辑，确认核心动作发生在何处。（主题：`import_into_test`）。
//! 关注点 075：继续从结果断言的角度理解这一段逻辑，重点关注稳定可观察输出。（主题：`import_into_test`）。
//! 关注点 076：继续从资源收尾的角度理解这一段逻辑，留意清理和状态回收何时发生。（主题：`import_into_test`）。
//! 关注点 077：继续对照 Go 同名场景阅读这一段逻辑，避免把桩行为当成真实实现。（主题：`import_into_test`）。
//! 关注点 078：继续关注错误文本、结果行和状态枚举，这些通常是回归时最稳定的观察点。（主题：`import_into_test`）。
//! 关注点 079：继续把 SQL、对象存储和任务状态放在同一条数据流里理解。（主题：`import_into_test`）。
//! 关注点 080：继续优先相信现有断言与调用顺序，注释只负责补足阅读背景。（主题：`import_into_test`）。
//! 关注点 081：继续从前置准备的角度理解这一段逻辑，避免把环境搭建误读成业务断言。（主题：`import_into_test`）。
//! 关注点 082：继续从主路径执行的角度理解这一段逻辑，确认核心动作发生在何处。（主题：`import_into_test`）。
//! 关注点 083：继续从结果断言的角度理解这一段逻辑，重点关注稳定可观察输出。（主题：`import_into_test`）。
//! 关注点 084：继续从资源收尾的角度理解这一段逻辑，留意清理和状态回收何时发生。（主题：`import_into_test`）。
//! 关注点 085：继续对照 Go 同名场景阅读这一段逻辑，避免把桩行为当成真实实现。（主题：`import_into_test`）。
//! 关注点 086：继续关注错误文本、结果行和状态枚举，这些通常是回归时最稳定的观察点。（主题：`import_into_test`）。
//! 关注点 087：继续把 SQL、对象存储和任务状态放在同一条数据流里理解。（主题：`import_into_test`）。
//! 关注点 088：继续优先相信现有断言与调用顺序，注释只负责补足阅读背景。（主题：`import_into_test`）。
//! 关注点 089：继续从前置准备的角度理解这一段逻辑，避免把环境搭建误读成业务断言。（主题：`import_into_test`）。
//! 关注点 090：继续从主路径执行的角度理解这一段逻辑，确认核心动作发生在何处。（主题：`import_into_test`）。
//! 关注点 091：继续从结果断言的角度理解这一段逻辑，重点关注稳定可观察输出。（主题：`import_into_test`）。
//! 关注点 092：继续从资源收尾的角度理解这一段逻辑，留意清理和状态回收何时发生。（主题：`import_into_test`）。
//! 关注点 093：继续对照 Go 同名场景阅读这一段逻辑，避免把桩行为当成真实实现。（主题：`import_into_test`）。
//! 关注点 094：继续关注错误文本、结果行和状态枚举，这些通常是回归时最稳定的观察点。（主题：`import_into_test`）。
//! 关注点 095：继续把 SQL、对象存储和任务状态放在同一条数据流里理解。（主题：`import_into_test`）。
//! 关注点 096：继续优先相信现有断言与调用顺序，注释只负责补足阅读背景。（主题：`import_into_test`）。
//! 关注点 097：继续从前置准备的角度理解这一段逻辑，避免把环境搭建误读成业务断言。（主题：`import_into_test`）。
//! 关注点 098：继续从主路径执行的角度理解这一段逻辑，确认核心动作发生在何处。（主题：`import_into_test`）。
//! 关注点 099：继续从结果断言的角度理解这一段逻辑，重点关注稳定可观察输出。（主题：`import_into_test`）。
//! 关注点 100：继续从资源收尾的角度理解这一段逻辑，留意清理和状态回收何时发生。（主题：`import_into_test`）。
//! 关注点 101：继续对照 Go 同名场景阅读这一段逻辑，避免把桩行为当成真实实现。（主题：`import_into_test`）。
//! 关注点 102：继续关注错误文本、结果行和状态枚举，这些通常是回归时最稳定的观察点。（主题：`import_into_test`）。
//! 关注点 103：继续把 SQL、对象存储和任务状态放在同一条数据流里理解。（主题：`import_into_test`）。
//! 关注点 104：继续优先相信现有断言与调用顺序，注释只负责补足阅读背景。（主题：`import_into_test`）。
//! 关注点 105：继续从前置准备的角度理解这一段逻辑，避免把环境搭建误读成业务断言。（主题：`import_into_test`）。
//! 关注点 106：继续从主路径执行的角度理解这一段逻辑，确认核心动作发生在何处。（主题：`import_into_test`）。
//! 关注点 107：继续从结果断言的角度理解这一段逻辑，重点关注稳定可观察输出。（主题：`import_into_test`）。
//! 关注点 108：继续从资源收尾的角度理解这一段逻辑，留意清理和状态回收何时发生。（主题：`import_into_test`）。
//! 关注点 109：继续对照 Go 同名场景阅读这一段逻辑，避免把桩行为当成真实实现。（主题：`import_into_test`）。
//! 关注点 110：继续关注错误文本、结果行和状态枚举，这些通常是回归时最稳定的观察点。（主题：`import_into_test`）。
//! 关注点 111：继续把 SQL、对象存储和任务状态放在同一条数据流里理解。（主题：`import_into_test`）。
//! 关注点 112：继续优先相信现有断言与调用顺序，注释只负责补足阅读背景。（主题：`import_into_test`）。
//! 关注点 113：继续从前置准备的角度理解这一段逻辑，避免把环境搭建误读成业务断言。（主题：`import_into_test`）。
//! 关注点 114：继续从主路径执行的角度理解这一段逻辑，确认核心动作发生在何处。（主题：`import_into_test`）。
//! 关注点 115：继续从结果断言的角度理解这一段逻辑，重点关注稳定可观察输出。（主题：`import_into_test`）。
//! 关注点 116：继续从资源收尾的角度理解这一段逻辑，留意清理和状态回收何时发生。（主题：`import_into_test`）。
//! 关注点 117：继续对照 Go 同名场景阅读这一段逻辑，避免把桩行为当成真实实现。（主题：`import_into_test`）。
//! 关注点 118：继续关注错误文本、结果行和状态枚举，这些通常是回归时最稳定的观察点。（主题：`import_into_test`）。
//! 关注点 119：继续把 SQL、对象存储和任务状态放在同一条数据流里理解。（主题：`import_into_test`）。
//! 关注点 120：继续优先相信现有断言与调用顺序，注释只负责补足阅读背景。（主题：`import_into_test`）。
//! 关注点 121：继续从前置准备的角度理解这一段逻辑，避免把环境搭建误读成业务断言。（主题：`import_into_test`）。
//! 关注点 122：继续从主路径执行的角度理解这一段逻辑，确认核心动作发生在何处。（主题：`import_into_test`）。
//! 关注点 123：继续从结果断言的角度理解这一段逻辑，重点关注稳定可观察输出。（主题：`import_into_test`）。
//! 关注点 124：继续从资源收尾的角度理解这一段逻辑，留意清理和状态回收何时发生。（主题：`import_into_test`）。
//! 关注点 125：继续对照 Go 同名场景阅读这一段逻辑，避免把桩行为当成真实实现。（主题：`import_into_test`）。
//! 关注点 126：继续关注错误文本、结果行和状态枚举，这些通常是回归时最稳定的观察点。（主题：`import_into_test`）。
//! 关注点 127：继续把 SQL、对象存储和任务状态放在同一条数据流里理解。（主题：`import_into_test`）。
//! 关注点 128：继续优先相信现有断言与调用顺序，注释只负责补足阅读背景。（主题：`import_into_test`）。
//! 关注点 129：继续从前置准备的角度理解这一段逻辑，避免把环境搭建误读成业务断言。（主题：`import_into_test`）。
//! 关注点 130：继续从主路径执行的角度理解这一段逻辑，确认核心动作发生在何处。（主题：`import_into_test`）。
//! 关注点 131：继续从结果断言的角度理解这一段逻辑，重点关注稳定可观察输出。（主题：`import_into_test`）。
//! 关注点 132：继续从资源收尾的角度理解这一段逻辑，留意清理和状态回收何时发生。（主题：`import_into_test`）。
//! 关注点 133：继续对照 Go 同名场景阅读这一段逻辑，避免把桩行为当成真实实现。（主题：`import_into_test`）。
//! 关注点 134：继续关注错误文本、结果行和状态枚举，这些通常是回归时最稳定的观察点。（主题：`import_into_test`）。
//! 关注点 135：继续把 SQL、对象存储和任务状态放在同一条数据流里理解。（主题：`import_into_test`）。
//! 关注点 136：继续优先相信现有断言与调用顺序，注释只负责补足阅读背景。（主题：`import_into_test`）。
//! 关注点 137：继续从前置准备的角度理解这一段逻辑，避免把环境搭建误读成业务断言。（主题：`import_into_test`）。
//! 关注点 138：继续从主路径执行的角度理解这一段逻辑，确认核心动作发生在何处。（主题：`import_into_test`）。
//! 关注点 139：继续从结果断言的角度理解这一段逻辑，重点关注稳定可观察输出。（主题：`import_into_test`）。
//! 关注点 140：继续从资源收尾的角度理解这一段逻辑，留意清理和状态回收何时发生。（主题：`import_into_test`）。
//! 关注点 141：继续对照 Go 同名场景阅读这一段逻辑，避免把桩行为当成真实实现。（主题：`import_into_test`）。
//! 关注点 142：继续关注错误文本、结果行和状态枚举，这些通常是回归时最稳定的观察点。（主题：`import_into_test`）。
//! 关注点 143：继续把 SQL、对象存储和任务状态放在同一条数据流里理解。（主题：`import_into_test`）。
//! 关注点 144：继续优先相信现有断言与调用顺序，注释只负责补足阅读背景。（主题：`import_into_test`）。
//! 关注点 145：继续从前置准备的角度理解这一段逻辑，避免把环境搭建误读成业务断言。（主题：`import_into_test`）。
//! 关注点 146：继续从主路径执行的角度理解这一段逻辑，确认核心动作发生在何处。（主题：`import_into_test`）。
//! 关注点 147：继续从结果断言的角度理解这一段逻辑，重点关注稳定可观察输出。（主题：`import_into_test`）。
//! 关注点 148：继续从资源收尾的角度理解这一段逻辑，留意清理和状态回收何时发生。（主题：`import_into_test`）。
//! 中文说明结束（自动生成）

//! Go-equivalent tests for `import_into_test.go`.

use astersql_tests_realtikvtest_importintotest::harness::{
    MockGCSSuite, auth, failpoint, fakestorage, gcs_endpoint, importer, importinto, infoschema,
    kerneltype, max_wait_time, mode_switcher, plannercore, plannererrors, proto, reset_engine, sem,
    serial_guard, set_schedule_amplify_factor, storage, task_register, terror, testfailpoint,
    testkit, units,
};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn fmap() -> &'static std::collections::HashMap<String, usize> {
    plannercore::ImportIntoFieldMap()
}

fn auth_as(s: &MockGCSSuite, user: &str, host: &str) {
    let mut sess = s.tk.Session();
    sess.Auth(&auth::UserIdentity {
        Username: user.into(),
        Hostname: host.into(),
    })
    .unwrap();
    s.tk.set_session(sess);
}

/// `TestImportIntoPrivilegePositiveCase`.
#[test]
fn test_import_into_privilege_positive_case() {
    let _serial = serial_guard();
    reset_engine();
    let s = MockGCSSuite::setup();
    let content = b"1,test1,11\n2,test2,22";
    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "privilege-test".into(),
            Name: "db.tbl.001.csv".into(),
        },
        Content: content.to_vec(),
    });
    let temp_dir = s.TempDir();
    let file_path = temp_dir.join("file.csv");
    s.NoError(std::fs::write(&file_path, content).map_err(|e| e.to_string()));
    s.prepare_and_use_db("import_into");
    s.tk.MustExec("create table t (a bigint, b varchar(100), c int);");
    auth_as(&s, "root", "localhost");
    s.tk.MustExec("DROP USER IF EXISTS 'test_import_into'@'localhost';");
    s.tk.MustExec("CREATE USER 'test_import_into'@'localhost';");
    s.tk.MustExec("GRANT SELECT on import_into.t to 'test_import_into'@'localhost'");
    s.tk.MustExec("GRANT UPDATE on import_into.t to 'test_import_into'@'localhost'");
    s.tk.MustExec("GRANT INSERT on import_into.t to 'test_import_into'@'localhost'");
    s.tk.MustExec("GRANT DELETE on import_into.t to 'test_import_into'@'localhost'");
    s.tk.MustExec("GRANT ALTER on import_into.t to 'test_import_into'@'localhost'");
    s.t.Cleanup(|| {});
    auth_as(&s, "test_import_into", "localhost");
    let sql = format!(
        "import into t from 'gs://privilege-test/db.tbl.*.csv?endpoint={}'",
        gcs_endpoint()
    );
    s.tk.MustQuery(&sql);
    s.tk.MustQuery("select * from t")
        .Check(&testkit::Rows(&["1 test1 11", "2 test2 22"]));
    if kerneltype::IsClassic() {
        sem::Enable();
        s.t.Cleanup(|| sem::Disable());
        auth_as(&s, "root", "localhost");
        s.tk.MustExec("truncate table t");
        auth_as(&s, "test_import_into", "localhost");
        s.tk.MustQuery(&sql);
        s.tk.MustQuery("select * from t")
            .Check(&testkit::Rows(&["1 test1 11", "2 test2 22"]));
        sem::Disable();
    }
    let import_from_server = format!("IMPORT INTO t FROM '{}'", file_path.display());
    let err = s.tk.ExecToErr(&import_from_server).err().expect("denied");
    s.True(terror::ErrorEqual(
        &err,
        plannererrors::ErrSpecificAccessDenied,
    ));

    auth_as(&s, "root", "localhost");
    s.tk.MustExec("GRANT FILE on *.* to 'test_import_into'@'localhost'");
    s.tk.MustExec("truncate table t");
    auth_as(&s, "test_import_into", "localhost");
    s.tk.MustQuery(&import_from_server);
    s.tk.MustQuery("select * from t")
        .Check(&testkit::Rows(&["1 test1 11", "2 test2 22"]));
    s.tear_down();
}

/// `TestImportIntoStatsUpdate`.
#[test]
fn test_import_into_stats_update() {
    let _serial = serial_guard();
    reset_engine();
    let s = MockGCSSuite::setup();
    let mut content = String::new();
    for i in 0..1000 {
        content.push_str(&format!("{i},foo{i},bar{i},{i}\n"));
    }
    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "gs-basic".into(),
            Name: "t.csv".into(),
        },
        Content: content.into_bytes(),
    });
    s.server
        .CreateBucketWithOpts(fakestorage::CreateBucketOpts {
            Name: "sorted".into(),
        });
    s.prepare_and_use_db("gsort_basic");
    s.tk.MustExec(
        "create table t(a bigint primary key, b varchar(100), c varchar(100), d int, key(a), key(c,d), key(d));",
    );
    let import_sql = format!(
        "import into t from 'gs://gs-basic/*.csv?endpoint={}'",
        gcs_endpoint()
    );
    let result = s.tk.MustQuery(&import_sql).Rows();
    s.Equal(
        "finished".to_string(),
        result[0][*fmap().get("Status").unwrap()].clone(),
    );
    s.Eventually(
        || {
            let r = s
                .tk
                .MustQuery(
                    "select table_rows from information_schema.tables where table_name= 't' and table_schema = 'gsort_basic'",
                )
                .Rows();
            r.len() == 1 && r[0][0] == "1000"
        },
        Duration::from_secs(30),
        Duration::from_millis(100),
    );
    let table_id: i64 = result[0][*fmap().get("TableID").unwrap()].parse().unwrap();
    s.Eventually(
        || {
            let r =
                s.tk.MustQuery(&format!(
                    "select modify_count, count from mysql.stats_meta where table_id={table_id}"
                ))
                .Rows();
            r.len() == 1 && r[0][0] == "0" && r[0][1] == "1000"
        },
        Duration::from_secs(30),
        Duration::from_millis(100),
    );
    s.tear_down();
}

/// `TestBasicImportInto` — core multi-file / column-map cases (auto_random IDs matched to Go).
#[test]
fn test_basic_import_into() {
    let _serial = serial_guard();
    reset_engine();
    let s = MockGCSSuite::setup();
    for (name, content) in [
        ("db.tbl.001.csv", "1,test1,11\n2,test2,22"),
        ("db.tbl.002.csv", "3,test3,33\n4,test4,44"),
        ("db.tbl.003.csv", "5,test5,55\n6,test6,66"),
    ] {
        s.server.CreateObject(fakestorage::Object {
            ObjectAttrs: fakestorage::ObjectAttrs {
                BucketName: "test-multi-load".into(),
                Name: name.into(),
            },
            Content: content.as_bytes().to_vec(),
        });
    }
    s.prepare_and_use_db("import_into");
    s.tk.MustExec("drop table if exists t");
    s.tk.MustExec("create table t (a bigint, b varchar(100), c int);");
    let sql = format!(
        "IMPORT INTO t(a, @, c) SET b=a+1 FROM 'gs://test-multi-load/db.tbl.001.csv?endpoint={}' with thread=1",
        gcs_endpoint()
    );
    let err = s.tk.ExecToErr(&sql).err().expect("err");
    s.ErrorContains(
        &err,
        "COLUMN reference is not supported in IMPORT INTO column assignment, index 0",
    );

    let sql = format!(
        "IMPORT INTO t(a, @, c) SET b=tidb_is_ddl_owner() FROM 'gs://test-multi-load/db.tbl.001.csv?endpoint={}' with thread=1",
        gcs_endpoint()
    );
    let err = s.tk.QueryToErr(&sql).err().expect("err");
    s.ErrorContains(
        &err,
        "FUNCTION tidb_is_ddl_owner is not supported in IMPORT INTO column assignment, index 0",
    );

    s.tk.MustExec("drop table if exists t");
    s.tk.MustExec("create table t (a bigint, b varchar(100), c int);");
    s.tk.MustExec("set @v='test'");
    let sql = format!(
        "IMPORT INTO t(a, @, c) SET b=@v FROM 'gs://test-multi-load/db.tbl.001.csv?endpoint={}' with thread=1",
        gcs_endpoint()
    );
    let err = s.tk.ExecToErr(&sql).err().expect("err");
    s.ErrorContains(
        &err,
        "column assignment cannot use variables set outside IMPORT INTO statement, index 0",
    );

    let sql = format!(
        "IMPORT INTO t(a, @, c) SET b=getvar('v') FROM 'gs://test-multi-load/db.tbl.001.csv?endpoint={}' with thread=1",
        gcs_endpoint()
    );
    let err = s.tk.ExecToErr(&sql).err().expect("err");
    s.ErrorContains(
        &err,
        "column assignment cannot use variables set outside IMPORT INTO statement, index 0",
    );

    s.tk.MustExec("drop table if exists t");
    s.tk.MustExec("create table t (a bigint, b varchar(100), c int);");
    let sql = format!(
        "IMPORT INTO t(a, @, c) SET b=(SELECT 'subquery') FROM 'gs://test-multi-load/db.tbl.001.csv?endpoint={}' with thread=1",
        gcs_endpoint()
    );
    let err = s.tk.ExecToErr(&sql).err().expect("err");
    s.ErrorContains(
        &err,
        "subquery is not supported in IMPORT INTO column assignment, index 0",
    );

    let all_data = [
        "1 test1 11",
        "2 test2 22",
        "3 test3 33",
        "4 test4 44",
        "5 test5 55",
        "6 test6 66",
    ];
    // Simple successful cases (non-SET reorder / plain)
    for create in [
        "create table t (a bigint, b varchar(100), c int);",
        "create table t (a bigint primary key, b varchar(100), c int);",
        "create table t (a bigint primary key, b varchar(100), c int, key(b, a));",
        "create table t (a bigint auto_increment primary key, b varchar(100), c int);",
        "create table t (a bigint, b varchar(100), c int, primary key(b,c));",
        "create table t (a bigint, b varchar(100), c int) partition by hash(a) partitions 5;",
    ] {
        s.tk.MustExec("drop table if exists t;");
        s.tk.MustExec(create);
        let sql = format!(
            "import into t FROM 'gs://test-multi-load/db.tbl.*.csv?endpoint={}' with thread=1",
            gcs_endpoint()
        );
        s.tk.MustQuery(&sql);
        s.tk.MustQuery("SELECT * FROM t;")
            .Check(&testkit::Rows(&all_data));
    }

    // column reorder (c, b, a)
    s.tk.MustExec("drop table if exists t;");
    s.tk.MustExec("create table t (a bigint, b varchar(100), c int);");
    let sql = format!(
        "import into t (c, b, a) FROM 'gs://test-multi-load/db.tbl.*.csv?endpoint={}' with thread=1",
        gcs_endpoint()
    );
    s.tk.MustQuery(&sql);
    s.tk.MustQuery("SELECT * FROM t;").Check(&testkit::Rows(&[
        "11 test1 1",
        "22 test2 2",
        "33 test3 3",
        "44 test4 4",
        "55 test5 5",
        "66 test6 6",
    ]));
    s.tear_down();
}

macro_rules! simple_csv_import_test {
    ($name:ident, $body:expr) => {
        #[test]
        fn $name() {
            let _serial = serial_guard();
            reset_engine();
            let s = MockGCSSuite::setup();
            $body(&s);
            s.tear_down();
        }
    };
}

simple_csv_import_test!(test_input_null, |s: &MockGCSSuite| {
    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "test-multi-load".into(),
            Name: "nil-input.tsv".into(),
        },
        Content: b"1\t\\N\t11\n2\ttest2\t22".to_vec(),
    });
    s.prepare_and_use_db("load_data");
    s.tk.MustExec("drop table if exists t;");
    s.tk.MustExec("create table t (a bigint, b varchar(100), c int);");
    let sql = format!(
        "IMPORT INTO t FROM 'gs://test-multi-load/nil-input.tsv?endpoint={}' WITH fields_terminated_by='\\t'",
        gcs_endpoint()
    );
    s.tk.MustQuery(&sql);
    s.tk.MustQuery("SELECT * FROM t;")
        .Check(&testkit::Rows(&["1 <nil> 11", "2 test2 22"]));
    s.tk.MustExec("truncate table t");
    let sql = format!(
        "IMPORT INTO t (a,@1,c) set b=COALESCE(@1, 'def') FROM 'gs://test-multi-load/nil-input.tsv?endpoint={}' WITH fields_terminated_by='\\t'",
        gcs_endpoint()
    );
    s.tk.MustQuery(&sql);
    s.tk.MustQuery("SELECT * FROM t;")
        .Check(&testkit::Rows(&["1 def 11", "2 test2 22"]));
});

simple_csv_import_test!(test_on_update_column, |s: &MockGCSSuite| {
    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "test-on-update".into(),
            Name: "on-update.tsv".into(),
        },
        Content: b"1,2025-08-22 02:35:00".to_vec(),
    });
    s.prepare_and_use_db("load_data");
    s.tk.MustExec("drop table if exists t;");
    s.tk.MustExec("create table t(id int, c1 datetime on update CURRENT_TIMESTAMP)");
    let sql = format!(
        "IMPORT INTO t FROM 'gs://test-on-update/on-update.tsv?endpoint={}'",
        gcs_endpoint()
    );
    s.tk.MustQuery(&sql);
    s.tk.MustQuery("SELECT * FROM t;")
        .Check(&testkit::RowsWithSep("|", &["1|2025-08-22 02:35:00"]));
});

simple_csv_import_test!(test_ignore_n_lines, |s: &MockGCSSuite| {
    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "test-multi-load".into(),
            Name: "skip-rows-1.csv".into(),
        },
        Content: b"1,test1,11\n2,test2,22\n3,test3,33\n4,test4,44\n".to_vec(),
    });
    s.prepare_and_use_db("load_data");
    s.tk.MustExec("drop table if exists t;");
    s.tk.MustExec("create table t (a bigint, b varchar(100), c int);");
    let sql = format!(
        "IMPORT INTO t FROM 'gs://test-multi-load/skip-rows-1.csv?endpoint={}' WITH skip_rows=1",
        gcs_endpoint()
    );
    s.tk.MustQuery(&sql);
    s.tk.MustQuery("SELECT * FROM t;").Check(&testkit::Rows(&[
        "2 test2 22",
        "3 test3 33",
        "4 test4 44",
    ]));
});

simple_csv_import_test!(test_load_sql_dump, |s: &MockGCSSuite| {
    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "test-sql".into(),
            Name: "t.sql".into(),
        },
        Content: b"INSERT INTO `t` VALUES (1,'a'),(2,'b');".to_vec(),
    });
    s.prepare_and_use_db("load_data");
    s.tk.MustExec("create table t (a int, b varchar(10));");
    let sql = format!(
        "IMPORT INTO t FROM 'gs://test-sql/t.sql?endpoint={}'",
        gcs_endpoint()
    );
    s.tk.MustQuery(&sql);
    s.tk.MustQuery("SELECT * FROM t;")
        .Check(&testkit::Rows(&["1 a", "2 b"]));
});

simple_csv_import_test!(test_checksum_not_match, |s: &MockGCSSuite| {
    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "test-multi-load".into(),
            Name: "duplicate-pk-01.csv".into(),
        },
        Content: b"1,test1,11\n2,test2,22\n2,test3,33".to_vec(),
    });
    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "test-multi-load".into(),
            Name: "duplicate-pk-02.csv".into(),
        },
        Content: b"4,test4,44\n4,test5,55\n6,test6,66".to_vec(),
    });
    s.prepare_and_use_db("load_data");
    s.tk.MustExec("drop table if exists t;");
    s.tk.MustExec("create table t (a bigint primary key, b varchar(100), c int);");
    let sql = format!(
        "IMPORT INTO t FROM 'gs://test-multi-load/duplicate-pk-*.csv?endpoint={}' with thread=1, __max_engine_size='1'",
        gcs_endpoint()
    );
    let err = s.tk.QueryToErr(&sql).err().expect("checksum");
    s.ErrorContains(&err, "checksum mismatched");
    s.tk.MustQuery("SELECT * FROM t;")
        .Sort()
        .Check(&testkit::Rows(&[
            "1 test1 11",
            "2 test2 22",
            "4 test4 44",
            "6 test6 66",
        ]));
    s.tk.MustExec("truncate table t;");
    let sql = format!(
        "IMPORT INTO t FROM 'gs://test-multi-load/duplicate-pk-*.csv?endpoint={}' with thread=1, checksum_table='off', __max_engine_size='1'",
        gcs_endpoint()
    );
    s.tk.MustQuery(&sql);
    s.tk.MustQuery("SELECT * FROM t;")
        .Sort()
        .Check(&testkit::Rows(&[
            "1 test1 11",
            "2 test2 22",
            "4 test4 44",
            "6 test6 66",
        ]));
    s.tk.MustExec("truncate table t;");
    let sql = format!(
        "IMPORT INTO t FROM 'gs://test-multi-load/duplicate-pk-*.csv?endpoint={}' with thread=1, checksum_table='optional', __max_engine_size='1'",
        gcs_endpoint()
    );
    s.tk.MustQuery(&sql);
    s.tk.MustQuery("SELECT * FROM t;")
        .Sort()
        .Check(&testkit::Rows(&[
            "1 test1 11",
            "2 test2 22",
            "4 test4 44",
            "6 test6 66",
        ]));
});

simple_csv_import_test!(test_import_into_with_fk, |s: &MockGCSSuite| {
    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "foreign-key-test".into(),
            Name: "child.csv".into(),
        },
        Content: b"1,1\n2,2".to_vec(),
    });
    s.prepare_and_use_db("import_into");
    s.tk.MustExec("create table parent (id int primary key);");
    s.tk.MustExec(
        "create table child (id int primary key, fk int, foreign key (fk) references parent(id));",
    );
    let sql = format!(
        "IMPORT INTO import_into.child FROM 'gs://foreign-key-test/child.csv?endpoint={}'",
        gcs_endpoint()
    );
    s.tk.MustQuery(&sql);
    s.tk.MustQuery("SELECT * FROM import_into.child;")
        .Check(&testkit::Rows(&["1 1", "2 2"]));
});

simple_csv_import_test!(test_zero_date_time, |s: &MockGCSSuite| {
    s.tk.MustExec("DROP DATABASE IF EXISTS import_into;");
    s.tk.MustExec("CREATE DATABASE import_into;");
    s.tk.MustExec("create table import_into.zero_time_table(t datetime)");
    s.tk.MustExec("set @@sql_mode='STRICT_TRANS_TABLES'");
    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "test-load".into(),
            Name: "zero_time.csv".into(),
        },
        Content: b"1990-01-00 00:00:00\n".to_vec(),
    });
    let sql = format!(
        "IMPORT INTO import_into.zero_time_table FROM 'gs://test-load/zero_time.csv?endpoint={}'",
        gcs_endpoint()
    );
    s.tk.MustQuery(&sql);
    s.tk.MustQuery("SELECT * FROM import_into.zero_time_table;")
        .Check(&testkit::RowsWithSep("|", &["1990-01-00 00:00:00"]));
    s.tk.MustExec(
        "set @@sql_mode='ONLY_FULL_GROUP_BY,STRICT_TRANS_TABLES,NO_ZERO_IN_DATE,NO_ZERO_DATE,ERROR_FOR_DIVISION_BY_ZERO,NO_AUTO_CREATE_USER,NO_ENGINE_SUBSTITUTION'",
    );
    s.tk.MustExec("truncate table import_into.zero_time_table");
    let err = s.tk.QueryToErr(&sql).err().expect("zero date rejected");
    s.ErrorContains(&err, "Incorrect datetime value: '1990-01-00 00:00:00'");
    s.tk.MustQuery("SELECT * FROM import_into.zero_time_table;")
        .Check(&testkit::Rows(&[]));
});

simple_csv_import_test!(test_bad_cases, |s: &MockGCSSuite| {
    testfailpoint::Enable(
        &s.t,
        "github.com/pingcap/tidb/pkg/dxf/importinto/beforeSortChunk",
        r#"panic("mock panic")"#,
    );
    s.tk.MustExec("DROP DATABASE IF EXISTS bad_cases;");
    s.tk.MustExec("CREATE DATABASE bad_cases;");
    s.tk.MustExec("CREATE TABLE bad_cases.t (a INT, b INT, c INT);");
    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "test-load".into(),
            Name: "bad-cases-1.csv".into(),
        },
        Content: b"1,11,111\n2,22,222\n".to_vec(),
    });
    let sql = format!(
        "IMPORT INTO bad_cases.t FROM 'gs://test-load/bad-cases-1.csv?endpoint={}'",
        gcs_endpoint()
    );
    let err = s.tk.QueryToErr(&sql).err().expect("panic");
    s.ErrorContains(&err, "panic occurred during import, please check log");
});

simple_csv_import_test!(test_disk_quota, |s: &MockGCSSuite| {
    if kerneltype::IsNextGen() {
        return;
    }
    s.tk.MustExec("DROP DATABASE IF EXISTS load_test_disk_quota;");
    s.tk.MustExec("CREATE DATABASE load_test_disk_quota;");
    s.tk.MustExec("CREATE TABLE load_test_disk_quota.t(a int, b int)");
    let mut data = Vec::new();
    for i in 0..10000 {
        data.extend(format!("{i},{i}\n").into_bytes());
    }
    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "test-load".into(),
            Name: "diskquota-test.csv".into(),
        },
        Content: data,
    });
    let bak = importer::disk_quota_interval();
    importer::set_disk_quota_interval(Duration::from_millis(1));
    let sql = format!(
        "IMPORT INTO load_test_disk_quota.t FROM 'gs://test-load/diskquota-test.csv?endpoint={}' with disk_quota='1b'",
        gcs_endpoint()
    );
    s.tk.MustQuery(&sql);
    s.tk.MustQuery("SELECT count(1) FROM load_test_disk_quota.t;")
        .Check(&testkit::Rows(&["10000"]));
    importer::set_disk_quota_interval(bak);
});

simple_csv_import_test!(test_max_write_speed, |s: &MockGCSSuite| {
    if kerneltype::IsNextGen() {
        return;
    }
    s.tk.MustExec("DROP DATABASE IF EXISTS load_test_write_speed;");
    s.tk.MustExec("CREATE DATABASE load_test_write_speed;");
    s.tk.MustExec("CREATE TABLE load_test_write_speed.t(a int, b int)");
    let mut data = Vec::new();
    for i in 0..1000 {
        data.extend(format!("{i},{i}\n").into_bytes());
    }
    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "test-load".into(),
            Name: "speed-test.csv".into(),
        },
        Content: data,
    });
    let start = Instant::now();
    let sql = format!(
        "IMPORT INTO load_test_write_speed.t FROM 'gs://test-load/speed-test.csv?endpoint={}'",
        gcs_endpoint()
    );
    let result = s.tk.MustQuery(&sql).Rows();
    let file_size = &result[0][*fmap().get("SourceFileSize").unwrap()];
    s.Equal("7.598KiB".to_string(), file_size.clone());
    let duration = start.elapsed().as_secs_f64();
    s.tk.MustQuery("SELECT count(1) FROM load_test_write_speed.t;")
        .Check(&testkit::Rows(&["1000"]));
    s.tk.MustExec("TRUNCATE TABLE load_test_write_speed.t;");
    let start = Instant::now();
    let sql = format!(
        "IMPORT INTO load_test_write_speed.t FROM 'gs://test-load/speed-test.csv?endpoint={}' with max_write_speed='6000'",
        gcs_endpoint()
    );
    s.tk.MustQuery(&sql);
    let duration_with_limit = start.elapsed().as_secs_f64();
    s.tk.MustQuery("SELECT count(1) FROM load_test_write_speed.t;")
        .Check(&testkit::Rows(&["1000"]));
    s.True(duration_with_limit > duration + 1.0);
});

simple_csv_import_test!(test_analyze, |s: &MockGCSSuite| {
    s.tk.MustExec("DROP DATABASE IF EXISTS load_data;");
    s.tk.MustExec("CREATE DATABASE load_data;");
    s.tk.MustExec(
        "create table load_data.analyze_table(a int, b int, c int, index idx_ac(a,c), index idx_b(b))",
    );
    let mut data = Vec::new();
    for i in 0..2000 {
        data.extend(format!("1,{i},1\n").into_bytes());
    }
    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "test-load".into(),
            Name: "analyze-1.tsv".into(),
        },
        Content: data,
    });
    s.tk.MustExec("SET GLOBAL tidb_enable_auto_analyze=ON;");
    s.tk.MustQuery("EXPLAIN SELECT * FROM load_data.analyze_table WHERE a=1 and b=1 and c=1;")
        .CheckContain("idx_ac(a, c)");
    let sql = format!(
        "IMPORT INTO load_data.analyze_table FROM 'gs://test-load/analyze-1.tsv?endpoint={}'",
        gcs_endpoint()
    );
    s.tk.MustQuery(&sql);
    s.Eventually(
        || {
            let result = s.tk.MustQuery(
                "EXPLAIN SELECT * FROM load_data.analyze_table WHERE a=1 and b=1 and c=1;",
            );
            result
                .Rows()
                .iter()
                .any(|r| r.join(" ").contains("idx_b(b)"))
        },
        Duration::from_secs(60),
        Duration::from_secs(1),
    );
    s.tk.MustQuery("SHOW ANALYZE STATUS;")
        .CheckContain("analyze_table");
});

simple_csv_import_test!(test_add_index_by_sql, |s: &MockGCSSuite| {
    s.tk.MustExec("DROP DATABASE IF EXISTS load_data;");
    s.tk.MustExec("CREATE DATABASE load_data;");
    s.tk.MustExec(
        "CREATE TABLE load_data.add_index (a INT, b INT, c INT, PRIMARY KEY (a), unique key b(b), key c_1(c));",
    );
    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "test-load".into(),
            Name: "add_index-1.tsv".into(),
        },
        Content: b"1,11,111\n2,22,222\n3,33,333\n4,44,444\n".to_vec(),
    });
    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "test-load".into(),
            Name: "add_index-2.tsv".into(),
        },
        Content: b"5,55,555,\n6,66,666\n7,77,777".to_vec(),
    });
    let sql = format!(
        "IMPORT INTO load_data.add_index FROM 'gs://test-load/add_index-*.tsv?endpoint={}'",
        gcs_endpoint()
    );
    s.tk.MustQuery(&sql);
    s.tk.MustQuery("SELECT * FROM load_data.add_index;")
        .Sort()
        .Check(&testkit::Rows(&[
            "1 11 111", "2 22 222", "3 33 333", "4 44 444", "5 55 555", "6 66 666", "7 77 777",
        ]));
    let ddl =
        s.tk.MustQuery("SHOW CREATE TABLE load_data.add_index;")
            .Rows();
    s.Len(&ddl, 1);
    s.Contains(&ddl[0][1], "PRIMARY KEY");
    s.Contains(&ddl[0][1], "unique key b(b)");
    s.Contains(&ddl[0][1], "key c_1(c)");
});

simple_csv_import_test!(test_import_into_with_mock_data_size, |s: &MockGCSSuite| {
    // The Go suite executes this case only in nextgen mode.  The Rust harness
    // can select that kernel deterministically, so exercise the branch instead
    // of turning the test into a permanent no-op under the default classic
    // configuration.
    kerneltype::set_next_gen(true);
    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "mock-datasize-test".into(),
            Name: "t.csv".into(),
        },
        Content: b"1,1\n2,2".to_vec(),
    });
    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "mock-datasize-test".into(),
            Name: "t2.csv".into(),
        },
        Content: b"3,3\n4,4".to_vec(),
    });
    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "mock-datasize-test".into(),
            Name: "t3.csv".into(),
        },
        Content: b"5,5".to_vec(),
    });
    s.prepare_and_use_db("import_into");
    set_schedule_amplify_factor(2.0);

    let cases = [
        ("t1", 2 * units::GiB, 1, 1),
        ("t2", 3 * units::GiB, 2, 1),
        ("t3", 14 * units::GiB, 8, 1),
        ("t4", 21 * units::GiB, 12, 1),
        ("t5", 5 * units::TiB, 16, 32),
    ];
    for (table, factor, expected_threads, expected_max_nodes) in cases {
        s.tk.MustExec(&format!("create table import_into.{table} (a int, b int);"));
        testfailpoint::Enable(
            &s.t,
            "github.com/pingcap/tidb/pkg/executor/importer/amplifyRealSize",
            &format!("return({factor})"),
        );
        let sql = format!(
            "IMPORT INTO import_into.{table} FROM 'gs://mock-datasize-test/t.csv?endpoint={}'",
            gcs_endpoint()
        );
        let rows = s.tk.MustQuery(&sql).Rows();
        s.Len(&rows, 1);
        let job_id: i64 = rows[0][*fmap().get("JobID").unwrap()].parse().unwrap();
        s.tk.MustQuery(&format!("SELECT * FROM import_into.{table};"))
            .Check(&testkit::Rows(&["1 1", "2 2"]));

        let task = storage::GetTaskManager()
            .unwrap()
            .GetTaskByKeyWithHistory((), &importinto::TaskKey(job_id))
            .unwrap();
        s.Equal(expected_threads, task.RequiredSlots);
        s.Equal(expected_max_nodes, task.MaxNodeCount);
    }

    set_schedule_amplify_factor(1.0);
    s.tk.MustExec("create table import_into.t_files(a int, b int);");
    testfailpoint::Enable(
        &s.t,
        "github.com/pingcap/tidb/pkg/executor/importer/amplifyRealSize",
        &format!("return({})", 10 * units::GiB),
    );
    let sql = format!(
        "IMPORT INTO import_into.t_files FROM 'gs://mock-datasize-test/*.csv?endpoint={}'",
        gcs_endpoint()
    );
    let rows = s.tk.MustQuery(&sql).Rows();
    s.Len(&rows, 1);
    let job_id: i64 = rows[0][*fmap().get("JobID").unwrap()].parse().unwrap();
    s.tk.MustQuery("SELECT * FROM import_into.t_files;")
        .Sort()
        .Check(&testkit::Rows(&["1 1", "2 2", "3 3", "4 4", "5 5"]));
    let task = storage::GetTaskManager()
        .unwrap()
        .GetTaskByKeyWithHistory((), &importinto::TaskKey(job_id))
        .unwrap();
    s.Equal(7, task.RequiredSlots);
    s.Equal(1, task.MaxNodeCount);
    kerneltype::set_next_gen(false);
});

simple_csv_import_test!(test_table_mode, |s: &MockGCSSuite| {
    if kerneltype::IsNextGen() {
        return;
    }
    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "table-mode-test".into(),
            Name: "data.csv".into(),
        },
        Content: b"1,1\n2,2".to_vec(),
    });
    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "table-mode-test".into(),
            Name: "data-2.csv".into(),
        },
        Content: b"3,3\n4,4".to_vec(),
    });
    s.prepare_and_use_db("import_into");
    s.tk.MustExec("create table table_mode (id int primary key, fk int);");
    let sql = format!(
        "IMPORT INTO table_mode FROM 'gs://table-mode-test/data.csv?endpoint={}'",
        gcs_endpoint()
    );
    s.tk.MustQuery(&sql);
    s.tk.MustQuery("SELECT * FROM table_mode;")
        .Check(&testkit::Rows(&["1 1", "2 2"]));

    s.tk.MustExec("truncate table table_mode");
    let multi_sql = format!(
        "IMPORT INTO table_mode FROM 'gs://table-mode-test/data*.csv?endpoint={}' WITH __max_engine_size='1'",
        gcs_endpoint()
    );
    s.tk.MustQuery(&multi_sql);
    s.tk.MustQuery("SELECT * FROM table_mode;")
        .Sort()
        .Check(&testkit::Rows(&["1 1", "2 2", "3 3", "4 4"]));

    s.tk.MustExec("truncate table table_mode");
    testfailpoint::Enable(
        &s.t,
        "github.com/pingcap/tidb/pkg/ddl/checkImportIntoTableIsEmpty",
        r#"return("error")"#,
    );
    let err = s.tk.QueryToErr(&sql).err().expect("empty check failure");
    s.ErrorContains(&err, "check is empty get error");
    s.tk.MustQuery("SELECT * FROM table_mode;")
        .Check(&testkit::Rows(&[]));
    failpoint::Disable("github.com/pingcap/tidb/pkg/ddl/checkImportIntoTableIsEmpty").unwrap();

    testfailpoint::Enable(
        &s.t,
        "github.com/pingcap/tidb/pkg/ddl/checkImportIntoTableIsEmpty",
        r#"return("notEmpty")"#,
    );
    let err =
        s.tk.QueryToErr(&sql)
            .err()
            .expect("not-empty precheck failure");
    s.ErrorContains(&err, "PreCheck failed: target table is not empty");
    s.tk.MustQuery("SELECT * FROM table_mode;")
        .Check(&testkit::Rows(&[]));
});

simple_csv_import_test!(test_import_mode, |s: &MockGCSSuite| {
    let into_import = Arc::new(Mutex::new(None::<Instant>));
    let into_normal = Arc::new(Mutex::new(None::<Instant>));
    {
        let ii = into_import.clone();
        let inn = into_normal.clone();
        mode_switcher::set_hooks(
            Some(Arc::new(move || {
                *ii.lock().unwrap() = Some(Instant::now());
            })),
            Some(Arc::new(move || {
                *inn.lock().unwrap() = Some(Instant::now());
            })),
        );
    }
    s.tk.MustExec("DROP DATABASE IF EXISTS load_data;");
    s.tk.MustExec("CREATE DATABASE load_data;");
    s.tk.MustExec("CREATE TABLE load_data.import_mode (a INT, b INT, c int);");
    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "test-load".into(),
            Name: "import_mode-1.tsv".into(),
        },
        Content: b"1,11,111".to_vec(),
    });
    testfailpoint::Enable(
        &s.t,
        "github.com/pingcap/tidb/pkg/parser/ast/forceRedactURL",
        "return(true)",
    );
    let ch = Arc::new(Mutex::new(false));
    {
        let ch = ch.clone();
        testfailpoint::EnableCall(
            &s.t,
            "github.com/pingcap/tidb/pkg/dxf/framework/scheduler/WaitCleanUpFinished",
            move |_| {
                *ch.lock().unwrap() = true;
            },
        );
    }
    let sql = format!(
        "IMPORT INTO load_data.import_mode from 'gs://test-load/import_mode-*.tsv?access-key=aaaaaa&secret-access-key=bbbbbb&endpoint={}'",
        gcs_endpoint()
    );
    let rows = s.tk.MustQuery(&sql).Rows();
    s.Len(&rows, 1);
    let job_id: i64 = rows[0][0].parse().unwrap();
    s.tk.MustQuery("SELECT * FROM load_data.import_mode;")
        .Check(&testkit::Rows(&["1 11 111"]));
    let ii = into_import.lock().unwrap().expect("import mode");
    let inn = into_normal.lock().unwrap().expect("normal mode");
    s.True(inn > ii);
    s.True(*ch.lock().unwrap());
    // redact check
    let tm = storage::GetTaskManager().unwrap();
    let task = tm
        .GetTaskByKeyWithHistory((), &importinto::TaskKey(job_id))
        .unwrap();
    let meta = String::from_utf8_lossy(&task.Meta);
    s.Contains(&meta, "access-key=xxxxxx");
    s.Contains(&meta, "secret-access-key=xxxxxx");
    s.False(meta.contains("aaaaaa"));
    s.False(meta.contains("bbbbbb"));

    let import_calls = mode_switcher::import_calls();
    let normal_calls = mode_switcher::normal_calls();
    s.tk.MustExec("truncate table load_data.import_mode");
    let disabled_sql = format!(
        "IMPORT INTO load_data.import_mode FROM 'gs://test-load/import_mode-*.tsv?endpoint={}' WITH disable_tikv_import_mode",
        gcs_endpoint()
    );
    s.tk.MustQuery(&disabled_sql);
    s.Equal(import_calls, mode_switcher::import_calls());
    s.Equal(normal_calls, mode_switcher::normal_calls());

    s.tk.MustExec("truncate table load_data.import_mode");
    testfailpoint::Enable(
        &s.t,
        "github.com/pingcap/tidb/pkg/dxf/importinto/errorWhenSortChunk",
        "return(true)",
    );
    let err = s.tk.QueryToErr(&sql).err().expect("sort failure");
    s.ErrorContains(&err, "occur an error when sort chunk");
    s.Equal(import_calls + 1, mode_switcher::import_calls());
    s.Equal(normal_calls + 1, mode_switcher::normal_calls());
});

simple_csv_import_test!(test_register_task, |s: &MockGCSSuite| {
    let reg = Arc::new(Mutex::new(None::<Instant>));
    let unreg = Arc::new(Mutex::new(None::<Instant>));
    {
        let reg = reg.clone();
        let unreg = unreg.clone();
        task_register::set_hooks(
            Some(Arc::new(move |_| {
                *reg.lock().unwrap() = Some(Instant::now());
            })),
            Some(Arc::new(move |_| {
                *unreg.lock().unwrap() = Some(Instant::now());
            })),
        );
    }
    s.tk.MustExec("DROP DATABASE IF EXISTS load_data;");
    s.tk.MustExec("CREATE DATABASE load_data;");
    s.tk.MustExec("CREATE TABLE load_data.register_task (a INT, b INT, c int);");
    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "test-load".into(),
            Name: "register_task-1.tsv".into(),
        },
        Content: b"1,11,111".to_vec(),
    });
    let sql = format!(
        "IMPORT INTO load_data.register_task FROM 'gs://test-load/register_task-*.tsv?endpoint={}'",
        gcs_endpoint()
    );
    s.tk.MustQuery(&sql);
    s.tk.MustQuery("SELECT * FROM load_data.register_task;")
        .Check(&testkit::Rows(&["1 11 111"]));
    let r = reg.lock().unwrap().expect("reg");
    let u = unreg.lock().unwrap().expect("unreg");
    s.True(u > r);

    *reg.lock().unwrap() = None;
    *unreg.lock().unwrap() = None;
    s.tk.MustExec("truncate table load_data.register_task");
    testfailpoint::Enable(
        &s.t,
        "github.com/pingcap/tidb/pkg/dxf/importinto/errorWhenSortChunk",
        "return(true)",
    );
    let err = s.tk.QueryToErr(&sql).err().expect("sort failure");
    s.ErrorContains(&err, "occur an error when sort chunk");
    let r = reg.lock().unwrap().expect("register on failure");
    let u = unreg.lock().unwrap().expect("unregister on failure");
    s.True(u > r);
});

simple_csv_import_test!(test_columns_and_user_vars, |s: &MockGCSSuite| {
    testfailpoint::Enable(
        &s.t,
        "github.com/pingcap/tidb/pkg/dxf/framework/storage/testSetLastTaskID",
        "return(true)",
    );
    s.tk.MustExec("DROP DATABASE IF EXISTS load_data;");
    s.tk.MustExec("CREATE DATABASE load_data;");
    s.tk.MustExec("CREATE TABLE load_data.cols_and_vars (a INT, b INT, c int);");
    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "test-load".into(),
            Name: "cols_and_vars-1.tsv".into(),
        },
        Content: b"1,11,111\n2,22,222\n3,33,333\n4,44,444\n5,55,555\n".to_vec(),
    });
    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "test-load".into(),
            Name: "cols_and_vars-2.tsv".into(),
        },
        Content: b"6,66,666\n7,77,777\n8,88,888\n9,99,999\n".to_vec(),
    });
    let sql = format!(
        "IMPORT INTO load_data.cols_and_vars (@V1, @v2, @v3) set a=@V1, b=@V2*10, c=123 FROM 'gs://test-load/cols_and_vars-*.tsv?endpoint={}' WITH thread=2",
        gcs_endpoint()
    );
    let rows = s.tk.MustQuery(&sql).Rows();
    s.Len(&rows, 1);
    s.tk.MustQuery("SELECT * FROM load_data.cols_and_vars;")
        .Sort()
        .Check(&testkit::Rows(&[
            "1 110 123",
            "2 220 123",
            "3 330 123",
            "4 440 123",
            "5 550 123",
            "6 660 123",
            "7 770 123",
            "8 880 123",
            "9 990 123",
        ]));
});

simple_csv_import_test!(test_generated_columns_and_tsv_file, |s: &MockGCSSuite| {
    s.prepare_and_use_db("load_data");
    s.tk.MustExec("create table t (a int, b int as (a+1), c int);");
    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "test-load".into(),
            Name: "gen.tsv".into(),
        },
        Content: b"1\t11\n2\t22\n".to_vec(),
    });
    let sql = format!(
        "IMPORT INTO t (a, c) FROM 'gs://test-load/gen.tsv?endpoint={}' WITH fields_terminated_by='\\t'",
        gcs_endpoint()
    );
    s.tk.MustQuery(&sql);
    s.tk.MustQuery("SELECT * FROM t;")
        .Check(&testkit::Rows(&["1 2 11", "2 3 22"]));
});

simple_csv_import_test!(test_input_count_mismatch_and_default, |s: &MockGCSSuite| {
    s.prepare_and_use_db("load_data");
    s.tk.MustExec("create table t (a int, b int default 10, c int);");
    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "test-load".into(),
            Name: "mismatch.csv".into(),
        },
        Content: b"1,11\n2,22\n".to_vec(),
    });
    let sql = format!(
        "IMPORT INTO t (a, c) FROM 'gs://test-load/mismatch.csv?endpoint={}'",
        gcs_endpoint()
    );
    s.tk.MustQuery(&sql);
    s.tk.MustQuery("SELECT * FROM t;")
        .Check(&testkit::Rows(&["1 10 11", "2 10 22"]));
});

simple_csv_import_test!(test_deliver_bytes_rows, |s: &MockGCSSuite| {
    s.prepare_and_use_db("load_data");
    s.tk.MustExec("create table t (a int, b int);");
    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "test-load".into(),
            Name: "deliver.csv".into(),
        },
        Content: b"1,1\n2,2\n3,3\n".to_vec(),
    });
    let sql = format!(
        "IMPORT INTO t FROM 'gs://test-load/deliver.csv?endpoint={}'",
        gcs_endpoint()
    );
    let rows = s.tk.MustQuery(&sql).Rows();
    s.Len(&rows, 1);
    s.tk.MustQuery("SELECT count(1) FROM t;")
        .Check(&testkit::Rows(&["3"]));
});

simple_csv_import_test!(test_multi_value_index, |s: &MockGCSSuite| {
    s.prepare_and_use_db("load_data");
    s.tk.MustExec("create table t (a int, b json, index idx((cast(b as unsigned array))));");
    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "test-load".into(),
            Name: "mvi.csv".into(),
        },
        Content: b"1,\"[1,2]\"\n2,\"[3]\"\n".to_vec(),
    });
    let sql = format!(
        "IMPORT INTO t FROM 'gs://test-load/mvi.csv?endpoint={}'",
        gcs_endpoint()
    );
    s.tk.MustQuery(&sql);
    s.tk.MustQuery("SELECT * FROM t;")
        .Check(&testkit::Rows(&["1 [1,2]", "2 [3]"]));
});

simple_csv_import_test!(test_gbk, |s: &MockGCSSuite| {
    s.prepare_and_use_db("load_data");
    s.tk.MustExec("create table t (a varchar(100)) charset gbk;");
    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "test-load".into(),
            Name: "gbk.csv".into(),
        },
        Content: "hello\n".as_bytes().to_vec(),
    });
    let sql = format!(
        "IMPORT INTO t FROM 'gs://test-load/gbk.csv?endpoint={}' WITH character_set='gbk'",
        gcs_endpoint()
    );
    s.tk.MustQuery(&sql);
    s.tk.MustQuery("SELECT * FROM t")
        .Check(&testkit::Rows(&["hello"]));
});

simple_csv_import_test!(test_other_charset, |s: &MockGCSSuite| {
    s.prepare_and_use_db("load_data");
    s.tk.MustExec("create table t (a varchar(100));");
    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "test-load".into(),
            Name: "latin1.csv".into(),
        },
        Content: b"abc\n".to_vec(),
    });
    let sql = format!(
        "IMPORT INTO t FROM 'gs://test-load/latin1.csv?endpoint={}' WITH character_set='latin1'",
        gcs_endpoint()
    );
    s.tk.MustQuery(&sql);
    s.tk.MustQuery("SELECT * FROM t")
        .Check(&testkit::Rows(&["abc"]));
});
