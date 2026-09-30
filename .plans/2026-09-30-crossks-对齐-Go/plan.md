# crossks 对齐 Go 计划

目标：删除 crossks 专用 owner 和 schema 实现，恢复 Go 的跨 keyspace 提交、同步和资源生命周期，保持真实生产能力。

范围：追查 0168e799a489de0f495f605f8ce801ed042d391f 及后续依赖；补齐对应系统会话、schemaver、issyncer、jobsubmit 和正常 DDL 消费路径；替换 Session 接线并删除六组专用实现与测试，保留必要适配及 Go serverinfo 清理。

范围外：任务 43 的 TTL、Extract、inference；全量 DDL 功能重写；修改 Go 行为；外部依赖移植；提交代码。

假设：当前 Session 已引用六组 crossks 模块。issyncer 存在占位参数，DDL 使用内存队列，已有组件不能视为生产能力已完成。工作区存在他人修改与 failpoint 生成物，执行前需重新记录基线。

## 设计决策

选择补齐 Go 对应组件后删除专用链。仅删文件会损失能力；搬迁专用 owner 会保留偏离。正常 DDL 缺口按编号任务验证，不能以专用 consumer 绕过。

## 架构说明

crossks 管理目标 Store、会话与同步器，只提交 DDL；目标正常 owner 消费。schema/MDL 归原模块，Session 只适配。系统会话保留事务亲和性。

## 开发策略

行为修改先写独立失败回归，记录预期失败，再实现并验证。所有批次串行，线性顺序为任务 1 至 11；不存在安全的并行批次，共享接线和验证资源不得并发写入。

plan.md 生成后只读。每个编号任务是独立的活跃 ExecPlan，按 PLANS.md 在其 Progress、Surprises & Discoveries、Decision Log、Outcomes & Retrospective 中维护实施证据；不得将进度写入本文件。prompt.md 只提供执行入口。

来源是本地任务 43 和提交，不是外部变更系统。来源任务仍有独立未完成内容，本计划不删除或改写它，不据 crossks 完成宣称任务 43 整体完成。

## 已验证的上下文

Go pkg/domain/crossks/cross_ks.go 的 createSessionManager 组装五个系统会话、schemaver、serverstate、issyncer、validator 和 jobsubmit，并启动 serverinfo、SyncLoop、MDLCheckLoop、MinJobID 刷新。它没有创建 crossks DDL owner。ad193e964b 第一父差异主要新增注册失败及关闭清理。

Rust pkg/session/runtime/crossks_owner.rs 自建选举、SQL 队列消费和 history；crossks_schema.rs 自建版本轮询。pkg/infoschema/issyncer/syncer.rs 的构造参数仍有 Option<()>，InitRequiredFields 是空实现。pkg/ddl/ddl.rs、job_scheduler.rs、job_worker.rs 存在内存模型，需要证实持久化生产链。删除前必须补齐这些依赖，不能仅换名称或搬目录。

## 验收边界

关闭正常目标 owner 后，crossks 提交只产生持久化作业，不修改表模式、不抢 owner；正常 owner 启动后消费并生成 Go 等价 history/schema diff。暂停、取消、失败、owner 丢失和重启遵循 Go 状态机。目标 schema 加载、校验、MDL 与生命周期遵循 Go 当前代码，不能要求 crossks 额外执行 Go 本来跳过的 MDL 分支。

替换后不存在六组专用文件、模块声明或生产引用。必要网络/非 Send 适配位于对应已有模块，不引入配置开关或旧逻辑兼容层。真实 PD/TiKV/etcd 测试使用独立测试 keyspace，验证源 keyspace 无变化，并在失败时清理资源。

## 验证与执行约束

代码迭代使用 WIP，交付使用 Ready：cargo fmt --all 后执行聚焦测试、cargo fmt --all -- --check、make lint、git diff --check 与 NextGen 服务 cargo check。仓库当前缺少 .agents/skills/tidb-verify-profile，执行者先搜索是否恢复，仍缺失时按 AGENTS.md 的检查集合执行并如实报告，不能虚构 skill 已使用。

Rust 测试与源文件分离；保留 PingCAP 注释，真正可用的 Rust 源文件添加 AsterSQL 版权。纯 Rust 不触发 Go/Bazel 条件；若必须更改 Go import、增加 Go Test 或 Bazel 元数据，先按 AGENTS.md 执行 bazel_prepare。禁止 bazel_lint_changed。Go 既有失败记录后可不阻塞，Rust 相关失败不能忽略。

已有 ignored 真实测试不能被过滤掉后声称通过。不得创建空 mysqlcompat 清单绕过编译；缺失测试资料需记录实际阻塞。当前工作区有 failpoint 改写，禁止替他人清理或恢复。只读计划不运行代码验证。

## 风险与恢复

正确性风险是作业持久化、owner fencing、schema/MDL 屏障和失败关闭；兼容性风险是 Go job/meta/etcd 协议；性能风险是新适配的线程数、池容量和轮询。删除在替代链验证后实施。不得整体 revert 原提交或 git reset；失败时仅撤销本任务拥有的修改。若正常 DDL 全量补建超出本计划可执行范围，记录具体缺口并阻塞后继任务，向用户说明需要独立计划，不悄悄扩大范围。
