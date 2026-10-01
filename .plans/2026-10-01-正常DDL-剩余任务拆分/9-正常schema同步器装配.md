# 任务 9: 正常schema同步器装配

批次：【批次 9】依赖：批次 8 及传递依赖。线性顺序为编号递增；本批仅此任务，不并行写文件或运行 Cargo 验证。

状态：未开始

目的：普通 Domain 使用公共普通 issyncer，刷新全库元数据和 validator。

来源任务：`.plans/2026-09-30-crossks-对齐-Go/9-跨空间工厂与生命周期对齐.md`；用户已授权扩展正常 DDL 分派及上游提交检查。原任务 10、11、43 保持各自范围。

预计会话范围：只处理本文件定义的单一行为或指定状态阶段。共享基础能力由前置批次提供；未完成后续步骤不能吞并到本会话，也不能用简化实现通过。

## 文件

- 修改/核对：`pkg/session/runtime/normal_ddl_service.rs`。
- 修改/核对：`pkg/session/runtime/session_factory.rs`。
- 修改/核对：`pkg/domain/domain.rs`。
- 修改/核对：`pkg/infoschema/issyncer/syncer.rs`。
- 修改/核对：`pkg/session/runtime/normal_ddl_test.rs`。
- Go来源：`pkg/domain/domain.go`。
- Go来源：`pkg/infoschema/issyncer/syncer.go`。

## 上下文

普通 issyncer::New 已存在；NewCrossKSSyncer 只加载系统库。Domain.start 另有旧 periodic reload，需要防止重复循环，保留 CrossKS 原路径。

普通 owner 复用公共 JobScheduler/NormalDdlExecutor/SystemSessionPool；crossks 只提交。版本与元数据及job写入遵守Go事务，处理器全部分支需要实际行为证据。阅读现有Rust实现后决定复用，不能假设它已有持久化能力。

## 测试计划

- 行为：普通同步器读到最新 KV 用户库元数据，更新共享 InfoCache/validator，并在关闭时终止 SyncLoop/MDLCheckLoop/min-job 循环。
- 新增/扩充独立测试前缀：`normal_ddl_plan_schema_runtime`。这是计划定义的名称，执行前需创建或注册测试，不代表已存在；不可零执行验收。
- 失败验证命令：`cargo test --manifest-path pkg/session/Cargo.toml --lib normal_ddl_plan_schema_runtime`。
- 预期失败原因：正常 service 目前仅 DomainSchemaLoader reload，未安装公共普通 Syncer/validator/循环组合。
- 通过验证命令：`cargo test --manifest-path pkg/session/Cargo.toml --lib normal_ddl_plan_schema_runtime`。
- 模拟策略：真实 SystemSessionPool、SQL系统表、KV/MVCC事务、完整Go-wire参数及非空元数据/已有行。只替换明确网络/时间故障边界，保留请求、错误和外部副作用；不以mock标志、空行表或内存job模型证明业务。

## 步骤

1. 用直接 KV 变更用户库元数据的失败回归，证明真正 schema 可见而非 reload 计数。
2. 组装公共 New、共享缓存、真实系统池、NormalSchemaCoordinator 和 version Syncer。
3. 按 Go 生命周期启动/停止 SyncLoop、MDLCheckLoop、MinJobIDRefresher；协调 Domain 原 reload 循环。
4. 验证每个构造失败、取消和幂等关闭，不能用 CrossKS 系统过滤器。

## 验证

- 先格式化：`cargo fmt --all`。
- WIP聚焦：`cargo test --manifest-path pkg/session/Cargo.toml --lib normal_ddl_plan_schema_runtime`，实际新增测试执行且断言目标数据/元数据/状态正确。
- 公共事务/分派受影响时的最小周边回归：`cargo test --manifest-path pkg/session/Cargo.toml --lib crossks_align_normal_ddl_persists_table_then_synced_history_after_barrier`。仅在相关证据失效时运行；完整正常DDL集合仅在最终统一验收重跑，不在每个action会话重复广泛扫描。
- Ready：`cargo fmt --all --check`、`make lint`、`git diff --check`。按AGENTS.md选择检查集合；不虚构已读取缺失的profile skill。
- 涉及生产接线：`cargo check --manifest-path cmd/tidb-server/Cargo.toml --bin astersql-cmd-tidb-server --features nextgen`。

- 所需证据：具体文件/符号与Go来源、失败→通过命令和退出码、实际测试数量且0ignored、真实输入/输出及副作用、diff自审、Ready结果和未验证项。

## 完成

仅本任务行为和前置阶段具备当前证据才能报告已完成并按prompt删除本编号文件。中间阶段完成只表示此阶段，不表示整个动作或来源9完成。相关接口/Go等价/真实行为缺口必须已阻塞；只有无关基线阻碍可已完成，待回归，不能解除后续依赖。

最终回复按AGENTS.md报告文件、验证profile及原因、正确性/兼容性/性能风险、确切命令和未验证项。只在本编号维护ExecPlan；不修改只读plan.md/prompt.md或来源任务43，不提交主仓库代码，不停止其他会话进程。新范围缺口先记录阻塞并交由用户另拆。

## Progress（进度）

- [ ] 阅读依赖最终证据并记录工作区基线。
- [ ] 取得适用的失败行为证据；纯验收记录不适用原因。
- [ ] 完成本任务步骤并取得通过证据。
- [ ] 差异自审、Ready检查及交接完成。

## Surprises & Discoveries（发现）

执行时在本节记录实际发现及文件/符号/输出；目前仅计划，尚未实现本编号行为。

## Decision Log（决策）

2026-10-01：按用户要求拆分长任务；本编号只承接所列行为，完整Go状态和副作用不得删减。

## Outcomes & Retrospective（结果）

未开始；实施时记录本编号产物、完成证据及未验证项，不凭历史局部通过宣称来源9完成。
