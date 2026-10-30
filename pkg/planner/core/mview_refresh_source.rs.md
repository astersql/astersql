# `pkg/planner/core/mview_refresh_source.rs` 逻辑说明

## 文件定位

物化视图刷新派生 SELECT AST 的生成模块，由 canonical session 的刷新 runtime 调用。

## 核心职责

为 FAST 构造 signed MLog 聚合与当前 MV LEFT JOIN；为 COMPLETE DELTA APPLY 构造 FULL JOIN 旧新行图像；为 MIN/MAX 重算构造仅包含绑定组键的 lookup。

## 主要符号

`BuildFastRefreshSource`、`BuildCompleteRefreshSource`、`BuildBoundMinMaxLookup`、`StripQualifier`。

## 执行流程

解析后的定义 AST 保留原 WHERE 和聚合参数。FAST 的 TSO 窗口与 WHERE 合并，输出遵守既有 merge sink 的列偏移；COMPLETE 比较缺失侧与 payload 差异；lookup 保留原 WHERE、GROUP BY 和支持索引 hint。

## 数据与状态

输入 MViewDefinition、定义 SELECT、实际表名和 TSO；输出 SELECT 与已有 MViewDeltaMerge / MViewCompleteDeltaApply 布局。AST 由调用者拥有，不读取存储。

## 依赖与调用关系

复用 mview_refresh_builder 的布局验证和聚合分类，由 session/runtime/mview_source 编译为真实物理计划。

## 错误处理与边界

返回缺失 COUNT(*)、日志列、非法聚合或布局的错误。nullable key/payload 使用 null-safe 比较，额外 rowid 单独投影；COMPLETE 不依赖 MLog 存在。

## 并发与资源生命周期

没有线程、连接或事务；NodeRef 的局部 AST 共享遵守现有线程内 planner 边界。

## 与 Go 版本的对应关系

经用户授权补移植 34d5c2b75b4f422f43f82c96b97bdf1667305d71 的派生源生成能力，为 a87bf8db6499ac38075781cef7357709f07ea002 runtime 提供不可缺少的数据源。

## 扩展指南

保持 sink 输入布局与原 WHERE；新增表达式必须用真实 typed executor 验证，AST 契约通过不能代替结果验证。真实 runtime 的15项聚焦回归已覆盖四模式、WHERE/nullable/MINMAX、有界水位与隐藏句柄；继续保持这些数据结果契约。
