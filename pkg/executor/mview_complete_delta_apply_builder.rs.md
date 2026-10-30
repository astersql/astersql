# `pkg/executor/mview_complete_delta_apply_builder.rs`

## 文件定位

COMPLETE DELTA APPLY 的真实执行器构建与 Chunk 消费入口；由 builder::buildMViewCompleteDeltaApply 调用。

## 核心职责

校验 writable 映射、保留分组键与 handle、执行真实原子批量写入，并在计划下登记运行统计。

## 主要符号

`TargetColumns`/`TargetColumn`、`Mappings`、`mappings`、`build`、`CompleteDeltaApplyExec`。统计类型定义在 mview_complete_delta_apply.rs。

## 执行流程

build 先校验表与源字段，编译 current/recomputed/compare 映射。Open 打开 child；首次 Next 消费所有 child chunks，在 run_atomic 回调内逐行调用 insert/delete_from_source/update_from_source。仅变化的非分组列触发 update。成功 EOF 设置语句影响行数和消息；无论成功失败均向启用的 RuntimeStatsColl 注册当前统计快照。

## 数据与状态

opened/done 控制生命周期。每次实际执行创建零值统计，避免复用旧计数；非空 chunk 增加 chunks/row_ops，只有成功的表调用增加对应 insert/update/delete；计时只在统计开启且 chunk 写入成功后累计。失败时保留已观察的表调用计数，语句完成结果不发布。

## 依赖与调用关系

依赖 planner 的 MViewCompleteDeltaApply、FieldType、Datum、Chunk、MutationRuntime 与 Resources。Resources::refresh_context 返回真实 plan ID 和 StatementContext；该接口为现有资源边界的必要统计接线，不创建新的事务/会话子系统。

## 错误处理与边界

映射长度/字段类型、非 public writable 列、NULL 或未知 op 以及 child/比较/写入错误均传播。run_atomic 的适配者负责提交与回滚；统计登记不能替代数据回滚。

## 并发与资源生命周期

当前 sink 同步消费与写入；StatementContext 及收集器由 Arc 共享，RegisterStatsShared 内部加锁。Close 关闭 child，统计在 Next 结束时转移给收集器，执行器不持有跨语句统计指针。

## 与 Go 版本的对应关系

37a4cd4f439b97980a42d64d23c126933ba1f7be 的 mview_complete_delta_apply.go：Next reset/register、成功 chunk writerTime、writerDetail.merge 与 Close 清理。Rust 使用每次执行的局部统计值实现相同复位与所有权契约。列比较助手后续行为沿用既有实现，未恢复旧 Go 实现。

## 扩展指南

扩展写入路径时在成功表调用之后计数；错误不能设置成功语句消息。新增测试置于同目录独立测试文件。

## 验证依据

`port_37a4cd4f43_complete_publishes_actual_writes_and_noops` 和刷新操作器组合测试调用本 build/Next；原有 mapping、回滚和 context 测试共同保护既有行为。确切验证命令与结果保存在本来源提交正文。
