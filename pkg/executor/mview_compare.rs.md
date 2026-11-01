# `pkg/executor/mview_compare.rs` 逻辑说明

## 文件定位

COMPARE MATERIALIZED VIEW 的 typed full-join 差异分类辅助，由 canonical session 真实调用。

## 核心职责

以 visible NOT NULL marker 区分缺失侧，比较非分组 payload；确定输出差异列名和分批字节上限。

## 主要符号

CompareLayout、classify、payload_equal、differ_type_column_name、output_batch_bytes。

## 执行流程

按 recomputed 缺失、current 缺失、payload 改变的顺序分类；vacuum 选择重算侧，其余选择当前 MV 侧。

## 数据与状态

joined 的形状是 current + recomputed，columns 是每侧宽度；group_keys 不参与 payload 比较。

## 依赖与调用关系

session/runtime/mview_compare.rs 驱动 typed_hash_join，调用本文件分类。payload_equal 对 DECIMAL 调用 Compare 忽略内部 padding，其他类型复用 Datum::Equals 的二进制值语义。

## 错误处理与边界

joined 宽度或 marker 越界返回错误；NULL payload 用 Datum 的相等语义。输出列名不区分大小写避让。

## 并发与资源生命周期

辅助函数不持有会话、事务或线程。join 与输出表生命周期归 session 中的清理守卫负责。

## 与 Go 版本的对应关系

对应 Go 59bfe3daabe4af4088631bb5094d0a67f204f613 的 buildCompareMaterializedViewDiffLayout、classifyCompareMaterializedViewRowDiff、chooseCompareMaterializedViewDifferTypeColumnName 和 batch byte resolver；Go 的并行 join worker 由已有 typed join 实现承担。

## 扩展指南

新增比较类型先验证同一 typed Datum 入口和原 Go NULL 分支；不要用格式化字符串代替 typed 相等。
