# 任务 13: TTL 双实例调度与故障接续

批次：【批次 13】依赖批次：10,11,12

状态：未开始

目的：证明两个 TTL manager 共享系统表时，timer 事件和作业只被有效 owner 执行，并能接续失败实例。

来源任务：43 当前任务文件的 TTL 分布式生产验证

预计会话范围：集成验证及必要的小范围修复；不再增加新调度架构。

## 文件

- 修改：仅验证暴露缺陷的 `pkg/session/runtime/ttl_runtime.rs`、`pkg/ttl/ttlworker/timer.rs` 或 `pkg/timer/tablestore/store.rs`。
- 测试：`pkg/session/runtime/ttl_runtime_test.rs`。
- Go 对照：`pkg/ttl/ttlworker/job_manager.go` 的 owner 判断、timer event 和 timeout takeover；`timer.go` 的 EventID 追踪。

## 上下文

- 现有单实例 `run_ttl_tick` 测试覆盖持久化认领、心跳、超时接管，但不能证明两个实例的 timer watch 和唯一提交。
- 用户允许模拟 Region 边界；Owner/SQL 系统表/TimerStore 仍应共享真实状态，不能用两个完全独立 mock 证明唯一性。

## 测试计划

- 行为：两个实例同时收到同一到期 timer 只产生一项持久作业；第一实例失主/关闭后第二实例按原扫描水位接续；事件 ID 与 summary 不错配。
- 失败验证测试：在 `ttl_runtime_test.rs` 新增 `go_merge_43_ttl_two_managers_single_event_and_takeover`，先运行观察重复或无法接续。
- 失败/通过命令：`CARGO_TARGET_DIR=/tmp/astersql-plan43-session cargo test --manifest-path pkg/session/Cargo.toml --lib go_merge_43_ttl_two_managers_single_event_and_takeover -- --nocapture`
- 预期失败原因：当前独立周期 tick 和 timer Hook 的 ownership/通知边界未经过双实例竞争验证。
- 模拟策略：仅模拟 Region 切分和故障时点；两个 manager 共用实际 SQL 系统表与 timer notifier。若能使用隔离 PD/TiKV/etcd 再补真实集成证据，并显式报告环境。

## 步骤

1. 建立两个 Domain/manager 实例和共享 SQL、timer state；先运行失败验证。
2. 只修复实际暴露的 claim、心跳、关闭或事件过滤缺陷。
3. 复跑双实例测试并检查持久状态、所有后台资源关闭。
4. 核对 Region 模拟输入与 Go 物理分区/范围规则，记录真实网络环境是否另行验证。

## 完成

报告唯一作业、原水位接续、EventID/summary、关闭清理的数据库证据及命令；若只用模拟 Region，明确真实多 Region 未验证。Ready 通过后删除本文件。
