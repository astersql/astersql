# `pkg/util/ranger/row_count_bound.rs`

## 文件定位

唯一键行数上界证明，供 IndexReader 小 Limit 请求选择并发。

## 核心职责

对每个逻辑范围证明固定列覆盖至少一项真实唯一键；成功返回范围数量，否则 (0,false)。

## 主要符号

RowCountUpperBoundByUniqueKey(TypeCtx, Option<TableInfo>, Column[], lengths[], Range[])。

## 执行流程

收集整数 handle 主键以及 public、完整、无 partial 条件、非多值的唯一/主索引；逐范围比较前导相等端点，检查固定列覆盖。

## 数据与状态

仅局部 HashSet 保存每个范围的固定列 ID。范围、类型上下文和表元数据均不修改。

## 依赖与调用关系

复用 expression::Column、model::TableInfo、现有 Range、Datum.Compare 和 Collator；由 executor 的 IndexReaderScan 和 session 覆盖索引请求构造消费。

## 错误处理与边界

缺表、空范围、列长不一致返回无界。NULL 与前缀列不加入固定集；无穷端点或比较失败停止前导收集。全相等但开端点不是点，返回无界；后列是区间仍可用前导唯一键。

## 并发与资源生命周期

无共享可变状态，无 I/O、线程或外部资源。逐范围复用局部集合。

## 与 Go 版本的对应关系

逐项对应来源 e48ac5fef19ea30a3cae73606ca8ad27c538e7f3 的 row_count_bound.go；Option 表达 nil table，借用切片表达原切片。不是行数估计器，不能将范围数量直接当上界。

## 扩展指南

扩展唯一键或范围表示时保留 NULL、公有状态、prefix、partial、MV 和开闭端点测试，不放宽证明条件来匹配估计行数。
