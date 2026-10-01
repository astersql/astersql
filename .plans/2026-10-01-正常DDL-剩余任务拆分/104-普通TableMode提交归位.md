# 任务 104: 普通TableMode提交归位

批次：【批次 104】依赖：批次 103 及传递依赖。线性顺序为编号递增；本批仅此任务，不并行写文件或运行 Cargo 验证。

状态：未开始

目的：将普通TableMode生产调用接到公共jobsubmit与正常owner，消除字符串提交占位。

来源任务：`.plans/2026-09-30-crossks-对齐-Go/9-跨空间工厂与生命周期对齐.md`；用户已授权扩展正常 DDL 分派及上游提交检查。原任务 10、11、43 保持各自范围。

预计会话范围：只处理本文件定义的单一行为或指定状态阶段。共享基础能力由前置批次提供；未完成后续步骤不能吞并到本会话，也不能用简化实现通过。

## 文件

- 修改/核对：`pkg/session/runtime/normal_ddl_service.rs`。
- 修改/核对：`pkg/session/runtime/system_session.rs`。
- 修改/核对：`pkg/session/runtime/dispatch.rs`。
- 修改/核对：`pkg/session/runtime/dxf_session.rs`。
- 修改/核对：`pkg/domain/domain.rs`。
- 修改/核对：`pkg/ddl/jobsubmit/table_mode.rs`。
- 修改/核对：`pkg/session/runtime/normal_ddl_test.rs`。
- Go来源：`pkg/ddl/table_mode.go`。
- Go来源：`pkg/ddl/job_submitter.go`。

## 上下文

DdlService::alter_table_mode目前是字符串接口且只测试占位callback。复用SubmitOnlyBackend及build_alter_table_mode_job，普通提交与crossks提交都不得直接写表模式。

普通 owner 复用公共 JobScheduler/NormalDdlExecutor/SystemSessionPool；crossks 只提交。版本与元数据及job写入遵守Go事务，处理器全部分支需要实际行为证据。阅读现有Rust实现后决定复用，不能假设它已有持久化能力。

## 测试计划

- 行为：普通目标服务接到真实typed TableMode请求后，owner关闭时仅排队，owner开启后正常步骤完成；CDC/SQL mode与错误/取消保持Go。
- 新增/扩充独立测试前缀：`normal_ddl_plan_normal_submit`。这是计划定义的名称，执行前需创建或注册测试，不代表已存在；不可零执行验收。
- 失败验证命令：`cargo test --manifest-path pkg/session/Cargo.toml --lib normal_ddl_plan_normal_submit`。
- 预期失败原因：现有NormalDdlService生产submit没有真实实现，调用返回占位错误或旁路直接更新TableMode。
- 通过验证命令：`cargo test --manifest-path pkg/session/Cargo.toml --lib normal_ddl_plan_normal_submit`。
- 模拟策略：真实 SystemSessionPool、SQL系统表、KV/MVCC事务、完整Go-wire参数及非空元数据/已有行。只替换明确网络/时间故障边界，保留请求、错误和外部副作用；不以mock标志、空行表或内存job模型证明业务。

## 步骤

1. 读Go正常AlterTableMode提交及所有Rust调用方，基于已存在类型替换无实际语义的字符串占位接口。
2. 为普通提交关闭owner→持久化Queueing→恢复owner→Synced新增失败回归，覆盖正常/no-op/非法mode。
3. 用公共jobsubmit提交、等待history和schema同步；审查dxf_session相关旁路，仅修本链TableMode调用。
4. 复用实际SQL/KV服务与取消，验证生产factory路径和源/目标隔离，不扩大为全SQL前端重写。

## 验证

- 先格式化：`cargo fmt --all`。
- WIP聚焦：`cargo test --manifest-path pkg/session/Cargo.toml --lib normal_ddl_plan_normal_submit`，实际新增测试执行且断言目标数据/元数据/状态正确。
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
