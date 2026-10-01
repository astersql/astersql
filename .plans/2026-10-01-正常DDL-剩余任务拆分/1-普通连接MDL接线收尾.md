# 任务 1: 普通连接MDL接线收尾

批次：【批次 1】依赖：无。线性顺序为编号递增；本批仅此任务，不并行写文件或运行 Cargo 验证。

状态：未开始

目的：确认普通连接和内部会话共同阻挡 DDL，收尾中断前的弱引用接线。

来源任务：`.plans/2026-09-30-crossks-对齐-Go/9-跨空间工厂与生命周期对齐.md`；用户已授权扩展正常 DDL 分派及上游提交检查。原任务 10、11、43 保持各自范围。

预计会话范围：只处理本文件定义的单一行为或指定状态阶段。共享基础能力由前置批次提供；未完成后续步骤不能吞并到本会话，也不能用简化实现通过。

## 文件

- 修改/核对：`pkg/domain/domain.rs`。
- 修改/核对：`pkg/domain/Cargo.toml`。
- 修改/核对：`pkg/server/runtime.rs`。
- 修改/核对：`pkg/server/runtime_test.rs`。
- 修改/核对：`pkg/session/runtime/dispatch.rs`。
- 修改/核对：`pkg/session/runtime/normal_ddl_service.rs`。
- 修改/核对：`pkg/session/runtime/normal_ddl_test.rs`。


## 上下文

现有 NormalSchemaCoordinator 合并内部与 Server 检查，Domain 弱持有 InfoSchemaCoordinator；现有协调器回归只替换连接管理边界，尚不能证明真实 Server 接线。

普通 owner 复用公共 JobScheduler/NormalDdlExecutor/SystemSessionPool；crossks 只提交。版本与元数据及job写入遵守Go事务，处理器全部分支需要实际行为证据。阅读现有Rust实现后决定复用，不能假设它已有持久化能力。

## 接手基线

来源记录：`.plans/2026-09-30-crossks-对齐-Go/9-跨空间工厂与生命周期对齐.md`。历史失败及通过证据均在该文件；本计划不据历史结果标任何新任务完成。已有action为1/10/17/26/39/55/75/76，service/policy未安装到普通生产Domain。

上游 `/Users/Shared/work/dir/data/codes/client-rust` 已提交05736879d5d7ffde9d740e699024f8bc2d5f4dd7并发布v0.4.2-aster.3，8个manifest已统一tag；本次只读复核rev-parse与clean工作树，不重新提交。Go pause/resume reason原始JSON两项回归已绿；后续模型变更后需重跑受影响policy测试。

中断前启动的两条检查现已结束：`cargo check --manifest-path pkg/session/Cargo.toml --features nextgen`退出0（1m34s）；`cargo test --manifest-path pkg/session/Cargo.toml --lib crossks_align_normal_ddl_coordinator`退出0，1passed/0failed/0ignored（5.49s）。后者只替换连接管理边界，不能视为真实Server接线或整个任务9完成。新 Domain/driver 变更后尚无当前NextGen服务入口编译或Ready整套证据。

工作区已有其他会话计划文件删除及代码改动；先git status记录，不恢复、不提交、不清理别人的进程。原来源plan.md将全量DDL/外部移植列为范围外，是授权前约束；用户已明确批准扩大正常DDL及上游提交检查范围，本计划承接剩余部分。

## 测试计划

- 行为：真实普通连接持有旧表版本时，普通 Domain 检查拒绝相关作业，事务结束后允许，关闭后不保留 Server。
- 新增/扩充独立测试前缀：`normal_ddl_plan_user_mdl`。这是计划定义的名称，执行前需创建或注册测试，不代表已存在；不可零执行验收。
- 失败验证命令：`cargo test --manifest-path pkg/server/Cargo.toml --lib normal_ddl_plan_user_mdl`。
- 预期失败原因：现有测试只证明适配器；新增真实 Server/driver 会话测试复现未接线或提前允许的路径，先确认断言失败。若新增测试在当前代码已通过，记录已有实现及新增验证证据，不虚构红阶段；纯补覆盖无需人为撤销现有修复。
- 通过验证命令：`cargo test --manifest-path pkg/server/Cargo.toml --lib normal_ddl_plan_user_mdl`。
- 模拟策略：真实 SystemSessionPool、SQL系统表、KV/MVCC事务、完整Go-wire参数及非空元数据/已有行。只替换明确网络/时间故障边界，保留请求、错误和外部副作用；不以mock标志、空行表或内存job模型证明业务。

## 步骤

1. 先核对下方交接基线，保留既有工作区修改。
2. 以真实 Server/ConcreteSessionDriver 打开会话持有 TransactionMDL，补齐生产调用测试。
3. 核验 SetSessionManager 和 driver 设置路径，连接与内部池分别阻挡相关作业，删除登记不释放用户事务锁。
4. 验证 Domain 弱引用、关闭、重复关闭与连接清理回调；不安装普通 owner。

## 验证

- 先格式化：`cargo fmt --all`。
- WIP聚焦：`cargo test --manifest-path pkg/server/Cargo.toml --lib normal_ddl_plan_user_mdl`，实际新增测试执行且断言目标数据/元数据/状态正确。
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
