# `pkg/executor/mview_delta_merge_builder.rs`

## 文件定位

FAST MV 刷新聚合 sink、快照重算与表 API 适配；由 builder::buildMViewDeltaMerge 调用。

## 核心职责

验证聚合依赖、固定快照和重算结果布局，计算聚合新值，执行真实表写入，登记 reader/merge worker/writer 统计。

## 主要符号

`Resources`、`ReaderSnapshot`、`LookupRequest`、`AggregateMapping`、`mappings`、`recompute_layout`、`DeltaMergeExec`、`TableMutationBatch`。

## 执行流程

build 校验 target 类型、COUNT(*) 与重算布局。Next 读取 chunk，保存旧行并合并聚合；必要的 MIN/MAX 重算通过 Resources 在 ReaderSnapshot 中执行。写入阶段比较存在性、最终 count 和 touched，分别 insert/delete/update/noop。成功 EOF 发布 affected rows 和语句消息；退出时登记已观察统计。

## 数据与状态

局部 MergeRuntimeStats 记录 reader 时间、一个真实同步 worker 的处理时间、writer 时间和实际写入计数。chunks/row_ops 对应非空写入输入；noop 不计入影响行数。键以 Vec<Datum> 保存，各列 Kind、NULL、长度及内容独立保留；same 逐列匹配，无拼接或可复用序列化缓冲，不存在前缀被预分配覆盖的路径。

## 依赖与调用关系

Resources 继承 MutationRuntime，提供表字段、重算 reader 和可选 `(plan_id, Arc<StatementContext>)`。None 表示调用方未请求语句记账；Some 下真正使用 StatementContext/RuntimeStatsColl。仓库现有资源接口的 SQL 会话适配并非本提交新增完整子系统，本文件不假定全 SQL MV runtime 已接入。

## 错误处理与边界

拒绝负 COUNT、溢出、错误类型/布局、重复或意外重算键、缺失快照；child/recompute/write 错误传播。错误时仍登记已观察统计，但不更新成功语句结果。TableMutationBatch 委托真实 Table API，事务回滚由资源持有者保证。

## 并发与资源生命周期

当前 Rust pipeline 为同步单 worker，统计 total=1；不能按 Go configured worker 数虚构并发。局部计数不共享写入；收集器通过 RegisterStatsShared 加锁注册。每次执行创建新统计，Close 不保留旧统计。

## 与 Go 版本的对应关系

37a4cd4f439b97980a42d64d23c126933ba1f7be exec.go 的 Next/Close、pipeline 接线、reader/worker/writer 计时、writer.go setRuntimeStats；本路径是 Rust 真实所有者。键修复的可观察契约由既有结构化 Datum 键满足，以原矩阵和真实重算验证，未新增无用途的序列化副本。

## 扩展指南

新增并行 worker 时按实际 worker 分槽计时并在 join 后注册；键编码若改为字节，须保留逐列 NULL 与变长边界并增加复用缓冲回归。

## 验证依据

`port_37a4cd4f43_` 聚焦测试覆盖真实 FAST 写入、复合 NULL/空串键重算、原 Go FAST/COMPLETE 结果和快照；原有 mview_delta_merge_* 测试保护布局、快照状态恢复和回滚。结果记录于提交正文。
