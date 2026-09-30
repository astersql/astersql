# 任务 11: TTL timer Hook 提交与收尾

批次：【批次 11】依赖批次：10

状态：未开始

目的：用通用 timer runtime 的事件 Hook 驱动 TTL 作业提交、追踪及事件关闭。

来源任务：43 当前任务文件的 TTL 完整生产链要求；Go `pkg/ttl/ttlworker/timer.go` 和 `job_manager.go`

预计会话范围：仅接通 timer 到一个 TTL 作业的事件协议，不加命令/扫描通知 watcher。

## 文件

- 修改：`pkg/ttl/ttlworker/timer.rs`、`pkg/session/runtime/ttl_runtime.rs`、`pkg/session/runtime/ttl_timer.rs`。
- 测试：`pkg/ttl/ttlworker/timer_test.rs`、`pkg/session/runtime/ttl_runtime_test.rs`。
- 依赖：`pkg/timer/runtime/runtime.rs`、`pkg/timer/api/hook.rs`、任务 10 的真实 TableTimerStore 会话适配。

## 上下文

- Go `newTTLTimerRuntime` 按 TTL key 前缀过滤，注册 `timerHookClass` Hook；`OnSchedEvent` 通过 `ManagerJobAdapter` 提交请求，完成后 `CloseTimerEvent` 更新 watermark 与 summary。
- 当前 Rust `TtlTimerHook` 只是独立状态接口，生产 `run_ttl_tick` 仍每 10 秒扫描所有表并手写 EVENT_STATUS，不能据此称为 Go timer runtime 接线。

## 测试计划

- 行为：到期 timer 只提交关联物理表一次；作业完成后关闭同一 EventID、推进 watermark 和 summary；窗口外延迟、不误关新事件。
- 失败验证测试：在 `ttl_runtime_test.rs` 新增 `go_merge_43_ttl_runtime_hook_submits_and_closes_event`，真实 timer store 加一个 TTL 表，先观察没有 Hook 驱动。
- 失败/通过命令：`CARGO_TARGET_DIR=/tmp/astersql-plan43-session cargo test --manifest-path pkg/session/Cargo.toml --lib go_merge_43_ttl_runtime_hook_submits_and_closes_event`
- 预期失败原因：生产 Domain 当前只启动周期 tick，没有注册 timer Hook 或按 EventID 追踪。
- 模拟策略：使用真实 SQL、真实通用 timer runtime；时间可注入控制，避免等待实际日窗口。

## 步骤

1. 阅读 Go Hook 的 pre-schedule、on-schedule、wait-finished 和 timer client 关闭逻辑。
2. 先写单表失败回归，再实现 Hook/adapter 与 Domain 启停。
3. 去除与通用 runtime 冲突的手写 timer 触发/收尾 SQL，保留真正需要的 TTL 扫描执行。
4. 验证关闭时停止 Hook worker，不留下后台线程。

## 完成

报告 EventID、请求 ID、watermark、summary 和关闭顺序的真实 SQL 证据；只验证 `run_ttl_tick` 不足以完成。Ready 通过后删除本文件。
