# `pkg/executor/mviewdeltamergeagg/mod.rs`

## 文件定位

MV 聚合运算与 FAST 刷新专项统计类型，真实 sink 在 mview_delta_merge_builder.rs。

## 核心职责

校验 COUNT/SUM/MIN/MAX 映射、合并行值，并提供可合并、可克隆的 MergeRuntimeStats。

## 主要符号

`AggKind`、`Mapping`、`MinMaxRecompute`、`validate`、`numeric_add`、`merge_rows`、`MergeRuntimeStats`。

## 执行流程

merge_rows 先校验依赖与类型，再逐行更新聚合值和 touched。MergeRuntimeStats::new/reset 初始化 worker 槽；String 汇总正耗时 worker 的 total/active/min/max/avg，附 reader 和 writer 时间/计数；Merge 按 worker 索引累加并扩长向量。

## 数据与状态

统计持有 reader_time/writer_time、独立 Vec<Duration> 和 RefreshWriterStats。CloneBox 深克隆 worker 向量；负 worker 数按零处理。reset 清空时间与计数，保留可用容量。

## 依赖与调用关系

使用 Datum、MyDecimal 与 RuntimeStats/FormatDuration；writer 明细由 mview_complete_delta_apply.rs 共享。DeltaMergeExec 构造并注册此类型，Tp 使用 execdetails 的 MV delta merge 专用编号。

## 错误处理与边界

COUNT 负数、SUM 溢出/类型不匹配、MIN/MAX 缺失 recompute 等仍按既有路径报错。Merge 遇到其他 RuntimeStats 类型不修改自身；空 worker 无 min/max/avg。

## 并发与资源生命周期

聚合调用由现有 sink 控制；统计值在线程/worker 完成后由拥有者登记，结构自身无内部可变共享引用。Rust 当前真实 sink 单 worker，不声称实现 Go goroutine 调度模型。

## 与 Go 版本的对应关系

聚合运算为既有 Rust。新增统计对应 37a4cd4f439b97980a42d64d23c126933ba1f7be exec.go 的 mergeRuntimeStats/new/reset/String/Clone/Merge/Tp；仅保留本来源统计增量。

## 扩展指南

增加统计字段时同步 String/Clone/Merge/reset；修改聚合规则需另行映射所属来源，不扩大本提交。

## 验证依据

独立 executor_pkg_test.rs 的 port_37a4cd4f43_merge_stats_string_merge_clone_reset 对照 Go TestMergeRuntimeStats 并补独立克隆、不同 worker 长度和零/负 worker 边界；真实 sink 测试证明类型实际注册。
