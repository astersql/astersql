# 正常 DDL 剩余任务拆分计划

目标：补齐 Go 当前正常 DDL 持久化分派与生产服务，完成来源任务 9 的工厂及生命周期验收。

范围：承接来源任务 9 已获授权的扩大范围；完善正常 owner、公共 schema/MDL、作业事务、正常处理器、服务安装和 TableMode 提交。按当前 Go runOneJobStep 的动作集合验收，不用内存模型、专用消费器或跳过其他作业替代。

范围外：来源任务 10、11 的旧链删除和真实跨空间集群交付；任务 43 其他功能；全体 SQL 前端重写；修改 Go 行为；主仓库提交。

假设：工作区含本任务及其他会话修改，不能整体恢复；来源任务 9 的历史限范围阻塞已被用户扩大授权取代。上游 client-rust 已发布 v0.4.2-aster.3，本计划不重新移植或发布依赖。

## 设计决策

- 复用公共 owner/scheduler/schemaver/issyncer，避免搬迁专用链。逐个移植 Go 处理器；仅组装无法填补行为缺口，重写全链单任务又会过长。
- 来源任务 9 拆分承接；来源只读 plan.md 和任务 43 不改。

## 架构说明

- crossks 只提交；普通 Domain 持有正常 owner。作业及元数据同事务；版本和 MDL 屏障完成后进入 history。
- Rust 测试独立，网络适配归原模块。共用分派和 Cargo 资源串行执行。

## 开发策略

- 行为变更先失败验证，再实现、通过验证、重构。以真实 SQL/MVCC、完整 Go 作业及非空元数据为基础；仅替换选举、PD 等明确网络边界。
- 实施细节和活跃 ExecPlan 四节只在编号文件维护。本文件生成后只读；prompt.md 仅为复制入口。
- 每个编号独立批次，编号顺序即线性执行顺序，每批依赖前一批及传递依赖。无并行批次：共享 dispatcher、crate 注册、事务适配和 Cargo 构建资源均会冲突。
- 前一批完成须有最终证据，文件删除不等于验证通过。待回归不能视为已满足依赖；相关失败与零执行测试不可豁免。
- 完整支持按 Go 当前 switch 判断。保留未实现路径的显式错误，最后统一核验 handler_available；不得中途把尚未实现的动作宣称可用。
- 只处理编号任务边界。发现独立新设计或前置缺口，在该文件记录已阻塞，向用户说明需要另行拆分，不能吞并后续任务。

## 执行约束

从仓库根目录执行。先读 AGENTS.md、PLANS.md、相关 doc.go 和 docs/agents/ddl/README.md，优先 skills/rustcodegraph/SKILL.md。既有 Rust 实现需要与 Go 事务及副作用逐项核对，不因符号已存在就视为生产实现。

修改 Rust 后先 cargo fmt --all；WIP 用聚焦检查，Ready 用聚焦测试、cargo fmt --all --check、make lint、git diff --check，生产接线加 NextGen 服务编译。仓库缺少验证 profile skill 时按 AGENTS.md 执行检查集合并如实说明。禁止 make bazel_lint_changed；Go/Bazel 修改触发准备规则，纯 Rust 不自动触发。不要改变他人 failpoint 状态或终止他人 Cargo 进程。

保留 PingCAP Apache 版权；真正修复的 Rust 文件添加 AsterSQL 2026 版权。外部依赖只能来自独立上游已发布 tag，禁止本地 patch/vendor；如新增依赖缺口，先记录阻塞，不重新复制 client-rust。没有主仓库 commit 步骤。

## 完成边界

来源任务 9 仅在全部编号取得当前证据、正常生产安装及混合队列实际通过后才可关闭。收尾任务汇总证据并更新来源编号文件；不改任何只读 plan.md，不删除来源任务 43。此后执行原计划任务 10，再执行任务 11；本计划不提前宣称旧链删除或真实集群验收完成。
