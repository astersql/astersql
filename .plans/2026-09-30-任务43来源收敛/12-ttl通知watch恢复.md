# 任务 12: TTL 命令与扫描通知 watcher

批次：【批次 12】依赖批次：11

状态：未开始

目的：对齐 Go JobManager 对 timer 变更、命令和扫描任务通知的监听及断线恢复。

来源任务：43 当前任务文件的 TTL 完整生产链要求；Go `pkg/ttl/ttlworker/job_manager.go:jobLoopWithSession`

预计会话范围：仅处理 watcher 事件和重订阅；跨实例唯一执行的集成验证在下一任务。

## 文件

- 修改：`pkg/session/runtime/ttl_runtime.rs`、必要时 `pkg/ttl/ttlworker/job_manager.rs`。
- 测试：`pkg/session/runtime/ttl_runtime_test.rs`、必要时 `pkg/ttl/ttlworker/job_manager_test.rs`。
- Go 对照：`jobLoopWithSession` 的 `cmdWatcher`、`scanTaskNotificationWatcher`、`jobRequestCh` 与 timer ticker 分支。

## 上下文

- Go watcher 关闭后会重新订阅，收到命令/扫描通知时即时唤醒相关逻辑；当前 Rust 10 秒全表 tick 没有这两条事件输入。
- 复用已有通知客户端和通用 TimerStore Watch；若没有生产实现，先列出缺失的最小接口，不自行创造新协议。

## 测试计划

- 行为：远端命令和扫描完成通知及时唤醒 JobManager；watch 通道断开后恢复，关闭后不再处理通知。
- 失败验证测试：在 `ttl_runtime_test.rs` 新增 `go_merge_43_ttl_watch_reconnects_and_wakes_job_manager`，模拟唯一外部边界为断线/消息送达。
- 失败/通过命令：`CARGO_TARGET_DIR=/tmp/astersql-plan43-session cargo test --manifest-path pkg/session/Cargo.toml --lib go_merge_43_ttl_watch_reconnects_and_wakes_job_manager`
- 预期失败原因：当前 manager 没有对应 watch 输入或重订阅逻辑。
- 模拟策略：通知 transport 用可控 fake，作业状态、系统表和 timer runtime 保持真实；记录消息形状和副作用。

## 步骤

1. 先查找并复用已有 Rust command/notification 客户端，核对 Go 消息字段。
2. 添加断线及通知的失败回归。
3. 接入 watcher、重订阅、取消和关闭，保持单表事件流程可运行。
4. 验证没有忙轮询和后台线程泄漏。

## 完成

报告命令、扫描通知、断线恢复三类事件的触发时序和精确命令；缺失真实客户端时仅记录阻塞，不用新的私有协议冒充 Go 兼容。Ready 通过后删除本文件。
