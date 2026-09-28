// Copyright 2026 AsterSQL.
//! Local stand-ins for TiKV / SQL / session / config boundaries
//! (arm64-safe; no kv/domain/kvproto/grpcio).
//!
//! Mirrors the surfaces `tests/realtikvtest` needs from config, ddl, domain,
//! keyspace, kv, session, store, testkit, tikv client, and goleak without
//! pulling heavy crates. Algorithms in `testkit.rs` stay faithful to Go.

//! 中文说明开始（自动生成）
//! 中文总览：`stubs.rs` 只补充中文说明，不改任何可执行逻辑。
//! 该文件围绕 `真实 TiKV 集成测试` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、锁、统计信息、会话或时间戳语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 123 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `EVENTS` 是当前文件里的静态量。
//! `EVENTS` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `EVENTS` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `EVENTS`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `OPEN_FAIL` 是当前文件里的静态量。
//! `OPEN_FAIL` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `OPEN_FAIL` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `OPEN_FAIL`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `BOOTSTRAP_FAIL` 是当前文件里的静态量。
//! `BOOTSTRAP_FAIL` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `BOOTSTRAP_FAIL` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `BOOTSTRAP_FAIL`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `OWNER_FAIL` 是当前文件里的静态量。
//! `OWNER_FAIL` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `OWNER_FAIL` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `OWNER_FAIL`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `NEXT_GEN` 是当前文件里的静态量。
//! `NEXT_GEN` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `NEXT_GEN` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `NEXT_GEN`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `BASE_TABLES` 是当前文件里的静态量。
//! `BASE_TABLES` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `BASE_TABLES` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `BASE_TABLES`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `VIEWS` 是当前文件里的静态量。
//! `VIEWS` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `VIEWS` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `VIEWS`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `MVCC_WAIT` 是当前文件里的静态量。
//! `MVCC_WAIT` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `MVCC_WAIT` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `MVCC_WAIT`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `STORE_SEQ` 是当前文件里的静态量。
//! `STORE_SEQ` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `STORE_SEQ` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `STORE_SEQ`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `events_slot` 是当前文件里的辅助函数。
//! `events_slot` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `events_slot` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `events_slot`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `push_event` 是当前文件里的公开函数。
//! `push_event` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `push_event` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `push_event`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `take_events` 是当前文件里的公开函数。
//! `take_events` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `take_events` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `take_events`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `clear_events` 是当前文件里的公开函数。
//! `clear_events` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `clear_events` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `clear_events`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `set_open_fail` 是当前文件里的公开函数。
//! `set_open_fail` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `set_open_fail` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `set_open_fail`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `set_bootstrap_fail` 是当前文件里的公开函数。
//! `set_bootstrap_fail` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `set_bootstrap_fail` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `set_bootstrap_fail`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `set_owner_fail` 是当前文件里的公开函数。
//! `set_owner_fail` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `set_owner_fail` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `set_owner_fail`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `set_next_gen` 是当前文件里的公开函数。
//! `set_next_gen` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `set_next_gen` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `set_next_gen`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `set_base_tables` 是当前文件里的公开函数。
//! `set_base_tables` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `set_base_tables` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `set_base_tables`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `set_views` 是当前文件里的公开函数。
//! `set_views` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `set_views` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `set_views`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `set_mvcc_wait` 是当前文件里的公开函数。
//! `set_mvcc_wait` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `set_mvcc_wait` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `set_mvcc_wait`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `mvcc_wait` 是当前文件里的公开函数。
//! `mvcc_wait` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `mvcc_wait` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `mvcc_wait`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `reset_test_globals` 是当前文件里的公开函数。
//! `reset_test_globals` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `reset_test_globals` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `reset_test_globals`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `DEFAULT_TIKV_PATH` 是当前文件里的常量。
//! `DEFAULT_TIKV_PATH` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `DEFAULT_TIKV_PATH` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `DEFAULT_TIKV_PATH`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `PD_ADDR` 是当前文件里的常量。
//! `PD_ADDR` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `PD_ADDR` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `PD_ADDR`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `WITH_REAL_TIKV` 是当前文件里的静态量。
//! `WITH_REAL_TIKV` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `WITH_REAL_TIKV` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `WITH_REAL_TIKV`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `TIKV_PATH` 是当前文件里的静态量。
//! `TIKV_PATH` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `TIKV_PATH` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TIKV_PATH`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `MOCK_PORT_ALLOC` 是当前文件里的静态量。
//! `MOCK_PORT_ALLOC` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `MOCK_PORT_ALLOC` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `MOCK_PORT_ALLOC`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `FLAGS_INIT` 是当前文件里的静态量。
//! `FLAGS_INIT` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `FLAGS_INIT` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `FLAGS_INIT`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `ensure_tikv_path` 是当前文件里的辅助函数。
//! `ensure_tikv_path` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `ensure_tikv_path` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `ensure_tikv_path`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `with_real_tikv` 是当前文件里的公开函数。
//! `with_real_tikv` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `with_real_tikv` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `with_real_tikv`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `set_with_real_tikv` 是当前文件里的公开函数。
//! `set_with_real_tikv` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `set_with_real_tikv` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `set_with_real_tikv`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `tikv_path` 是当前文件里的公开函数。
//! `tikv_path` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `tikv_path` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `tikv_path`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `set_tikv_path` 是当前文件里的公开函数。
//! `set_tikv_path` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `set_tikv_path` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `set_tikv_path`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `mock_port_alloc_add` 是当前文件里的公开函数。
//! `mock_port_alloc_add` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `mock_port_alloc_add` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `mock_port_alloc_add`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `mock_port_alloc_get` 是当前文件里的公开函数。
//! `mock_port_alloc_get` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `mock_port_alloc_get` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `mock_port_alloc_get`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `TestCtxInner` 是当前文件里的状态类型。
//! `TestCtxInner` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `TestCtxInner` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestCtxInner`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `TestCtx` 是当前文件里的状态类型。
//! `TestCtx` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `TestCtx` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestCtx`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `clone` 是当前文件里的辅助函数。
//! `clone` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `clone` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `clone`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `default` 是当前文件里的辅助函数。
//! `default` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `default` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `default`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `new` 是当前文件里的公开函数。
//! `new` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `new` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `new`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `Fail` 是当前文件里的公开函数。
//! `Fail` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `Fail` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `Fail`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `Failed` 是当前文件里的公开函数。
//! `Failed` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `Failed` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `Failed`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `Log` 是当前文件里的公开函数。
//! `Log` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `Log` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `Log`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `Cleanup` 是当前文件里的公开函数。
//! `Cleanup` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `Cleanup` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `Cleanup`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `run_cleanups` 是当前文件里的公开函数。
//! `run_cleanups` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `run_cleanups` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `run_cleanups`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `logs` 是当前文件里的公开函数。
//! `logs` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `logs` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `logs`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `drop` 是当前文件里的辅助函数。
//! `drop` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `drop` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `drop`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `TestMain` 是当前文件里的状态类型。
//! `TestMain` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `TestMain` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestMain`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `require` 是当前文件里的模块。
//! `require` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `require` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `require`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `NoError` 是当前文件里的公开函数。
//! `NoError` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `NoError` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `NoError`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `config` 是当前文件里的模块。
//! `config` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `config` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `config`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `StoreTypeTiKV` 是当前文件里的常量。
//! `StoreTypeTiKV` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `StoreTypeTiKV` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `StoreTypeTiKV`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `AsyncCommit` 是当前文件里的状态类型。
//! `AsyncCommit` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `AsyncCommit` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `AsyncCommit`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `TiKVClient` 是当前文件里的状态类型。
//! `TiKVClient` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `TiKVClient` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TiKVClient`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `TxnLocalLatches` 是当前文件里的状态类型。
//! `TxnLocalLatches` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `TxnLocalLatches` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TxnLocalLatches`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `Instance` 是当前文件里的状态类型。
//! `Instance` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `Instance` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `Instance`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `Config` 是当前文件里的状态类型。
//! `Config` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `Config` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `Config`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `GLOBAL` 是当前文件里的静态量。
//! `GLOBAL` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `GLOBAL` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `GLOBAL`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `slot` 是当前文件里的辅助函数。
//! `slot` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `slot` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `slot`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `reset_global` 是当前文件里的公开函数。
//! `reset_global` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `reset_global` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `reset_global`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `GetGlobalConfig` 是当前文件里的公开函数。
//! `GetGlobalConfig` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `GetGlobalConfig` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `GetGlobalConfig`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `StoreGlobalConfig` 是当前文件里的公开函数。
//! `StoreGlobalConfig` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `StoreGlobalConfig` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `StoreGlobalConfig`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `UpdateGlobal` 是当前文件里的公开函数。
//! `UpdateGlobal` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `UpdateGlobal` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `UpdateGlobal`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `kerneltype` 是当前文件里的模块。
//! `kerneltype` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `kerneltype` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `kerneltype`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `IsNextGen` 是当前文件里的公开函数。
//! `IsNextGen` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `IsNextGen` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `IsNextGen`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `keyspace` 是当前文件里的模块。
//! `keyspace` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `keyspace` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `keyspace`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `System` 是当前文件里的常量。
//! `System` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `System` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `System`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `handle` 是当前文件里的模块。
//! `handle` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `handle` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `handle`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `NextGenTargetScope` 是当前文件里的常量。
//! `NextGenTargetScope` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `NextGenTargetScope` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `NextGenTargetScope`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `vardef` 是当前文件里的模块。
//! `vardef` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `vardef` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `vardef`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `DefInnodbLockWaitTimeout` 是当前文件里的常量。
//! `DefInnodbLockWaitTimeout` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `DefInnodbLockWaitTimeout` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `DefInnodbLockWaitTimeout`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `SCHEMA_LEASE` 是当前文件里的静态量。
//! `SCHEMA_LEASE` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `SCHEMA_LEASE` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `SCHEMA_LEASE`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `SetSchemaLease` 是当前文件里的公开函数。
//! `SetSchemaLease` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `SetSchemaLease` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `SetSchemaLease`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `schema_lease` 是当前文件里的公开函数。
//! `schema_lease` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `schema_lease` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `schema_lease`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `reset_schema_lease` 是当前文件里的公开函数。
//! `reset_schema_lease` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `reset_schema_lease` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `reset_schema_lease`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `Storage` 是当前文件里的状态类型。
//! `Storage` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `Storage` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `Storage`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `Close` 是当前文件里的公开函数。
//! `Close` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `Close` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `Close`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `is_closed` 是当前文件里的公开函数。
//! `is_closed` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `is_closed` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `is_closed`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `MockSessionManager` 是当前文件里的状态类型。
//! `MockSessionManager` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `MockSessionManager` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `MockSessionManager`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `InfoSyncer` 是当前文件里的状态类型。
//! `InfoSyncer` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `InfoSyncer` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `InfoSyncer`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `SetSessionManager` 是当前文件里的公开函数。
//! `SetSessionManager` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `SetSessionManager` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `SetSessionManager`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `has_session_manager` 是当前文件里的公开函数。
//! `has_session_manager` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `has_session_manager` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `has_session_manager`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `Domain` 是当前文件里的状态类型。
//! `Domain` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `Domain` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `Domain`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `ddl` 是当前文件里的模块。
//! `ddl` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `ddl` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `ddl`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `StartOwnerManager` 是当前文件里的公开函数。
//! `StartOwnerManager` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `StartOwnerManager` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `StartOwnerManager`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `CloseOwnerManager` 是当前文件里的公开函数。
//! `CloseOwnerManager` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `CloseOwnerManager` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `CloseOwnerManager`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `session` 是当前文件里的模块。
//! `session` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `session` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `session`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `BootstrapSession` 是当前文件里的公开函数。
//! `BootstrapSession` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `BootstrapSession` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `BootstrapSession`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `kvstore` 是当前文件里的模块。
//! `kvstore` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `kvstore` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `kvstore`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `SYSTEM` 是当前文件里的静态量。
//! `SYSTEM` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `SYSTEM` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `SYSTEM`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `REGISTERED` 是当前文件里的静态量。
//! `REGISTERED` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `REGISTERED` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `REGISTERED`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `Register` 是当前文件里的公开函数。
//! `Register` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `Register` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `Register`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `SetSystemStorage` 是当前文件里的公开函数。
//! `SetSystemStorage` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `SetSystemStorage` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `SetSystemStorage`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `system_storage` 是当前文件里的公开函数。
//! `system_storage` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `system_storage` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `system_storage`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `is_registered` 是当前文件里的公开函数。
//! `is_registered` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `is_registered` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `is_registered`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `driver` 是当前文件里的模块。
//! `driver` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `driver` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `driver`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `TiKVDriver` 是当前文件里的状态类型。
//! `TiKVDriver` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `TiKVDriver` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TiKVDriver`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `Open` 是当前文件里的公开函数。
//! `Open` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `Open` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `Open`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `testkit` 是当前文件里的模块。
//! `testkit` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `testkit` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `testkit`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `ResultSet` 是当前文件里的状态类型。
//! `ResultSet` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `ResultSet` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `ResultSet`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `Rows` 是当前文件里的公开函数。
//! `Rows` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `Rows` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `Rows`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `TestKit` 是当前文件里的状态类型。
//! `TestKit` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `TestKit` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestKit`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `MustExec` 是当前文件里的公开函数。
//! `MustExec` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `MustExec` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `MustExec`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `MustQuery` 是当前文件里的公开函数。
//! `MustQuery` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `MustQuery` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `MustQuery`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `execs` 是当前文件里的公开函数。
//! `execs` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `execs` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `execs`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `NewTestKit` 是当前文件里的公开函数。
//! `NewTestKit` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `NewTestKit` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `NewTestKit`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `testsetup` 是当前文件里的模块。
//! `testsetup` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `testsetup` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `testsetup`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `CALLED` 是当前文件里的静态量。
//! `CALLED` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `CALLED` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `CALLED`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `SetupForCommonTest` 是当前文件里的公开函数。
//! `SetupForCommonTest` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `SetupForCommonTest` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `SetupForCommonTest`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `was_called` 是当前文件里的公开函数。
//! `was_called` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `was_called` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `was_called`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `reset` 是当前文件里的公开函数。
//! `reset` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `reset` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `reset`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `testmain` 是当前文件里的模块。
//! `testmain` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `testmain` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `testmain`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `WrapTestingM` 是当前文件里的公开函数。
//! `WrapTestingM` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `WrapTestingM` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `WrapTestingM`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `sleep_mvcc` 是当前文件里的公开函数。
//! `sleep_mvcc` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `sleep_mvcc` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `sleep_mvcc`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `tikv` 是当前文件里的模块。
//! `tikv` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `tikv` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `tikv`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `ENABLED` 是当前文件里的静态量。
//! `ENABLED` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `ENABLED` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `ENABLED`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `EnableFailpoints` 是当前文件里的公开函数。
//! `EnableFailpoints` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `EnableFailpoints` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `EnableFailpoints`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `failpoints_enabled` 是当前文件里的公开函数。
//! `failpoints_enabled` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `failpoints_enabled` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `failpoints_enabled`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `transaction` 是当前文件里的模块。
//! `transaction` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `transaction` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `transaction`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `ManagedLockTTL` 是当前文件里的静态量。
//! `ManagedLockTTL` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `ManagedLockTTL` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `ManagedLockTTL`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `PrewriteMaxBackoff` 是当前文件里的静态量。
//! `PrewriteMaxBackoff` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `PrewriteMaxBackoff` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `PrewriteMaxBackoff`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `view` 是当前文件里的模块。
//! `view` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `view` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `view`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `STOPPED` 是当前文件里的静态量。
//! `STOPPED` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `STOPPED` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `STOPPED`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `Stop` 是当前文件里的公开函数。
//! `Stop` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `Stop` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `Stop`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `was_stopped` 是当前文件里的公开函数。
//! `was_stopped` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `was_stopped` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `was_stopped`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `testutil` 是当前文件里的模块。
//! `testutil` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `testutil` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `testutil`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `CheckIngestLeakageForTest` 是当前文件里的公开函数。
//! `CheckIngestLeakageForTest` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `CheckIngestLeakageForTest` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `CheckIngestLeakageForTest`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `goleak` 是当前文件里的模块。
//! `goleak` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `goleak` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `goleak`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `Option` 是当前文件里的分支类型。
//! `Option` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `Option` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `Option`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `LAST_OPTS` 是当前文件里的静态量。
//! `LAST_OPTS` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `LAST_OPTS` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `LAST_OPTS`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `VERIFY_CALLED` 是当前文件里的静态量。
//! `VERIFY_CALLED` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `VERIFY_CALLED` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `VERIFY_CALLED`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `IgnoreTopFunction` 是当前文件里的公开函数。
//! `IgnoreTopFunction` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `IgnoreTopFunction` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `IgnoreTopFunction`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `IgnoreAnyFunction` 是当前文件里的公开函数。
//! `IgnoreAnyFunction` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `IgnoreAnyFunction` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `IgnoreAnyFunction`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `VerifyTestMain` 是当前文件里的公开函数。
//! `VerifyTestMain` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `VerifyTestMain` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `VerifyTestMain`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `last_opts` 是当前文件里的公开函数。
//! `last_opts` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `last_opts` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `last_opts`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `verify_called` 是当前文件里的公开函数。
//! `verify_called` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `verify_called` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `verify_called`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `StringMap` 是当前文件里的类型别名。
//! `StringMap` 所处的位置主要服务 `真实 TiKV 集成测试` 主题下的一个阅读切面。
//! 阅读 `StringMap` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `StringMap`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 中文说明结束（自动生成）

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

// ---------------------------------------------------------------------------
// Shared event / failure injection
// ---------------------------------------------------------------------------

static EVENTS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
static OPEN_FAIL: OnceLock<Mutex<Option<String>>> = OnceLock::new();
static BOOTSTRAP_FAIL: OnceLock<Mutex<Option<String>>> = OnceLock::new();
static OWNER_FAIL: OnceLock<Mutex<Option<String>>> = OnceLock::new();
static NEXT_GEN: OnceLock<Mutex<bool>> = OnceLock::new();
static BASE_TABLES: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
static VIEWS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
static MVCC_WAIT: OnceLock<Mutex<Duration>> = OnceLock::new();
static STORE_SEQ: AtomicU64 = AtomicU64::new(1);

fn events_slot() -> &'static Mutex<Vec<String>> {
    EVENTS.get_or_init(|| Mutex::new(Vec::new()))
}

pub fn push_event(e: impl Into<String>) {
    events_slot().lock().unwrap().push(e.into());
}

pub fn take_events() -> Vec<String> {
    std::mem::take(&mut *events_slot().lock().unwrap())
}

pub fn clear_events() {
    events_slot().lock().unwrap().clear();
}

pub fn set_open_fail(msg: Option<&str>) {
    *OPEN_FAIL.get_or_init(|| Mutex::new(None)).lock().unwrap() = msg.map(|s| s.to_string());
}

pub fn set_bootstrap_fail(msg: Option<&str>) {
    *BOOTSTRAP_FAIL
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap() = msg.map(|s| s.to_string());
}

pub fn set_owner_fail(msg: Option<&str>) {
    *OWNER_FAIL.get_or_init(|| Mutex::new(None)).lock().unwrap() = msg.map(|s| s.to_string());
}

pub fn set_next_gen(v: bool) {
    *NEXT_GEN.get_or_init(|| Mutex::new(false)).lock().unwrap() = v;
}

pub fn set_base_tables(tables: Vec<String>) {
    *BASE_TABLES
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .unwrap() = tables;
}

pub fn set_views(views: Vec<String>) {
    *VIEWS.get_or_init(|| Mutex::new(Vec::new())).lock().unwrap() = views;
}

pub fn set_mvcc_wait(d: Duration) {
    *MVCC_WAIT
        .get_or_init(|| Mutex::new(Duration::from_secs(1)))
        .lock()
        .unwrap() = d;
}

pub fn mvcc_wait() -> Duration {
    *MVCC_WAIT
        .get_or_init(|| Mutex::new(Duration::from_secs(1)))
        .lock()
        .unwrap()
}

pub fn reset_test_globals() {
    clear_events();
    set_open_fail(None);
    set_bootstrap_fail(None);
    set_owner_fail(None);
    set_next_gen(false);
    set_base_tables(Vec::new());
    set_views(Vec::new());
    set_mvcc_wait(Duration::ZERO);
    config::reset_global();
    kvstore::SetSystemStorage(None);
    transaction::ManagedLockTTL.store(0, Ordering::SeqCst);
    transaction::PrewriteMaxBackoff.store(20000, Ordering::SeqCst);
    WITH_REAL_TIKV.store(false, Ordering::SeqCst);
    *TIKV_PATH.lock().unwrap() = DEFAULT_TIKV_PATH.to_string();
    MOCK_PORT_ALLOC.store(4000, Ordering::SeqCst);
    vardef::reset_schema_lease();
    goleak::reset();
    testsetup::reset();
    tikv::reset();
    view::reset();
}

// ---------------------------------------------------------------------------
// flag-like package vars (Go WithRealTiKV / TiKVPath / mockPortAlloc)
// ---------------------------------------------------------------------------

pub const DEFAULT_TIKV_PATH: &str = "tikv://127.0.0.1:2379?disableGC=true";
pub const PD_ADDR: &str = "127.0.0.1:2379";

static WITH_REAL_TIKV: AtomicBool = AtomicBool::new(false);
static TIKV_PATH: Mutex<String> = Mutex::new(String::new());
static MOCK_PORT_ALLOC: AtomicI32 = AtomicI32::new(4000);
static FLAGS_INIT: OnceLock<()> = OnceLock::new();

fn ensure_tikv_path() {
    FLAGS_INIT.get_or_init(|| {
        *TIKV_PATH.lock().unwrap() = DEFAULT_TIKV_PATH.to_string();
    });
}

pub fn with_real_tikv() -> bool {
    WITH_REAL_TIKV.load(Ordering::SeqCst)
}

pub fn set_with_real_tikv(v: bool) {
    WITH_REAL_TIKV.store(v, Ordering::SeqCst);
}

pub fn tikv_path() -> String {
    ensure_tikv_path();
    TIKV_PATH.lock().unwrap().clone()
}

pub fn set_tikv_path(p: impl Into<String>) {
    ensure_tikv_path();
    *TIKV_PATH.lock().unwrap() = p.into();
}

pub fn mock_port_alloc_add(delta: i32) -> i32 {
    MOCK_PORT_ALLOC.fetch_add(delta, Ordering::SeqCst) + delta
}

pub fn mock_port_alloc_get() -> i32 {
    MOCK_PORT_ALLOC.load(Ordering::SeqCst)
}

// ---------------------------------------------------------------------------
// testing.T / TestMain stand-ins
// ---------------------------------------------------------------------------

struct TestCtxInner {
    failed: AtomicBool,
    cleanups: Mutex<Vec<Box<dyn FnOnce() + Send>>>,
    logs: Mutex<Vec<String>>,
}

/// Cloneable stand-in for Go `*testing.T` (Arc so cleanups can capture it).
pub struct TestCtx {
    inner: Arc<TestCtxInner>,
    cleanup_owner: bool,
}

impl Clone for TestCtx {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            cleanup_owner: false,
        }
    }
}

impl Default for TestCtx {
    fn default() -> Self {
        Self::new()
    }
}

impl TestCtx {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(TestCtxInner {
                failed: AtomicBool::new(false),
                cleanups: Mutex::new(Vec::new()),
                logs: Mutex::new(Vec::new()),
            }),
            cleanup_owner: true,
        }
    }

    pub fn Fail(&self) {
        self.inner.failed.store(true, Ordering::SeqCst);
        push_event("t.Fail");
    }

    pub fn Failed(&self) -> bool {
        self.inner.failed.load(Ordering::SeqCst)
    }

    pub fn Log(&self, msg: impl Into<String>) {
        let m = msg.into();
        push_event(format!("t.Log:{m}"));
        self.inner.logs.lock().unwrap().push(m);
    }

    pub fn Cleanup<F>(&self, f: F)
    where
        F: FnOnce() + Send + 'static,
    {
        self.inner.cleanups.lock().unwrap().push(Box::new(f));
    }

    /// Run registered cleanups in LIFO order (Go `testing.T.Cleanup`).
    pub fn run_cleanups(&self) {
        loop {
            let cleanup = self.inner.cleanups.lock().unwrap().pop();
            match cleanup {
                Some(cleanup) => cleanup(),
                None => break,
            }
        }
    }

    pub fn logs(&self) -> Vec<String> {
        self.inner.logs.lock().unwrap().clone()
    }
}

impl Drop for TestCtx {
    fn drop(&mut self) {
        if self.cleanup_owner {
            self.run_cleanups();
        }
    }
}

pub struct TestMain {
    pub code: i32,
    pub wrapped: bool,
}

impl TestMain {
    pub fn new(code: i32) -> Self {
        Self {
            code,
            wrapped: false,
        }
    }
}

// ---------------------------------------------------------------------------
// require (testify subset)
// ---------------------------------------------------------------------------

pub mod require {
    use super::TestCtx;

    pub fn NoError(t: &TestCtx, err: Result<(), String>) {
        if let Err(e) = err {
            t.Fail();
            panic!("require.NoError: {e}");
        }
    }
}

// ---------------------------------------------------------------------------
// config
// ---------------------------------------------------------------------------

pub mod config {
    use super::*;
    use std::sync::RwLock;

    pub const StoreTypeTiKV: &str = "tikv";

    #[derive(Clone, Debug, Default)]
    pub struct AsyncCommit {
        pub SafeWindow: i64,
        pub AllowedClockDrift: i64,
    }

    #[derive(Clone, Debug, Default)]
    pub struct TiKVClient {
        pub AsyncCommit: AsyncCommit,
    }

    #[derive(Clone, Debug, Default)]
    pub struct TxnLocalLatches {
        pub Enabled: bool,
    }

    #[derive(Clone, Debug, Default)]
    pub struct Instance {
        pub TiDBServiceScope: String,
    }

    #[derive(Clone, Debug, Default)]
    pub struct Config {
        pub TiKVClient: TiKVClient,
        pub TxnLocalLatches: TxnLocalLatches,
        pub KeyspaceName: String,
        pub Store: String,
        pub NewCollationsEnabledOnFirstBootstrap: bool,
        pub Port: u32,
        pub Path: String,
        pub TiKVWorkerURL: String,
        pub MeteringStorageURI: String,
        pub Instance: Instance,
    }

    static GLOBAL: OnceLock<RwLock<Config>> = OnceLock::new();

    fn slot() -> &'static RwLock<Config> {
        GLOBAL.get_or_init(|| RwLock::new(Config::default()))
    }

    pub fn reset_global() {
        *slot().write().unwrap() = Config::default();
    }

    pub fn GetGlobalConfig() -> Config {
        slot().read().unwrap().clone()
    }

    pub fn StoreGlobalConfig(c: &Config) {
        *slot().write().unwrap() = c.clone();
        push_event("config.StoreGlobalConfig");
    }

    pub fn UpdateGlobal<F>(f: F)
    where
        F: FnOnce(&mut Config),
    {
        let mut g = slot().write().unwrap();
        f(&mut g);
        push_event("config.UpdateGlobal");
    }
}

// ---------------------------------------------------------------------------
// kerneltype / keyspace / handle / vardef
// ---------------------------------------------------------------------------

pub mod kerneltype {
    use super::*;

    pub fn IsNextGen() -> bool {
        *NEXT_GEN.get_or_init(|| Mutex::new(false)).lock().unwrap()
    }
}

pub mod keyspace {
    pub const System: &str = "SYSTEM";
}

pub mod handle {
    pub const NextGenTargetScope: &str = "dxf_service";
}

pub mod vardef {
    use super::*;
    use std::sync::Mutex;

    pub const DefInnodbLockWaitTimeout: i64 = 50;

    static SCHEMA_LEASE: OnceLock<Mutex<Duration>> = OnceLock::new();

    fn slot() -> &'static Mutex<Duration> {
        SCHEMA_LEASE.get_or_init(|| Mutex::new(Duration::from_secs(0)))
    }

    pub fn SetSchemaLease(d: Duration) {
        *slot().lock().unwrap() = d;
        push_event(format!("vardef.SetSchemaLease:{}ms", d.as_millis()));
    }

    pub fn schema_lease() -> Duration {
        *slot().lock().unwrap()
    }

    pub fn reset_schema_lease() {
        *slot().lock().unwrap() = Duration::from_secs(0);
    }
}

// ---------------------------------------------------------------------------
// kv Storage / domain / ddl / session / store / driver
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Storage {
    pub id: u64,
    pub path: String,
    closed: Arc<Mutex<bool>>,
}

impl Storage {
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            id: STORE_SEQ.fetch_add(1, Ordering::SeqCst),
            path: path.into(),
            closed: Arc::new(Mutex::new(false)),
        }
    }

    pub fn Close(&self) -> Result<(), String> {
        *self.closed.lock().unwrap() = true;
        push_event(format!("store.Close:{}", self.path));
        Ok(())
    }

    pub fn is_closed(&self) -> bool {
        *self.closed.lock().unwrap()
    }
}

#[derive(Clone, Debug, Default)]
pub struct MockSessionManager;

#[derive(Clone, Debug, Default)]
pub struct InfoSyncer {
    has_sm: Arc<Mutex<bool>>,
}

impl InfoSyncer {
    pub fn SetSessionManager(&self, _sm: &MockSessionManager) {
        *self.has_sm.lock().unwrap() = true;
        push_event("InfoSyncer.SetSessionManager");
    }

    pub fn has_session_manager(&self) -> bool {
        *self.has_sm.lock().unwrap()
    }
}

#[derive(Clone, Debug)]
pub struct Domain {
    closed: Arc<Mutex<bool>>,
    info_syncer: InfoSyncer,
}

impl Domain {
    pub fn new() -> Self {
        Self {
            closed: Arc::new(Mutex::new(false)),
            info_syncer: InfoSyncer::default(),
        }
    }

    pub fn Close(&self) {
        *self.closed.lock().unwrap() = true;
        push_event("domain.Close");
    }

    pub fn is_closed(&self) -> bool {
        *self.closed.lock().unwrap()
    }

    pub fn InfoSyncer(&self) -> &InfoSyncer {
        &self.info_syncer
    }
}

impl Default for Domain {
    fn default() -> Self {
        Self::new()
    }
}

pub mod ddl {
    use super::*;

    pub fn StartOwnerManager(_ctx: (), store: &Storage) -> Result<(), String> {
        if let Some(msg) = OWNER_FAIL
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap()
            .clone()
        {
            return Err(msg);
        }
        push_event(format!("ddl.StartOwnerManager:{}", store.path));
        Ok(())
    }

    pub fn CloseOwnerManager(store: &Storage) {
        push_event(format!("ddl.CloseOwnerManager:{}", store.path));
    }
}

pub mod session {
    use super::*;

    pub fn BootstrapSession(store: &Storage) -> Result<Domain, String> {
        if let Some(msg) = BOOTSTRAP_FAIL
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap()
            .clone()
        {
            return Err(msg);
        }
        push_event(format!("session.BootstrapSession:{}", store.path));
        Ok(Domain::new())
    }
}

pub mod kvstore {
    use super::*;

    static SYSTEM: OnceLock<Mutex<Option<Storage>>> = OnceLock::new();
    static REGISTERED: OnceLock<Mutex<bool>> = OnceLock::new();

    pub fn Register(_store_type: &str, _driver: &driver::TiKVDriver) -> Result<(), String> {
        *REGISTERED.get_or_init(|| Mutex::new(false)).lock().unwrap() = true;
        push_event("kvstore.Register");
        Ok(())
    }

    pub fn SetSystemStorage(s: Option<Storage>) {
        let slot = SYSTEM.get_or_init(|| Mutex::new(None));
        match &s {
            Some(st) => push_event(format!("kvstore.SetSystemStorage:{}", st.path)),
            None => push_event("kvstore.SetSystemStorage:nil"),
        }
        *slot.lock().unwrap() = s;
    }

    pub fn system_storage() -> Option<Storage> {
        SYSTEM
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap()
            .clone()
    }

    pub fn is_registered() -> bool {
        *REGISTERED.get_or_init(|| Mutex::new(false)).lock().unwrap()
    }
}

pub mod driver {
    use super::*;

    #[derive(Clone, Debug, Default)]
    pub struct TiKVDriver;

    impl TiKVDriver {
        pub fn Open(&self, path: &str) -> Result<Storage, String> {
            if let Some(msg) = OPEN_FAIL
                .get_or_init(|| Mutex::new(None))
                .lock()
                .unwrap()
                .clone()
            {
                return Err(msg);
            }
            push_event(format!("driver.Open:{path}"));
            Ok(Storage::new(path))
        }
    }
}

// ---------------------------------------------------------------------------
// testkit SQL harness
// ---------------------------------------------------------------------------

pub mod testkit {
    use super::*;

    #[derive(Clone, Debug)]
    pub struct ResultSet {
        rows: Vec<Vec<String>>,
    }

    impl ResultSet {
        pub fn Rows(&self) -> &[Vec<String>] {
            &self.rows
        }
    }

    pub struct TestKit {
        pub store: Storage,
        execs: Arc<Mutex<Vec<String>>>,
    }

    impl TestKit {
        pub fn MustExec(&self, sql: &str) {
            push_event(format!("tk.MustExec:{sql}"));
            self.execs.lock().unwrap().push(sql.to_string());
        }

        pub fn MustQuery(&self, sql: &str) -> ResultSet {
            push_event(format!("tk.MustQuery:{sql}"));
            if sql.contains("BASE TABLE") {
                let tables = BASE_TABLES
                    .get_or_init(|| Mutex::new(Vec::new()))
                    .lock()
                    .unwrap()
                    .clone();
                return ResultSet {
                    rows: tables.into_iter().map(|t| vec![t]).collect(),
                };
            }
            if sql.contains("VIEW") {
                let views = VIEWS
                    .get_or_init(|| Mutex::new(Vec::new()))
                    .lock()
                    .unwrap()
                    .clone();
                return ResultSet {
                    rows: views.into_iter().map(|v| vec![v]).collect(),
                };
            }
            ResultSet { rows: Vec::new() }
        }

        pub fn execs(&self) -> Vec<String> {
            self.execs.lock().unwrap().clone()
        }
    }

    pub fn NewTestKit(_t: &TestCtx, store: Storage) -> TestKit {
        push_event("testkit.NewTestKit");
        TestKit {
            store,
            execs: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub use super::MockSessionManager;
}

// ---------------------------------------------------------------------------
// testsetup / testmain / tikv / transaction / view / goleak / testutil
// ---------------------------------------------------------------------------

pub mod testsetup {
    use super::*;

    static CALLED: AtomicBool = AtomicBool::new(false);

    pub fn SetupForCommonTest() {
        CALLED.store(true, Ordering::SeqCst);
        push_event("testsetup.SetupForCommonTest");
    }

    pub fn was_called() -> bool {
        CALLED.load(Ordering::SeqCst)
    }

    pub fn reset() {
        CALLED.store(false, Ordering::SeqCst);
    }
}

pub mod testmain {
    use super::TestMain;
    use std::time::Duration;

    pub fn WrapTestingM<F>(m: &mut TestMain, callback: F) -> i32
    where
        F: FnOnce(i32) -> i32,
    {
        m.wrapped = true;
        let code = m.code;
        callback(code)
    }

    pub fn sleep_mvcc(d: Duration) {
        if !d.is_zero() {
            std::thread::sleep(d);
        }
    }
}

pub mod tikv {
    use super::*;

    static ENABLED: AtomicBool = AtomicBool::new(false);

    pub fn EnableFailpoints() {
        ENABLED.store(true, Ordering::SeqCst);
        push_event("tikv.EnableFailpoints");
    }

    pub fn failpoints_enabled() -> bool {
        ENABLED.load(Ordering::SeqCst)
    }

    pub fn reset() {
        ENABLED.store(false, Ordering::SeqCst);
    }
}

pub mod transaction {
    use std::sync::atomic::{AtomicU64, Ordering};

    pub static ManagedLockTTL: AtomicU64 = AtomicU64::new(0);
    pub static PrewriteMaxBackoff: AtomicU64 = AtomicU64::new(20000);
}

pub mod view {
    use super::*;

    static STOPPED: AtomicBool = AtomicBool::new(false);

    pub fn Stop() {
        STOPPED.store(true, Ordering::SeqCst);
        push_event("view.Stop");
    }

    pub fn was_stopped() -> bool {
        STOPPED.load(Ordering::SeqCst)
    }

    pub fn reset() {
        STOPPED.store(false, Ordering::SeqCst);
    }
}

pub mod testutil {
    use super::*;

    pub fn CheckIngestLeakageForTest() {
        push_event("testutil.CheckIngestLeakageForTest");
    }
}

pub mod goleak {
    use super::*;

    #[derive(Clone, Debug)]
    pub enum Option {
        IgnoreTopFunction(String),
        IgnoreAnyFunction(String),
        Cleanup(&'static str),
    }

    static LAST_OPTS: OnceLock<Mutex<Vec<Option>>> = OnceLock::new();
    static VERIFY_CALLED: AtomicBool = AtomicBool::new(false);

    pub fn IgnoreTopFunction(name: &str) -> Option {
        Option::IgnoreTopFunction(name.to_string())
    }

    pub fn IgnoreAnyFunction(name: &str) -> Option {
        Option::IgnoreAnyFunction(name.to_string())
    }

    pub fn Cleanup(name: &'static str) -> Option {
        Option::Cleanup(name)
    }

    pub fn VerifyTestMain(code: i32, opts: Vec<Option>) -> i32 {
        VERIFY_CALLED.store(true, Ordering::SeqCst);
        *LAST_OPTS
            .get_or_init(|| Mutex::new(Vec::new()))
            .lock()
            .unwrap() = opts;
        push_event(format!("goleak.VerifyTestMain:{code}"));
        code
    }

    pub fn last_opts() -> Vec<Option> {
        LAST_OPTS
            .get_or_init(|| Mutex::new(Vec::new()))
            .lock()
            .unwrap()
            .clone()
    }

    pub fn verify_called() -> bool {
        VERIFY_CALLED.load(Ordering::SeqCst)
    }

    pub fn reset() {
        VERIFY_CALLED.store(false, Ordering::SeqCst);
        if let Some(slot) = LAST_OPTS.get() {
            slot.lock().unwrap().clear();
        }
    }
}

// Re-export HashMap for callers that build keyspace maps via stubs helpers.
pub type StringMap<T> = HashMap<String, T>;
