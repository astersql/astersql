# `pkg/planner/core/mview_refresh_lookup.rs`

## 文件定位

筛选前缀覆盖 group keys 的可见索引；把 lookup 输出的 MV offsets 重排到 inner schema 顺序；校验并保存 inner plan、索引 range 与 key 映射。

## 核心职责

筛选前缀覆盖 group keys 的可见索引；把 lookup 输出的 MV offsets 重排到 inner schema 顺序；校验并保存 inner plan、索引 range 与 key 映射。

## 主要符号

`MViewIndex` 保存 name/columns/visible。`FindVisibleIndexesWithPrefixCoveringColumns` 返回所有匹配名称。`MViewFullUpdateLookupTemplate` 拥有 inner_source、列数、ranges、key 到 index/result 的映射及 output_mv_offsets。`DeriveMViewFullUpdateOutputMVOffsetsByInnerSchema` 重排输出，`ExtractMViewFullUpdateLookupTemplate` 校验并构造模板。

## 执行流程

索引筛选先将 group columns ASCII 小写并去重为集合，对每个可见索引的前 N 列做集合相等判断，允许 key 顺序不同，保留输入索引顺序。重排先把每个 group key MV offset 放到对应 key_result_col_indexes 位置，再将非 key lookup offsets 按原序填入未占用槽位。Extract 依次校验非空 schema/ranges、映射长度、key index offset 范围，调用重排，最后要求传入节点自身为 `PlanKind::IndexJoin`。

## 数据与状态

所有映射均为 usize，未填槽位用 usize::MAX。key_off_to_index_off 属于 range 坐标；key_result_col_indexes 属于 inner schema 坐标；output_mv_offsets 属于 MV 坐标。range 使用 `Vec<Vec<String>>`，不是 ranger 的真实可变 key range。

## 依赖与调用关系

[`mview_refresh_builder.rs`](mview_refresh_builder.rs) 的 `BuildMView` 直接调用索引筛选。[`../mview/mview.rs`](../mview/mview.rs) 再导出 lookup API。冻结 builder 实现入口自行构造 identity key mappings，未调用 Extract/Derive 生成其 FAST 计划；Derive 有独立测试直接调用。不要从公开导出推断所有校验已在生产路径执行。

## 错误处理与边界

字符串错误涵盖输出长度不符、key 数量不符、重复 MV group key、重复/越界结果列、非 key 个数不匹配、空 inner schema、空首个 range、key offset 越界及非 IndexJoin。只按第一个 range 的宽度检查 key offsets，不校验后续 range 等宽；不限制重复非 key offsets。索引筛选没有表达 prefix length/状态/索引类型等完整 metadata，空 group keys 的集合规则会匹配可见索引，调用方需维护前置约束。

## 并发与资源生命周期

同步纯内存操作，无 I/O、锁或线程。Extract 移动 inner_source 和向量取得所有权；筛选/重排分配集合及向量。没有真实 snapshot 或查询执行资源。

## 与 Go 版本的对应关系

Go 可遍历优化物理计划并丢弃合成 outer probe，Rust 仅要求调用者显式传入 IndexJoin 描述，不遍历树、不剥离 child、不访问 InfoSchema，也不执行 lookup。

## 扩展指南

扩展真实优化器接线时保留 MV/index/inner 三种偏移的区分，核对 schema 重排并新增独立非法映射测试。若增加索引元数据，逐项对照 Go 可用性约束；不要让 identity mapping 掩盖 key 重排序。
