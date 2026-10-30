# `pkg/session/runtime/mview_refresh.rs` 逻辑说明

## 文件定位

物化视图刷新 runtime 协调器；来源阶段通过直接构造器调用，公开 SQL builder 由后续提交接入。

## 核心职责

协调四种刷新模式、独立历史会话、水位、真实悲观事务、shadow DDL/cutover、失败历史、锁与 panic 清理、内部统计和耗时日志。

## 主要符号

`RefreshMaterializedViewExec`、`RefreshTimings`、`InternalCollectorGuard`、`SessionInitGuard`、`TaskMonitor`、`ShadowGuard`。

## 执行流程

校验目标与基表权限，初始化维护会话；获取 advisory lock 与 refresh info 行锁；插入 running 历史；运行对应数据变更；CAS 水位并提交或持久 DDL cutover；更新历史与告警。错误路径 rollback、清理 shadow、补失败历史并释放锁。

## 数据与状态

done 标记保证重复 Next 无副作用；history 与 maintenance 使用不同真实会话；watermark 使用 TSO；timings 保留总耗时、执行和模式特定阶段；影子创建/加载/cutover 的计时在 panic 时仍记录，事务计时仅覆盖实际事务区间。

## 依赖与调用关系

调用 mview_source、既有 merge/apply sink、SystemSessionPool 和持久 DDL action 92/93/94；Domain collector 持有全局聚合，会话 guard 管理局部条目。

## 错误处理与边界

捕获 panic 后执行清理；成功 cutover 在 observer end 前解除 shadow 清理责任。历史 finalize 重试一次；锁只在实际获取后释放。初始化 guard 恢复真实 SQLMode/timezone/precision 与维护参数；失败历史保留取消请求者，rollback/cleanup 错误附加原失败。15 项真实 runtime 回归覆盖成功、失败和恢复路径。

## 并发与资源生命周期

TaskMonitor 使用后台独立会话记录 heartbeat/取消，Drop 唤醒并 join；维护会话变量由 guard 恢复，嵌套 collector 保留已有身份且只解绑自己创建的条目。

## 与 Go 版本的对应关系

FAST 成功历史的 REFRESH_ROWS 使用实际 MLog iterator 计数；关闭执行信息采集时保持 NULL。派生源的数据执行通过既有 SessionBoundAdapterOwner 的 SlowQuery 路径记录恢复后的内部实现语句文本。计划行在数据阶段开始与结束回调之间发出，先捕获实际派生源计划，避免 MIN/MAX 回查覆盖该计划。未持有 canonical plan 的既有 legacy INSERT/DELETE/IMPORT 路径按 Go 的 nil-plan 条件跳过计划行；不生成虚假 runtime 计数。

对应 a87bf8db6499ac38075781cef7357709f07ea002 的 mview_refresh.go、mview_refresh_steps.go 与 session.go 内部 collector 行为；保留公开 builder 在后续提交接入的阶段边界。

## 扩展指南

扩展时保持直接构造器的来源阶段边界，公开 REFRESH SQL dispatcher 由后续来源提交接入。继续复用已有查询/DDL/统计设施；typed 执行链的通用 EXPLAIN runtime 明细属于独立基础设施，不在本增量递归补建。
