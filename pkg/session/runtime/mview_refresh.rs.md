# `pkg/session/runtime/mview_refresh.rs` 逻辑说明

## 文件定位

物化视图刷新 runtime 协调器；既可通过直接构造器调用，也由 dispatch 的真实 REFRESH SQL 入口调用。

## 核心职责

协调四种刷新模式、独立历史会话、水位、真实悲观事务、shadow DDL/cutover、失败历史、锁与 panic 清理、内部统计和耗时日志。

## 主要符号

`ConcreteSession::execute_refresh_sql`、`refresh_dry_plans`、`RefreshObserveCollector`、`format_observe_plan`、`format_observe_profile_plan`、`analyze_refresh_plan_rows`、`RefreshMaterializedViewExec`、`RefreshTimings`、`InternalCollectorGuard`、`SessionInitGuard`、`TaskMonitor`、`ShadowGuard`。

## 执行流程

63ec01b244 的迁移回归补充：开始执行时清零本语句结果；锁定刷新水位后另取实际读 TSO，避免遗漏事务开始后、锁定读取前提交的数据。restricted SQL 保留 automatic 标记。bounded MIN/MAX 的非空输入通过独立历史会话读取 GC safe point，纯增量聚合与已证明空输入不要求旧基表快照。历史时长由持久 start/end 时间计算；影子构建保留 u64 TSO，TiFlash 旧进度仅在 cutover 提交成功后清理。

语句结果同时清理旧 `last_dml_report`，避免 no-op 再次发布前一条 DML 的 affected rows。历史收尾在 Rust 中按固定微秒 wall-clock 起止时间计算精确六位小数，再写入同一 UPDATE，保持 Go TIMESTAMPDIFF 等式；不向通用 DML 表达式系统补建时间单位 AST 支持。`check_materialized_shadow_access` 由 dispatch 前置调用，复用真实表元数据，保护用户写入、DDL 与 JOIN 读访问；restricted 维护语句仍可操作自己的 shadow。

维护会话保留 Go 的整组 execution settings 与单变量 apply 故障点：手动刷新直接报错；automatic 模式记录警告并跳过失败项，其他变量继续应用。RAII guard 同时恢复已成功应用的原值，测试覆盖六种组合。

CREATE_SHADOW 的观察回调之后、DDL 提交之前再次检查真实 monitor 的取消状态，防止收到取消后仍创建再删除 shadow。锁释放仍实际执行 RELEASE_LOCK；原 Go count fault point 只覆盖结果检查以复现提交后的清理异常，不能把已经提交的刷新数据回滚或追加 failed 历史。

FAST 数据阶段把可变 delta 计划传给 `compile_mview_source_with_empty`；该编译取得真实优化器输出后更新 empty 标志，再构建同一个现有 DeltaMergeExec。空刷新仍运行事务/水位 CAS/提交/历史收尾，只有读合并写管线被省略。

05fd982b2d SQL 入口在 MySQL 方言内检查事务、异步模式、AS OF 模式、OPERATE VIEW；观察模式还先检查 SHOW VIEW，然后检查基表 SELECT。普通语句运行既有协调器；DRY RUN 使用新的维护会话初始化定义环境并编译真实关系源并标识既有写入 sink，不打开源执行器，不写历史、水位或 shadow；PROFILE 运行真实刷新并按步骤回调收集时间、错误和计划。所有结果通过 ConcreteRecordSet 返回；Next/recordset 生命周期由既有会话框架管理。

校验目标与基表权限，初始化维护会话；获取 advisory lock 与 refresh info 行锁；插入 running 历史；运行对应数据变更；CAS 水位并提交或持久 DDL cutover；更新历史与告警。错误路径 rollback、清理 shadow、补失败历史并释放锁。

## 数据与状态

done 标记保证重复 Next 无副作用；history 与 maintenance 使用不同真实会话；watermark 使用 TSO；timings 保留总耗时、执行和模式特定阶段；影子创建/加载/cutover 的计时在 panic 时仍记录，事务计时仅覆盖实际事务区间。

## 依赖与调用关系

调用 mview_source、既有 merge/apply sink、SystemSessionPool 和持久 DDL action 92/93/94；Domain collector 持有全局聚合，会话 guard 管理局部条目。

## 错误处理与边界

取消刷新任务使用最新 Domain InfoSchema 解析历史行中的目标与 schema；缺少系统历史表、无法解析目标及 schema 时保留来源的具体错误文本，已请求的 running job 仍先检查 OPERATE VIEW 再由条件 UPDATE/affected rows 判重复。无用户身份的内部调用保持权限检查跳过语义。

05fd982b2d 的 PROFILE 从真实 typed 算子的 Open/Next/Close 采样 actRows、循环次数与耗时，沿执行树共享 RuntimeStatsColl；行数来自返回 Chunk，既不使用计划估算也不使用树节点数。legacy INSERT/DELETE/IMPORT 仍执行原 SQL 路径，根写入阶段统计来自真实 Execute/RecordSet drain 的输出行数与耗时；没有执行 typed 子树的静态源行不冒充已采样。无算子内存/磁盘 tracker 时保留 N/A，不伪造计数。

捕获 panic 后执行清理；成功 cutover 在 observer end 前解除 shadow 清理责任。历史 finalize 重试一次；锁只在实际获取后释放。初始化 guard 恢复真实 SQLMode/timezone/precision 与维护参数；失败历史保留取消请求者，rollback/cleanup 错误附加原失败。15 项真实 runtime 回归覆盖成功、失败和恢复路径。

## 并发与资源生命周期

TaskMonitor 使用后台独立会话记录 heartbeat/取消，Drop 唤醒并 join；维护会话变量由 guard 恢复，嵌套 collector 保留已有身份且只解绑自己创建的条目。

## 与 Go 版本的对应关系

对应 63ec01b2440281b0debbfb05c5dfd378e94474d1 的 planner→executor MLogEmpty 传递在实际 Rust 会话所有者中的最小接线；公开 CREATE MATERIALIZED VIEW 既有 SQL 入口缺口不在本来源修复。

FAST 成功历史的 REFRESH_ROWS 使用实际 MLog iterator 计数；关闭执行信息采集时保持 NULL。派生源的数据执行通过既有 SessionBoundAdapterOwner 的 SlowQuery 路径记录恢复后的内部实现语句文本。计划行在数据阶段开始与结束回调之间发出，先捕获实际派生源计划，避免 MIN/MAX 回查覆盖该计划。观察模式下 INSERT/DELETE/shadow load 通过同一维护会话编译真实 SELECT 物理输入，显示运行时实际使用的写入 sink 与物理源计划，避免通用 legacy EXPLAIN 的占位输出；merge/apply 收集实际编译源与对应 sink 算子。不生成虚假 runtime 计数。刷新提交之后重新建立 DML report，将真实 affected rows 送入协议层，避免 COMMIT 的语句重置丢失报告。

对应 a87bf8db6499ac38075781cef7357709f07ea002 的 mview_refresh.go、mview_refresh_steps.go 与 session.go 内部 collector 行为；05fd982b2d43072817c809fb66bc1155aff236e8 的 builder.go、mview_refresh_executor_builder.go、mv_refresh_observability.go 与 session.go 增量在现有 ConcreteSession 所有者处接线，不复制一个未使用的 executor 模块。Go 线程/系统会话池由现有 Rust 独立维护会话和 RAII guard 对应。

## 扩展指南

扩展时保持 MySQL 方言边界和 DRY RUN 无副作用契约；不要在观察路径运行真实刷新。继续复用已有查询/DDL/统计设施；typed 执行链的通用 EXPLAIN runtime 明细属于独立基础设施，不在本增量递归补建。
