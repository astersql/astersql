# `pkg/session/runtime/import_query_source.rs`

## 文件定位

IMPORT 查询源的 ConcreteSession 专用适配，对应 Go query_source.go 与 session.go 的本提交增量。

## 核心职责

使用当前 MySQL SQL mode 解析 IMPORT，拒绝不可移植来源，复用 MDL 元数据与真实逻辑规划后捕获；区分查询状态编码与整会话迁移资格。

## 主要符号

ResolveSource、CaptureImportQuerySource、prepare_import_query_source、CheckCanMigrateStates、QuerySessionStates for ConcreteSession。

## 执行流程

解析 SELECT 后遍历真实 AST 解析源表和视图；视图在定义数据库中展开并忽略本地临时表遮蔽。mdl_stats_table 提供真实 MDL 表定义，transaction_mdl_schema 保留原事务 schema 版本。buildResultSetNode 必须成功才返回捕获；不会执行 SELECT。

## 数据与状态

来源表按 ID 排序去重，记录 DBID/ID/Revision；ReadTS 按既有读取优先级选择显式 snapshot、external timestamp、RR 活动事务或 storage CurrentVersion；READ COMMITTED 使用语句的新版本；Timestamp 使用当前系统秒数，状态 timestamp 使用现有变量 getter。

## 依赖与调用关系

由 import_query.rs 消费捕获和已展开 AST；SHOW 经 migration.rs 调用资格检查；恢复复用 migration 系统变量 setter 与运行时当前库/用户变量，存储读取仍走既有 query 路径。

## 错误处理与边界

临时表、虚拟系统表、序列函数/序列表、参数 marker、stale read 被拒绝；拒绝来源使用 Go 的 planner 1235 错误，参数 marker 使用 parser 1064；解析/规划/MDL 错误直接传播。循环视图受路径 ID 集合保护。

## 并发与资源生命周期

本地 Guard 无论成功失败恢复 mdl_autocommit_write；MDL 生命周期仍由原会话/事务拥有。对会话 RefCell 的借用不跨 SQL setter 或 KV snapshot 查询。

## 与 Go 版本的对应关系

将原 Go API 的作用映射到 ConcreteSession 等价入口，不要求复制 Go 对象模型；目前运行时真实可持有的事务、临时表、advisory lock 参与迁移检查。SQL LOCK TABLES 与 sandbox 在现有运行时未提供对应有效状态，不为本来源补建这些子系统。

## 扩展指南

新增来源种类需同时覆盖实际 planner 和 worker；禁止仅以 AST 字符串扫描当规划成功，禁止恢复 worker 自有事务变量。
