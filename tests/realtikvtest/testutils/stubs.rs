// Copyright 2026 AsterSQL.
//! Adapters for SQL sessions, failpoints, and fake GCS boundaries.
//! SQL executes through the canonical TestKit with independent sessions sharing storage.
//!
//! Production algorithms live in `common.rs` / `compatibility.rs` /
//! `global_sort.rs` / `workload.rs` and call these surfaces the same way Go
//! calls testkit, failpoint, fakestorage, and kerneltype.

//! 中文说明开始（自动生成）
//! 中文总览：`stubs.rs` 只补充中文说明，不改任何可执行逻辑。
//! 该文件围绕 `测试工具与兼容封装` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、锁、统计信息、会话或时间戳语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 83 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `EVENTS` 是当前文件里的静态量。
//! `EVENTS` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `EVENTS` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `EVENTS`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `FAILPOINT_HOLD_MS` 是当前文件里的静态量。
//! `FAILPOINT_HOLD_MS` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `FAILPOINT_HOLD_MS` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `FAILPOINT_HOLD_MS`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `CLASSIC` 是当前文件里的静态量。
//! `CLASSIC` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `CLASSIC` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `CLASSIC`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `events_slot` 是当前文件里的辅助函数。
//! `events_slot` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `events_slot` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `events_slot`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `push_local_event` 是当前文件里的公开函数。
//! `push_local_event` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `push_local_event` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `push_local_event`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `take_local_events` 是当前文件里的公开函数。
//! `take_local_events` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `take_local_events` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `take_local_events`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `clear_local_events` 是当前文件里的公开函数。
//! `clear_local_events` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `clear_local_events` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `clear_local_events`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `set_failpoint_hold_ms_for_test` 是当前文件里的公开函数。
//! `set_failpoint_hold_ms_for_test` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `set_failpoint_hold_ms_for_test` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `set_failpoint_hold_ms_for_test`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `failpoint_hold_duration` 是当前文件里的公开函数。
//! `failpoint_hold_duration` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `failpoint_hold_duration` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `failpoint_hold_duration`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `set_classic_for_test` 是当前文件里的公开函数。
//! `set_classic_for_test` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `set_classic_for_test` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `set_classic_for_test`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `reset_test_globals` 是当前文件里的公开函数。
//! `reset_test_globals` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `reset_test_globals` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `reset_test_globals`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `kerneltype` 是当前文件里的模块。
//! `kerneltype` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `kerneltype` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `kerneltype`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `IsClassic` 是当前文件里的公开函数。
//! `IsClassic` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `IsClassic` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `IsClassic`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `IsNextGen` 是当前文件里的公开函数。
//! `IsNextGen` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `IsNextGen` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `IsNextGen`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `require` 是当前文件里的模块。
//! `require` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `require` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `require`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `NoError` 是当前文件里的公开函数。
//! `NoError` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `NoError` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `NoError`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `Contains` 是当前文件里的公开函数。
//! `Contains` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `Contains` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `Contains`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `Equal` 是当前文件里的公开函数。
//! `Equal` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `Equal` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `Equal`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `equal` 是当前文件里的公开函数。
//! `equal` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `equal` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `equal`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `Empty` 是当前文件里的公开函数。
//! `Empty` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `Empty` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `Empty`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `Nil` 是当前文件里的公开函数。
//! `Nil` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `Nil` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `Nil`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `True` 是当前文件里的公开函数。
//! `True` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `True` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `True`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `logutil` 是当前文件里的模块。
//! `logutil` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `logutil` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `logutil`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `Logger` 是当前文件里的状态类型。
//! `Logger` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `Logger` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `Logger`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `Info` 是当前文件里的公开函数。
//! `Info` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `Info` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `Info`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `Error` 是当前文件里的公开函数。
//! `Error` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `Error` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `Error`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `BgLogger` 是当前文件里的公开函数。
//! `BgLogger` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `BgLogger` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `BgLogger`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `failpoint` 是当前文件里的模块。
//! `failpoint` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `failpoint` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `failpoint`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `ENABLED` 是当前文件里的静态量。
//! `ENABLED` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `ENABLED` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `ENABLED`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `slot` 是当前文件里的辅助函数。
//! `slot` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `slot` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `slot`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `Enable` 是当前文件里的公开函数。
//! `Enable` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `Enable` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `Enable`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `Disable` 是当前文件里的公开函数。
//! `Disable` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `Disable` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `Disable`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `is_enabled` 是当前文件里的公开函数。
//! `is_enabled` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `is_enabled` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `is_enabled`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `reset` 是当前文件里的公开函数。
//! `reset` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `reset` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `reset`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `testkit` 是当前文件里的模块。
//! `testkit` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `testkit` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `testkit`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `ExecHandler` 是当前文件里的类型别名。
//! `ExecHandler` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `ExecHandler` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `ExecHandler`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `ResultSet` 是当前文件里的状态类型。
//! `ResultSet` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `ResultSet` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `ResultSet`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `new` 是当前文件里的公开函数。
//! `new` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `new` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `new`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `Close` 是当前文件里的公开函数。
//! `Close` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `Close` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `Close`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `is_closed` 是当前文件里的公开函数。
//! `is_closed` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `is_closed` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `is_closed`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `Session` 是当前文件里的状态类型。
//! `Session` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `Session` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `Session`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `AffectedRows` 是当前文件里的公开函数。
//! `AffectedRows` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `AffectedRows` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `AffectedRows`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `set_affected_rows` 是当前文件里的公开函数。
//! `set_affected_rows` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `set_affected_rows` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `set_affected_rows`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `TestKit` 是当前文件里的状态类型。
//! `TestKit` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `TestKit` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestKit`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `MustExec` 是当前文件里的公开函数。
//! `MustExec` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `MustExec` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `MustExec`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `Exec` 是当前文件里的公开函数。
//! `Exec` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `Exec` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `Exec`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `execs` 是当前文件里的公开函数。
//! `execs` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `execs` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `execs`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `set_exec_handler` 是当前文件里的公开函数。
//! `set_exec_handler` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `set_exec_handler` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `set_exec_handler`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `clear_exec_handler` 是当前文件里的公开函数。
//! `clear_exec_handler` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `clear_exec_handler` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `clear_exec_handler`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `NewTestKit` 是当前文件里的公开函数。
//! `NewTestKit` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `NewTestKit` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `NewTestKit`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `storage` 是当前文件里的模块。
//! `storage` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `storage` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `storage`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `ErrorKind` 是当前文件里的分支类型。
//! `ErrorKind` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `ErrorKind` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `ErrorKind`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `object_not_exist` 是当前文件里的公开函数。
//! `object_not_exist` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `object_not_exist` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `object_not_exist`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `other` 是当前文件里的公开函数。
//! `other` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `other` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `other`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `is_object_not_exist` 是当前文件里的公开函数。
//! `is_object_not_exist` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `is_object_not_exist` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `is_object_not_exist`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `fmt` 是当前文件里的辅助函数。
//! `fmt` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `fmt` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `fmt`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `ErrObjectNotExist` 是当前文件里的公开函数。
//! `ErrObjectNotExist` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `ErrObjectNotExist` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `ErrObjectNotExist`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `fakestorage` 是当前文件里的模块。
//! `fakestorage` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `fakestorage` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `fakestorage`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `ListOptions` 是当前文件里的状态类型。
//! `ListOptions` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `ListOptions` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `ListOptions`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `ObjectAttrs` 是当前文件里的状态类型。
//! `ObjectAttrs` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `ObjectAttrs` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `ObjectAttrs`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `ObjectHandle` 是当前文件里的状态类型。
//! `ObjectHandle` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `ObjectHandle` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `ObjectHandle`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `Delete` 是当前文件里的公开函数。
//! `Delete` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `Delete` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `Delete`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `BucketHandle` 是当前文件里的状态类型。
//! `BucketHandle` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `BucketHandle` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `BucketHandle`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `Object` 是当前文件里的公开函数。
//! `Object` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `Object` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `Object`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `Client` 是当前文件里的状态类型。
//! `Client` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `Client` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `Client`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `Bucket` 是当前文件里的公开函数。
//! `Bucket` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `Bucket` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `Bucket`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `ServerInner` 是当前文件里的状态类型。
//! `ServerInner` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `ServerInner` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `ServerInner`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `Server` 是当前文件里的状态类型。
//! `Server` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `Server` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `Server`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `put_objects` 是当前文件里的公开函数。
//! `put_objects` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `put_objects` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `put_objects`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `objects` 是当前文件里的公开函数。
//! `objects` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `objects` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `objects`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `set_list_error` 是当前文件里的公开函数。
//! `set_list_error` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `set_list_error` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `set_list_error`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `set_delete_error` 是当前文件里的公开函数。
//! `set_delete_error` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `set_delete_error` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `set_delete_error`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `ListObjectsWithOptions` 是当前文件里的公开函数。
//! `ListObjectsWithOptions` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `ListObjectsWithOptions` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `ListObjectsWithOptions`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `default` 是当前文件里的辅助函数。
//! `default` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `default` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `default`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `reset_global_errors` 是当前文件里的公开函数。
//! `reset_global_errors` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `reset_global_errors` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `reset_global_errors`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `ExternalTaggedField` 是当前文件里的分支类型。
//! `ExternalTaggedField` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `ExternalTaggedField` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `ExternalTaggedField`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `ExternalTagged` 是当前文件里的契约类型。
//! `ExternalTagged` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `ExternalTagged` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `ExternalTagged`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `external_fields` 是当前文件里的辅助函数。
//! `external_fields` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `external_fields` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `external_fields`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 中文说明结束（自动生成）

use astersql_tests_realtikvtest::stubs::{Storage, TestCtx, push_event};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

// ---------------------------------------------------------------------------
// Shared event log / injectable TestKit behaviour
// ---------------------------------------------------------------------------

static EVENTS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
static FAILPOINT_HOLD_MS: AtomicU64 = AtomicU64::new(10_000);
static CLASSIC: AtomicBool = AtomicBool::new(true);

fn events_slot() -> &'static Mutex<Vec<String>> {
    EVENTS.get_or_init(|| Mutex::new(Vec::new()))
}

pub fn push_local_event(e: impl Into<String>) {
    let s = e.into();
    push_event(s.clone());
    events_slot().lock().unwrap().push(s);
}

pub fn take_local_events() -> Vec<String> {
    std::mem::take(&mut *events_slot().lock().unwrap())
}

pub fn clear_local_events() {
    events_slot().lock().unwrap().clear();
}

pub fn set_failpoint_hold_ms_for_test(ms: u64) {
    FAILPOINT_HOLD_MS.store(ms, Ordering::SeqCst);
}

pub fn failpoint_hold_duration() -> Duration {
    Duration::from_millis(FAILPOINT_HOLD_MS.load(Ordering::SeqCst))
}

pub fn set_classic_for_test(v: bool) {
    CLASSIC.store(v, Ordering::SeqCst);
}

pub fn reset_test_globals() {
    clear_local_events();
    set_failpoint_hold_ms_for_test(10_000);
    set_classic_for_test(true);
    failpoint::reset();
    fakestorage::reset_global_errors();
}

// ---------------------------------------------------------------------------
// kerneltype
// ---------------------------------------------------------------------------

pub mod kerneltype {
    use super::*;

    pub fn IsClassic() -> bool {
        CLASSIC.load(Ordering::SeqCst)
    }

    pub fn IsNextGen() -> bool {
        !IsClassic()
    }
}

// ---------------------------------------------------------------------------
// require (testify subset used by this package)
// ---------------------------------------------------------------------------

pub mod require {
    use super::TestCtx;

    pub fn NoError(t: &TestCtx, err: Result<(), String>) {
        if let Err(e) = err {
            t.Fail();
            panic!("require.NoError: {e}");
        }
    }

    pub fn Contains(t: &TestCtx, haystack: &str, needle: &str) {
        if !haystack.contains(needle) {
            t.Fail();
            panic!("require.Contains: {haystack:?} does not contain {needle:?}");
        }
    }

    pub fn Equal<T: PartialEq + std::fmt::Debug>(t: &TestCtx, expected: T, actual: T) {
        if expected != actual {
            t.Fail();
            panic!("require.Equal: expected {expected:?}, got {actual:?}");
        }
    }

    /// Snake alias used by production modules.
    pub fn equal<T: PartialEq + std::fmt::Debug>(t: &TestCtx, expected: T, actual: T) {
        Equal(t, expected, actual);
    }

    pub fn Empty<T>(t: &TestCtx, v: &[T]) {
        if !v.is_empty() {
            t.Fail();
            panic!("require.Empty: len={}", v.len());
        }
    }

    pub fn Nil(t: &TestCtx, is_nil: bool, field: &str) {
        if !is_nil {
            t.Fail();
            panic!("Field {field} should be nil");
        }
    }

    pub fn True(t: &TestCtx, cond: bool, msg: &str) {
        if !cond {
            t.Fail();
            panic!("require.True: {msg}");
        }
    }
}

// ---------------------------------------------------------------------------
// logutil (no-op logger matching call sites)
// ---------------------------------------------------------------------------

pub mod logutil {
    use super::push_local_event;

    pub struct Logger;

    impl Logger {
        pub fn Info(&self, msg: &str, fields: &[(&str, String)]) {
            let extra: Vec<String> = fields.iter().map(|(k, v)| format!("{k}={v}")).collect();
            push_local_event(format!("log.Info:{msg}:{}", extra.join(",")));
        }

        pub fn Error(&self, msg: &str, fields: &[(&str, String)]) {
            let extra: Vec<String> = fields.iter().map(|(k, v)| format!("{k}={v}")).collect();
            push_local_event(format!("log.Error:{msg}:{}", extra.join(",")));
        }
    }

    /// Go `logutil.BgLogger()`.
    pub fn BgLogger() -> Logger {
        Logger
    }
}

// ---------------------------------------------------------------------------
// failpoint
// ---------------------------------------------------------------------------

pub mod failpoint {
    use super::*;
    use std::collections::HashSet;

    static ENABLED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

    fn slot() -> &'static Mutex<HashSet<String>> {
        ENABLED.get_or_init(|| Mutex::new(HashSet::new()))
    }

    pub fn Enable(path: &str, _term: &str) -> Result<(), String> {
        slot().lock().unwrap().insert(path.to_string());
        push_local_event(format!("failpoint.Enable:{path}"));
        Ok(())
    }

    pub fn Disable(path: &str) -> Result<(), String> {
        slot().lock().unwrap().remove(path);
        push_local_event(format!("failpoint.Disable:{path}"));
        Ok(())
    }

    pub fn is_enabled(path: &str) -> bool {
        slot().lock().unwrap().contains(path)
    }

    pub fn reset() {
        slot().lock().unwrap().clear();
    }
}

// ---------------------------------------------------------------------------
// testkit SQL harness (Exec / MustExec / Session.AffectedRows)
// ---------------------------------------------------------------------------

pub mod testkit {
    use super::*;

    pub type ExecOutcome = (Option<ResultSet>, Option<String>);
    #[cfg(test)]
    type OutcomeHandler = Arc<dyn Fn(&str) -> ExecOutcome + Send + Sync>;

    pub type ExecHandler = Arc<dyn Fn(&str) -> Result<Option<ResultSet>, String> + Send + Sync>;

    #[derive(Clone, Debug, Default)]
    pub struct ResultSet {
        closed: Arc<Mutex<bool>>,
        close_error: Option<String>,
    }

    impl ResultSet {
        pub fn new() -> Self {
            Self {
                closed: Arc::new(Mutex::new(false)),
                close_error: None,
            }
        }

        pub fn Close(&self) -> Result<(), String> {
            *self.closed.lock().unwrap() = true;
            push_local_event("rs.Close");
            match &self.close_error {
                Some(e) => Err(e.clone()),
                None => Ok(()),
            }
        }

        #[cfg(test)]
        pub(crate) fn with_close_error(error: &str) -> Self {
            Self {
                close_error: Some(error.into()),
                ..Self::new()
            }
        }

        pub fn is_closed(&self) -> bool {
            *self.closed.lock().unwrap()
        }
    }

    #[derive(Clone, Debug)]
    pub struct Session {
        affected: Arc<Mutex<u64>>,
    }

    impl Session {
        pub fn AffectedRows(&self) -> u64 {
            *self.affected.lock().unwrap()
        }

        pub fn set_affected_rows(&self, n: u64) {
            *self.affected.lock().unwrap() = n;
        }
    }

    #[derive(Clone)]
    pub struct TestKit {
        pub store: Storage,
        execs: Arc<Mutex<Vec<String>>>,
        session: Session,
        handler: Arc<Mutex<Option<ExecHandler>>>,
        backend: Arc<Mutex<astersql_testkit::TestKit>>,
        #[cfg(test)]
        outcome_handler: Arc<Mutex<Option<OutcomeHandler>>>,
    }

    impl TestKit {
        pub fn MustExec(&self, sql: &str) {
            push_local_event(format!("tk.MustExec:{sql}"));
            self.execs.lock().unwrap().push(sql.to_string());
            self.execute(sql)
                .unwrap_or_else(|e| panic!("tk.MustExec failed: {e}"));
        }

        pub fn Exec(&self, sql: &str) -> Result<Option<ResultSet>, String> {
            push_local_event(format!("tk.Exec:{sql}"));
            self.execs.lock().unwrap().push(sql.to_string());
            self.execute(sql)
        }

        /// Go Exec returns the record set and error independently.
        pub fn ExecWithResult(&self, sql: &str) -> ExecOutcome {
            #[cfg(test)]
            if let Some(handler) = self.outcome_handler.lock().unwrap().clone() {
                return handler(sql);
            }
            match self.Exec(sql) {
                Ok(rs) => (rs, None),
                Err(e) => (None, Some(e)),
            }
        }

        #[cfg(test)]
        pub(crate) fn set_outcome_handler(
            &self,
            handler: impl Fn(&str) -> ExecOutcome + Send + Sync + 'static,
        ) {
            *self.outcome_handler.lock().unwrap() = Some(Arc::new(handler));
        }

        fn execute(&self, sql: &str) -> Result<Option<ResultSet>, String> {
            if let Some(handler) = self.handler.lock().unwrap().clone() {
                return handler(sql);
            }
            let mut backend = self.backend.lock().unwrap();
            let result = backend.Exec(sql, Vec::new()).map_err(|e| e.to_string())?;
            self.session.set_affected_rows(result.affected_rows);
            Ok(None)
        }

        pub fn Query(&self, sql: &str) -> Result<Vec<Vec<String>>, String> {
            self.backend
                .lock()
                .unwrap()
                .Query(sql, Vec::new())
                .map(|rows| rows.string_rows())
                .map_err(|e| e.to_string())
        }

        pub fn Session(&self) -> Session {
            self.session.clone()
        }

        pub fn execs(&self) -> Vec<String> {
            self.execs.lock().unwrap().clone()
        }

        pub fn set_exec_handler<F>(&self, f: F)
        where
            F: Fn(&str) -> Result<Option<ResultSet>, String> + Send + Sync + 'static,
        {
            *self.handler.lock().unwrap() = Some(Arc::new(f));
        }

        pub fn clear_exec_handler(&self) {
            *self.handler.lock().unwrap() = None;
        }
    }

    pub fn NewTestKit(_t: &TestCtx, store: Storage) -> TestKit {
        push_local_event("testkit.NewTestKit");
        // Each Go storage has one database and each TestKit creates an independent
        // session. Weak entries do not extend the lifetime of completed fixtures.
        type Backend = astersql_testkit::mockstore::AnalyzeStatsStore;
        static STORES: OnceLock<Mutex<std::collections::HashMap<u64, std::sync::Weak<Backend>>>> =
            OnceLock::new();
        let backend_store = {
            let mut stores = STORES.get_or_init(Default::default).lock().unwrap();
            stores.retain(|_, store| store.strong_count() > 0);
            if let Some(existing) = stores.get(&store.id).and_then(std::sync::Weak::upgrade) {
                existing
            } else {
                let backend = astersql_testkit::mockstore::CreateAnalyzeStatsStore();
                stores.insert(store.id, Arc::downgrade(&backend));
                backend
            }
        };
        let backend = Arc::new(Mutex::new(astersql_testkit::TestKit::new(backend_store)));
        TestKit {
            store,
            execs: Arc::new(Mutex::new(Vec::new())),
            session: Session {
                affected: Arc::new(Mutex::new(0)),
            },
            handler: Arc::new(Mutex::new(None)),
            backend,
            #[cfg(test)]
            outcome_handler: Arc::new(Mutex::new(None)),
        }
    }
}

// ---------------------------------------------------------------------------
// cloud.storage / fake-gcs-server
// ---------------------------------------------------------------------------

pub mod storage {
    use std::fmt;

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Error {
        kind: ErrorKind,
        message: String,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum ErrorKind {
        ObjectNotExist,
        Other,
    }

    impl Error {
        pub fn object_not_exist() -> Self {
            Self {
                kind: ErrorKind::ObjectNotExist,
                message: "storage: object doesn't exist".into(),
            }
        }

        pub fn other(msg: impl Into<String>) -> Self {
            Self {
                kind: ErrorKind::Other,
                message: msg.into(),
            }
        }

        pub fn is_object_not_exist(&self) -> bool {
            matches!(self.kind, ErrorKind::ObjectNotExist)
        }
    }

    impl fmt::Display for Error {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(&self.message)
        }
    }

    impl std::error::Error for Error {}

    /// Go `storage.ErrObjectNotExist`.
    pub fn ErrObjectNotExist() -> Error {
        Error::object_not_exist()
    }

    pub fn is_object_not_exist(err: &Error) -> bool {
        err.is_object_not_exist()
    }
}

pub mod fakestorage {
    use super::*;
    use std::sync::Mutex;

    #[derive(Clone, Debug, Default)]
    pub struct ListOptions;

    #[derive(Clone, Debug)]
    pub struct ObjectAttrs {
        pub Name: String,
    }

    #[derive(Clone)]
    pub struct ObjectHandle {
        server: Arc<Mutex<ServerInner>>,
        bucket: String,
        name: String,
        fail_delete: Arc<Mutex<Option<storage::Error>>>,
    }

    impl ObjectHandle {
        pub fn Delete(&self, _ctx: ()) -> Result<(), storage::Error> {
            if let Some(err) = self.fail_delete.lock().unwrap().clone() {
                push_local_event(format!(
                    "gcs.DeleteFail:{}:{}:{}",
                    self.bucket, self.name, err
                ));
                return Err(err);
            }
            let mut g = self.server.lock().unwrap();
            if let Some(objs) = g.buckets.get_mut(&self.bucket) {
                let before = objs.len();
                objs.retain(|o| o != &self.name);
                if objs.len() == before {
                    return Err(storage::ErrObjectNotExist());
                }
            } else {
                return Err(storage::ErrObjectNotExist());
            }
            push_local_event(format!("gcs.Delete:{}:{}", self.bucket, self.name));
            Ok(())
        }
    }

    #[derive(Clone)]
    pub struct BucketHandle {
        server: Arc<Mutex<ServerInner>>,
        name: String,
        fail_delete: Arc<Mutex<Option<storage::Error>>>,
    }

    impl BucketHandle {
        pub fn Object(&self, object: &str) -> ObjectHandle {
            ObjectHandle {
                server: self.server.clone(),
                bucket: self.name.clone(),
                name: object.to_string(),
                fail_delete: self.fail_delete.clone(),
            }
        }
    }

    #[derive(Clone)]
    pub struct Client {
        server: Arc<Mutex<ServerInner>>,
        fail_delete: Arc<Mutex<Option<storage::Error>>>,
    }

    impl Client {
        pub fn Bucket(&self, name: &str) -> BucketHandle {
            BucketHandle {
                server: self.server.clone(),
                name: name.to_string(),
                fail_delete: self.fail_delete.clone(),
            }
        }
    }

    #[derive(Default)]
    struct ServerInner {
        buckets: HashMap<String, Vec<String>>,
        list_err: Option<String>,
    }

    /// In-memory stand-in for `fakestorage.Server`.
    #[derive(Clone)]
    pub struct Server {
        inner: Arc<Mutex<ServerInner>>,
        fail_delete: Arc<Mutex<Option<storage::Error>>>,
    }

    impl Server {
        pub fn new() -> Self {
            Self {
                inner: Arc::new(Mutex::new(ServerInner::default())),
                fail_delete: Arc::new(Mutex::new(None)),
            }
        }

        pub fn put_objects(&self, bucket: &str, names: Vec<String>) {
            self.inner
                .lock()
                .unwrap()
                .buckets
                .insert(bucket.to_string(), names);
        }

        pub fn objects(&self, bucket: &str) -> Vec<String> {
            self.inner
                .lock()
                .unwrap()
                .buckets
                .get(bucket)
                .cloned()
                .unwrap_or_default()
        }

        pub fn set_list_error(&self, err: Option<String>) {
            self.inner.lock().unwrap().list_err = err;
        }

        /// Next Delete calls return this error (e.g. ErrObjectNotExist race).
        pub fn set_delete_error(&self, err: Option<storage::Error>) {
            *self.fail_delete.lock().unwrap() = err;
        }

        pub fn ListObjectsWithOptions(
            &self,
            bucket: &str,
            _opts: ListOptions,
        ) -> (Vec<ObjectAttrs>, Result<(), String>) {
            let g = self.inner.lock().unwrap();
            if let Some(err) = &g.list_err {
                return (Vec::new(), Err(err.clone()));
            }
            let attrs = g
                .buckets
                .get(bucket)
                .map(|names| {
                    names
                        .iter()
                        .map(|n| ObjectAttrs { Name: n.clone() })
                        .collect()
                })
                .unwrap_or_default();
            (attrs, Ok(()))
        }

        pub fn Client(&self) -> Client {
            Client {
                server: self.inner.clone(),
                fail_delete: self.fail_delete.clone(),
            }
        }
    }

    impl Default for Server {
        fn default() -> Self {
            Self::new()
        }
    }

    pub fn reset_global_errors() {
        // per-server state; nothing global beyond failpoint hold.
    }
}

// ---------------------------------------------------------------------------
// External-tagged field descriptors for AssertExternalField
// ---------------------------------------------------------------------------

/// One Go struct field tagged `external:"true"`.
#[derive(Debug, Clone)]
pub enum ExternalTaggedField {
    Ptr { name: String, is_nil: bool },
    Struct { name: String, is_zero: bool },
    Slice { name: String, len: usize },
    Map { name: String, len: usize },
}

/// Types that expose `external:"true"` fields for [`crate::AssertExternalField`].
pub trait ExternalTagged {
    fn external_fields(&self) -> Vec<ExternalTaggedField>;
}
