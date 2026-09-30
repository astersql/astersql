# 任务 10: TTL timer 表存储的真实会话适配

批次：【批次 10】依赖批次：1,2

状态：未开始

目的：让现有通用 `TableTimerStore` 使用真实 TTL 系统 SQL 会话访问 `mysql.tidb_timers`。

来源任务：43 当前任务文件的 TTL 完整生产链要求；Go `pkg/ttl/ttlworker/job_manager.go:jobLoopWithSession`

预计会话范围：仅构造 `syssession::Pool` 到真实 SQL Session 的适配，并证明 timer CRUD/watch；不连接作业调度 Hook。

## 文件

- 新建或修改：`pkg/session/runtime/ttl_timer_store.rs`、`pkg/session/runtime.rs`、`pkg/session/Cargo.toml`。
- 测试：`pkg/session/runtime/ttl_timer_store_test.rs`。
- 现有依赖：`pkg/timer/tablestore/store.rs`、`pkg/session/syssession/session.rs`、`pool.rs`、`pkg/session/runtime/ttl_worker_session.rs`。
- Go 对照：`pkg/ttl/ttlworker/job_manager.go` 创建 `tablestore.NewTableTimerStore(1, sessPool, "mysql", "tidb_timers", etcd)`。

## 上下文

- Rust 通用 Timer API、TableTimerStore、runtime 已存在；TableTimerStore 要求 `syssession::Pool` 返回可执行带参数 SQL 且能解码 `SqlResult` 的 `SessionContext`。
- `ConcreteSession` 非 `Send`，适配必须保持线程亲和与事务亲和；现有 `CrossKSSessionPool` 只是可参考的做法，不能直接当 TTL 策略来源。
- 先以真实 timer SQL 验证当前解析器是否支持所需语法；若不支持，明确最小缺口，不用假 row 使 CRUD 测试通过。

## 测试计划

- 行为：在真实系统表中 Create→List→Update→Watch→Delete，另一个会话能看到持久化状态，关闭 pool 后不泄漏线程。
- 失败验证测试：在 `ttl_timer_store_test.rs` 新增 `go_merge_43_ttl_table_timer_store_real_sql_roundtrip`，先观察当前无生产适配或 SQL 语法失败。
- 失败/通过命令：`CARGO_TARGET_DIR=/tmp/astersql-plan43-session cargo test --manifest-path pkg/session/Cargo.toml --lib go_merge_43_ttl_table_timer_store_real_sql_roundtrip`
- 预期失败原因：`TableTimerStore` 当前没有真实 `syssession::Pool` 接线，或真实 SQL 对 timer 参数/返回类型处理不完整。
- 模拟策略：本地 Domain 与真实 SQL 系统表；etcd notifier 可先用现有内存 notifier，下一任务验证跨实例通知。

## 步骤

1. 用真实 SQL 做最小可行性回归，记录不支持的语法和列类型。
2. 实现线程亲和的 `SessionContext` 和参数/结果转换；禁止静默吞错。
3. 跑失败→通过回归，检查事务复用和关闭。
4. 若缺失 mysqlcompat 清单阻止 Session 测试编译，先查清来源并记录阻塞，不把临时空清单当正式资产。

## 完成

提供真实 SQL CRUD/watch 行和关闭证据；只有 fake store 测试不能标记完成。Ready 通过后删除本文件。
