# 合并 3ba36621 的 Go 逐提交严格对齐 Rust 计划

目标：逐提交移植本次合并新增的 Go 行为到 Rust，保留既有 PostgreSQL 逻辑；本轮只生成计划，不修改代码或 Git 历史。

## 精确来源范围

合并 `3ba36621f06beafb25cd00073894118e91f13e2d`；第一父 `e3f3403b4002203a9cf12ae21fc6d82c9b63b888`；第二父 `f95ec32c8c038267a99abab991e9f754f8b49ba2`；共同祖先 `13544e9b1472f5c69c05e1ed607adcb3c151d53e`。来源是第二父可达、第一父不可达的 50 条提交，均为单父提交，不包括合并本身及此前已有提交。命令 `git rev-list --reverse --topo-order e3f3403b4002203a9cf12ae21fc6d82c9b63b888..f95ec32c8c038267a99abab991e9f754f8b49ba2` 确定历史拓扑顺序，不按可能错乱的作者时间排序。每条提交的非 .go 元数据、生成链、依赖和测试也核查。

## 范围外

完整历史缺口修复、通用基础设施、其他来源提交和当前未提交 PostgreSQL 功能工作不纳入。已解决的 .github 工作流冲突保持 .github-bak 布局，不重新启用。mvservice 框架本身是来源增量，允许实现，但不扩成后续调度系统。

## 已验证假设

Rust workspace、各 Cargo 所有者与独立测试入口存在；cmd/tidb-server/Cargo.toml 的 astersql-cmd-tidb-server 提供 nextgen，pkg/executor 也声明 nextgen；不向其他 crate 盲加 feature。RustCodeGraph status/sync 已运行，sync 报告 0 变化。已读受影响现有 doc.go（pkg/ddl/doc.go、pkg/config/diagnosticmode/doc.go）、DDL README、测试流程与 docs/docs-manifest.md。

mviewdeltamergeagg 的 Rust 实现在 pkg/executor/mviewdeltamergeagg/mod.rs，孤儿数据实现在 pkg/ingestor/globalsort/orphandata.rs，不按 Go 目录机械新建。pkg/parser/ast/lib.rs 包含 SqlDialect/AST 业务语义，不能当骨架排除；含业务逻辑的 mod.rs 同理。PostgreSQL 测试已经通过 pkg/server/lib.rs 挂载；真实客户端需要 libpq 与 Python。

## 设计决策与架构约束

选择一来源一任务、局部增量对齐，而非全量覆盖或合并多来源。共享源码、说明、manifest、格式化/lint 和 Git index 按来源顺序串行；Cargo 分槽不代表代码可并行，批次约束不冒充业务依赖。编号任务是唯一进度来源，plan.md 只读。

保留 PostgreSQL 的 SqlDialect::PostgreSQL、$n 参数、Parse/Bind/Execute/Sync、portal 生命周期、SQLSTATE、OID/二进制编码、search_path、事务失败状态和类型语义。Go 的 MySQL 专用规则只在既有方言边界内对齐，不能覆盖 PostgreSQL 的 NULL 顺序、强制转换、标识符或系统变量逻辑。

Go 特有 runtime/ABI 和 goroutine 栈格式以可观察契约核查；不同语言不是省略业务行为的理由。优先复用现有 Rust，生产改动必须有 Go hunk 或必要接线依据。外部依赖严格走独立上游提交/tag，不允许本地覆盖。

## Rust 说明文档同步原则

本提交实际修改的合格生产 .rs 同步同目录 .rs.md；测试及纯装配排除，lib.rs/mod.rs 按职责判断。A/M/D/R 与说明动作一致；已有清单条目不重复计数，新增/删除净加减，重命名保持勾选与净总数，纯修改不改计数。不重排无关条目。说明章节至少为：文件定位、核心职责、主要符号、执行流程、数据与状态、依赖与调用关系、错误处理与边界、并发与资源生命周期、与 Go 版本的对应关系、扩展指南、验证依据；区分真实接线、仅测试可见与 Go 独有能力。

## 总体验证策略

Rust 改动先 cargo fmt --all，真实聚焦回归与适用 Ready 随后；代码需要 cargo fmt --all -- --check、make lint、git diff --check，生产接线变化编译实际 NextGen 二进制。Bug/行为差异取红→绿；已有等价取得当前绿灯，不能用编译、零测试、全 ignored 或桩当证据。共享 SQL/协议路径跑 PostgreSQL 查询、扩展协议、事务、类型、会话和真实客户端保护。

按 docs/agents/testing-flow.md 决定 Go failpoint、集成记录和真实 TiKV 生命周期。执行期触发 Go/Bazel 元数据/工具链规则时先 make bazel_prepare；禁止默认 make bazel_lint_changed。上一轮本机 bazel_prepare 因缺少 bazel 失败，.agents/skills/tidb-verify-profile 也缺失；执行时复查，不能认为 Ready 已通过。Go 既有失败不阻塞已证明的 Rust 增量。

本轮只核验来源、映射、路径、提示与批次，不构建或运行行为测试。完成任务必须删除编号文件并独立 Git commit；状态只用未开始/进行中/已完成/已阻塞。覆盖证据摘要写入提交正文，避免删除任务后丢失证据。
