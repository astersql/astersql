# execdetails 内部核心类型

## 文件定位

`pkg/util/execdetails/internal/group1/lib.rs` 是 `astersql-util-execdetails-group1` 的业务入口；通过相对路径挂载共享 execdetails 和 runtime_stats 实现，供上层 crate 统一复用核心类型。

## 核心职责

提供时间、扫描、RPC、提交、锁和 RU 明细，执行摘要兼容类型及百分位样本统计。它拥有真实扫描证据的数据结构与合并行为，并非纯模块装配。

## 主要符号

`util::ScanDetail` 保存 ProcessedKeys、ProcessedKeysSize、TotalKeys 和 TotalKeysSize，以及 RocksDB/IaCache 统计；`Merge` 合并各字段。`TimeDetail`、`PoolTaskDetails`、`CommitDetails`、`LockKeysDetails`、`RUDetails` 保存各类执行证据。`tipb::ExecutorExecutionSummary` 和 TiFlash 摘要类型支持运行时摘要消费；`Percentile` 累积样本并计算分位值。

## 执行流程

调用者先构造值明细，再通过 Merge 合并到请求或语句聚合器。ScanDetail.TotalKeysSize 原样累计后端总字节，零表示未提供；消费方 runtime_stats::EstimateScanBytes 和执行器分类器负责选择后端总字节或比例回退。

## 数据与状态

ScanDetail 使用有符号 i64 暴露计数。TotalKeysSize 的 wrapping_add 保留 Go 的机器整数合并行为，负值由消费方拒绝；不能用 ProcessedKeysSize 覆盖它。Clone 生成独立值快照。

## 依赖与调用关系

标准库提供时间、互斥锁和原子。group1 重导出 execdetails_impl、runtime_stats_impl，distsql、executor 和 session 通过上层 re-export 使用相同 ScanDetail，避免不同 crate 的统计类型脱节。

## 错误处理与边界

明细合并不返回业务错误；有效性由后续分类决定。零计数不等于完整响应覆盖。TotalKeysSize 不参与 String 的既有显示格式，避免诊断文本变化。

## 并发与资源生命周期

ScanDetail 通过独占可变引用合并，无内部锁；调用者负责同步。其他提交/锁明细的共享字段使用既有互斥或原子结构。所有快照都是拥有值，不持有 RPC 响应引用。

## 与 Go 版本的对应关系

对应 Go execdetails 使用的 client-go util 明细和本地 runtime stats 核心类型。来源 `74dc8d6e8722608c8067d975a9dea84662c60a5d` 在消费路径新增 TotalKeysSize；本文件新增同名字段与合并是不可缺少的本地接线。真实 point RPC 统计由已发布的 client-rust v0.4.2-aster.16 提供，不在本文件复制上游代码。

## 扩展指南

新增证据字段时同时检查生产响应转换、Merge、快照和分类器，并用独立测试覆盖缺失值、负值及后端报告值。保留已有显示格式和所有权边界，不把校准覆盖证明等同于单个计数存在。
