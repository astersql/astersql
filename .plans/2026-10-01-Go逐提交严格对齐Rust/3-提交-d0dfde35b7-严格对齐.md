# 任务 3: 提交 d0dfde35b7 严格对齐 Rust

执行批次：【批次 3】；文件顺序前驱：2；任务编号：3。同批可并行，实际能力依赖另行核查。

状态：已完成

目的：逐项核对本Go提交在当前Rust的实际覆盖，只修缺失和偏离，保留完整Go逻辑与测试意图。

来源任务：Go提交 `d0dfde35b7279221642fcab69fa59cf567054383`；父提交 `ab7d93b603ba83d398d1b9b0063c78eceaacfc20`；原始提交标题：ddl: load cloud storage before picking add-index backfill (#69733)。

预计会话范围：限定本提交的变更行为及直接依赖，先复用已有正确移植；不扩大为所属子系统全量重写。


## 独立任务验收规则（2026-10-02 用户更新）


本节覆盖旧总计划、执行技能及本文件历史记录中与独立验收冲突的要求；总plan.md保留只读。本任务独立交付，只对来源提交包含的增量、必要直接接线及测试意图负责。按文件冲突图分批并行，编号前后关系和Git父提交不是业务依赖，不要求前面所有任务完成或全部回归；仅核查实际用到的具体能力，缺少时在本编号记录接口、源码和影响。

完成条件：全部增量在当前Rust真实路径中已实现且必要聚焦验证通过，或者有完整后续SHA及源码证明该增量已撤销/替代、无需恢复；仍保留的行为和测试意图必须逐项核验。符号存在、源码已读、仅修改状态、零测试、编译失败或旧报告不能代替行为证据。全部被撤销或无Rust影响的提交可用完整来源/去向/消费者证据完成，不强造行为测试。

验证只选本任务必要的受影响测试与仓库按变更类型要求的交付检查。Rust代码交付仍要求cargo fmt、聚焦回归、make lint及diff检查；生产接线改变再做NextGen编译。全仓测试、其他提交回归、全子系统补建、无关Bazel构建和一般性性能/集群验证不属于本任务完成条件，不因未运行它们标“待回归”。如果本提交行为本身依赖真实TiKV、集成或并发条件，相应最小验证仍是本任务必需，不能归入范围外来豁免。

仅使用状态：未开始、进行中、已完成、已阻塞。必要目标行为或必要验证缺失则已阻塞，准确记录缺口及已运行检查；与任务无关的基线失败先用合法目标命令或等价真实边界绕开，确实无法取得必要证据仍标已阻塞。禁止“已完成，待回归”及“核查完成”冒充实现完成。

交接明确列出：本提交范围内完成项、当前必要验证命令/数量/退出码、实际阻塞（若有），另列范围外建议且不影响已完成状态。记录保留，不删除编号，不提交主仓库，不吞并其他提交。历史状态由本文件最新结论覆盖，历史失败和未验证事实不删除。


## 文件冲突与并行执行（2026-10-02 用户更新）


本节更新并行调度，覆盖旧总plan.md及历史记录中的全串行要求；总plan.md仍只读。批次和边仅由候选写入文件交集生成，是编辑顺序约束，不代表业务能力依赖。按编号保留共享文件的先后顺序，同批次可并行，最多10个任务；已完成节点直接满足约束。阻塞节点只延后真正依赖其能力或必须接续其共享文件改动的任务，不阻断不相关分支。

执行前必须在本编号确认实际写入清单、所属crate注册文件及必要helper/调用方。图未覆盖的新写入文件、真实业务依赖或共享可变资源出现时，先向调度者报告并调整文件占用，暂停冲突任务；不能擅自写入另一活动任务占用的文件。来源Git父提交、Cargo只读依赖或共同读取文档不是默认串行理由。没有完成全部函数级依赖分析，因此此图不是“无业务依赖”的证明。

根Cargo.toml/Cargo.lock、cargo fmt --all、make lint、Bazel准备、全局failpoint开关、共用playground以及上游发布由调度者串行安排独占窗口；其他编辑在cargo fmt --all窗口暂停，之后再基于最终文件做必要验证。聚焦测试可在安全资源边界并行，共用Cargo target会由Cargo锁串行等待，不能停别人的Cargo；若隔离target会导致重复巨量编译，优先排队。每任务仍要取得独立目标证据，串行共享交付检查只可复用确实覆盖最终未再改变文件的结果。

本任务的候选写入清单和带路径原因的前驱边见本目录依赖与并行批次.md、文件依赖图.html及并行调度.json。来源候选不是保证写入项；确认某共享注册/manifest无需修改后可在调度层解除相应编辑约束，真实能力依赖必须另以源码证据记录。


## 文件


- Go来源：`pkg/ddl/backfilling_test.go`。
- Go来源：`pkg/ddl/index.go`。

- 已存在Rust候选（先核对，不代表须修改）：`pkg/ddl/backfilling_test.rs`。
- 已存在Rust候选（先核对，不代表须修改）：`pkg/ddl/index.rs`。

其他来源文件也必须核查，不默认全部无需Rust同步：

无。

回归测试使用真正拥有crate的独立测试文件，前缀 `go_commit_d0dfde35b7`；已有同名*_test.rs优先扩充，autotests=false时必须核验模块注册。本文件只列实际存在候选，不凭路径相似创建重复实现。

## 上下文


从仓库根执行，先读AGENTS.md、PLANS.md、相关包doc.go；DDL先读docs/agents/ddl/README.md。使用skills/rustcodegraph/SKILL.md定位真实实现；未索引、配置或过期文件再rg。当前已有部分移植，不预设缺失，不继承旧任务完成状态。

## Go变更定位清单


以下是Git差异上下文入口，并非完整行为清单；必须补读完整函数、helper、调用方和前后代码：

- `import (`。
- `func TestPickBackfillType(t *testing.T) {`。
- `func initForReorgIndexes(w *worker, job *model.Job, idxInfos []*model.IndexInfo)`。

新增Go测试入口：本提交差异没有新增顶层Go测试；仍需阅读相邻已有测试及变更子测试。

## 测试计划


行为：严格覆盖本提交每项生产变更、错误/边界及Go测试意图，记录每项Go源码位置到Rust实际调用路径的映射。

失败验证：在实际拥有模块的独立测试文件新增 `go_commit_d0dfde35b7` 前缀回归，输入、输出、错误及副作用来自本Go差异，使用现有真实组件。具体子测试名及数据在修改生产逻辑前写入本任务覆盖记录。

候选失败和通过命令相同，以下manifest均在制定计划时存在；先确认真正拥有模块，只运行受影响者。若实现已移动，在本文件记录新的真实manifest及原因后再运行，不同时遍历所有crate：

    cargo test --manifest-path pkg/ddl/Cargo.toml go_commit_d0dfde35b7 -- --nocapture

预期失败：缺失或偏离的Go行为在真实路径出现错误结果、遗漏副作用或状态/错误不符；每个测试具体预期值来自本提交Go逻辑。编译错误、缺资源和零测试不是有效行为红灯。已完整移植的项不人为回退制造失败，注明不适用并取得当前通过证据；新增测试应验证现有实现而非强迫重写。

模拟策略：复用真实SQL/KV/元数据及已有输入，Go要求有行或非空元数据时不能用空样例；只替换明确外部网络、时间与故障边界，记录数据形状、调用顺序、错误及副作用。

## 步骤


1. 运行下面的来源命令，读取本提交前后完整变更函数和直接helper、相邻Go测试；检查后续提交及当前Go是否撤销或替代该逻辑，记录完整SHA及逐项去向。
2. 验证Rust生产入口、工厂接线和相邻独立测试；逐函数建立覆盖清单，区分已等价、未完整、偏离、被后续替代及有证据的无Rust影响，不用同名符号或旧通过报告认定完成。
3. 对已等价项运行实际测试并记录数量与退出码；对缺失项先写真实行为回归、取得准确失败，随后按Go条件、顺序、状态、错误、并发和副作用修复当前所属模块，不新建简化替代链。
4. Rust修改后先cargo fmt --all，运行对应前缀回归，确保实际执行、无相关ignored；核查Go原有测试分支均有对应覆盖，不能仅测成功路径。
5. 只做本提交必要的前置修复；遇独立架构或外部依赖缺口记录本编号阻塞、准确接口和错误，不扩大为全量DDL/前端重写。Go有意跳过的逻辑不能在Rust加额外行为。
6. 完成自审与适用Ready验证，保存Go→Rust覆盖表、当前命令及红绿证据；大提交逐段续接，不以局部通过宣称完整完成。

## 验证


来源检查：

    git show --format=fuller --stat d0dfde35b7279221642fcab69fa59cf567054383
    git diff --find-renames --unified=30 ab7d93b603ba83d398d1b9b0063c78eceaacfc20 d0dfde35b7279221642fcab69fa59cf567054383
    git log --format='%H %s' d0dfde35b7279221642fcab69fa59cf567054383..ad193e964b^2 -- pkg/ddl/backfilling_test.go pkg/ddl/index.go

后续日志只查本提交相关路径；源码更名则追踪重命名，不把日志无命中当作没有后续影响。生成Go追溯生成源和Rust机制，不手抄生成结果；删除路径核对调用者与替代实现。

代码交付Ready命令：

    cargo fmt --all
    cargo fmt --all -- --check
    make lint
    git diff --check

再运行本任务拥有crate的上述聚焦回归。生产接线增加：

    cargo check --manifest-path cmd/tidb-server/Cargo.toml --bin astersql-cmd-tidb-server --features nextgen

Go/Bazel修改前按AGENTS.md判断make bazel_prepare；failpoint/集成记录/RealTiKV按docs/agents/testing-flow.md。禁止make bazel_lint_changed，不停止他人Cargo、不改变他人failpoint。文档、测试资料、构建或无Rust影响任务选择实际适用检查并说明理由，不为非代码提交制造生产修改。

所需证据：全部源路径和Go函数/分支有明确去向，测试意图匹配，Rust实际调用链及字段/错误/副作用等价；缺失项有行为红→绿及数量、退出码，已有正确项有当前绿；每个Ready结果与未验证项明确。0测试不算通过。

## 完成


只在全部本提交行为及测试意图取得证据后标记已完成，并保留编号文件。已阻塞在本文件记录准确原因及已运行命令，不修改plan.md/prompt.md，不删除本提交的覆盖依据。范围外全仓/集群/性能建议不影响本任务完成；本任务必要行为或验证缺失必须标已阻塞，不使用待回归状态。最终按AGENTS.md报告修改文件、profile与理由、正确性/兼容性/性能风险、确切命令及未验证项；不提交主仓库。

## Go到Rust覆盖记录


执行时按函数或行为分段填写：Go位置/条件与顺序/错误与副作用/Go测试意图 → Rust真实符号及入口 → 已有或修复/后续替代SHA → 具体测试输入输出与命令、退出码和数量。每段完整保留，不用一句“已对齐”替代逐项证据。

## Progress（进度）


- [x] 读取Go完整来源与后续覆盖关系，列出本提交全部行为分段。
- [x] 核对现有Rust与实际接线，记录已覆盖和缺口。
- [x] 逐段补齐必要逻辑并取得适用红绿/当前通过证据。
- [x] 自审、适用Ready及交接完成。

## Surprises & Discoveries（发现）


尚未执行；Git路径与现存候选是计划事实，Rust行为完整性尚未验证。

## Decision Log（决策）


2026-10-01：一个来源提交对应一个任务；保留已有正确移植，按Go完整逻辑核对，不偏移目标。

## Outcomes & Retrospective（结果）


尚未执行；结束时记录逐项产物、最终证据与未验证项。分段完成只代表该段，不代表整个提交。


## 2026-10-02 当前执行证据与阻塞交接


读取 skills/do-task-plans/SKILL.md、skills/rustcodegraph/SKILL.md、PLANS.md、pkg/ddl/doc.go、docs/agents/ddl/README.md、docs/agents/testing-flow.md；按照用户指令保留本编号，覆盖技能的删除任务文件要求。plan.md/prompt.md只读。HEAD为2beb25b9a4f3554998e872b4babc3b0854acc50a；工作区存在批次1/2已授权修改，本次没有回退或提交这些修改。

前置依赖证据：任务1最终记录14个目标测试、4个定向测试及fmt/lint/diff/NextGen退出0；任务2最新2026-10-02记录23项唯一目标回归及Ready退出0，历史阻塞已被后续授权和最终证据覆盖。批次1无前置依赖。上述记录仅用于顺序解锁，不代替本任务当前证据。

### Go→Rust覆盖与去向


| Go行为/测试意图 | 当前Rust实际事实 | 判定 |
| --- | --- | --- |
| initForReorgIndexes：空idxInfos直接返回；非空先loadCloudStorageURI，再pickBackfillType；错误立即返回；Txn/TxnMerge拒绝部分索引；NeedMergeProcess递增telemetry并将全部索引BackfillState置Running | index.rs没有对应初始化函数；index.rs:857的pick_backfill_type只接受distributed/ingest/temporary_index_merge三个布尔值；graph callers仅backfilling_test.rs:150，未接生产 | 缺失，不能用布尔模型验证顺序、元数据和副作用 |
| loadCloudStorageURI：读取worker/store对应URI，缓存到jobContext，UseCloudStorage=URI非空且IsDistReorg，之后afterLoadCloudStorageURI注入点 | meta/model/reorg.rs保留真实DDLReorgMeta字段；vardef/tidb_vars.rs有CloudStorageURI；DXF handle有GetCloudStorageURI，但未见DDL模式初始化将这些组件接到真实job；reorg_util.rs的InitializedReorgMeta只是独立快照模型且仅测试调用 | 组件存在不代表生产等价；缺少job生命周期中的接线 |
| pickBackfillType：已有ReorgTp保持；非fast用Txn；LitInitialized且cloud用Ingest并跳过磁盘；本地预检错误返回None/error且不写入类型；预检成功用Ingest；环境未初始化用TxnMerge | index.rs:846枚举只有Transactional/LocalIngest/Distributed，缺少None/TxnMerge，函数不接受job、不返回Result、不调用DiskRoot；ingest/disk_root.rs:135存在真实pre_check_usage，但DDL crate该依赖仅cfg(windows)，选择器没有调用 | 与Go完整类型、持久化、错误契约偏离 |
| TestPickBackfillType原三个分支：已选Txn保持、环境未初始化TxnMerge、环境初始化Ingest；新子测试ID2、fast/dist=true、URI=s3://bucket、磁盘预检强制失败，调用真实初始化后无错误、UseCloudStorage=true、ReorgTp=Ingest；Cleanup恢复全局配置 | backfilling_test.rs:150只测三个布尔输入和temporary merge；lib.rs:144真实注册，但没有go_commit_d0dfde35b7目标测试 | 既有测试不是本Go意图；不能计为覆盖 |

当前Go initForReorgIndexes位于index.go:1385–1408，helper位于1854–1888，调用方index.go:1229、modify_column.go:1269/1517。来源父→提交完整差异仅移动加载调用并新增测试及testfailpoint导入。已读本提交完整helper及当前完整函数；后续路径日志列出14提交，d0dfde35b7→ad193e964b^2的差异没有改变初始化顺序或删除新增cloud子测试。5e1901b920a58fce91f8493cfe24299ebe77f168新增resolveCloudStorageURI处理owner failover，未撤销本提交；保留其后续语义，本任务不回移或吞并该独立提交。后续RU、vector、auto split、collation及disk worker变更没有替代目标初始化函数。

### 精确生产阻塞边界


persistent_actions.rs:20–26的handler_available不接受ACTION_ADD_INDEX=7或ACTION_ADD_PRIMARY_KEY=32；table_mode.rs:190–199在真实持久化分派直接返回normal DDL persistent handler unavailable for action。job_worker.rs:443–472明确只有transaction backend stage，run_transactional_index_backfill拒绝IsDistReorg或非ReorgTypeTxn，模式选择属于尚缺的action。canonical_domain.rs:1039的add_index是catalog元数据追加，不能替代owner/job/schema状态和模式初始化。PersistentReorgContext仅存snapshot/info/runtime，未提供本提交需要的worker jobContext URI与模式初始化链。

因此不能通过局部移动一个调用完成任务：最小前置能力是实际ADD INDEX（及共享初始化调用方）的job初始化入口、真实job ReorgMeta与owner上下文URI加载、ingest环境/磁盘预检的生产边界，以及选定模式的生产分派。补完整ADD INDEX action/state machine及ingest/DXF调用链属于独立基础能力范围，不在本次授权内。没有新增布尔简化模型、仅内部flag测试或未接线helper来冒充修复；没有改动任何Rust/Go源或依赖。

按任务步骤5及执行技能“遇到阻碍立即停止执行”标已阻塞；这属于相关Rust能力缺口，不能使用无关基线例外标已完成，待回归。续接需要提供上述能力的拥有任务与当前完成证据，或明确授权必要生产接线范围；仍在本编号维护，不能推进批次4。

### 当前命令与结果


仓库根来源命令均退出0：

    git show --format=fuller --stat d0dfde35b7279221642fcab69fa59cf567054383
    git diff --find-renames --unified=30 ab7d93b603ba83d398d1b9b0063c78eceaacfc20 d0dfde35b7279221642fcab69fa59cf567054383
    git log --format='%H %s' d0dfde35b7279221642fcab69fa59cf567054383..ad193e964b^2 -- pkg/ddl/backfilling_test.go pkg/ddl/index.go
    git show d0dfde35b7:pkg/ddl/index.go
    git diff d0dfde35b7 ad193e964b^2 -- pkg/ddl/backfilling_test.go
    git diff d0dfde35b7 ad193e964b^2 -- pkg/ddl/index.go
    git show 5e1901b920a58fce91f8493cfe24299ebe77f168 -- pkg/ddl/index.go
    git rev-parse HEAD
    git status --short
    ~/.rustcodegraph/bin/rustcodegraph status
    ~/.rustcodegraph/bin/rustcodegraph callers pick_backfill_type --limit 20

RustCodeGraph当前11338文件/298676节点；使用query和node核验上表的具体位置，rg核对未命中符号及元数据字段，跨语言图边不作为Rust接线证据。

    cargo test --manifest-path pkg/ddl/Cargo.toml go_commit_d0dfde35b7 -- --nocapture

退出0；构建1m14s，实际running 0 tests，0 passed/0 failed/0 ignored/610 filtered out。仅证明目标测试入口缺失，不算行为红灯或绿灯；没有当前有效回归验收证据。构建过程未修改Cargo清单/依赖，也未使用本地patch。

本次为只读代码核查与测试入口诊断，仅修改本编号记录；无代码交付profile，未完成Ready。verify-profile技能在.agents不存在，搜索skills也没有入口。未运行cargo fmt、fmt check、make lint、NextGen（没有代码修改，不应以编译代替相关缺失行为）；未运行Go/Bazel、failpoint开关、集成录制、RealTiKV或性能测试。回归新增/有效红绿、完整Go边界、真实接线均未验证。正确性风险：本目标尚未对齐；兼容性风险：真实job类型/错误/持久化语义未证明；性能：无新增生产修改，但未验证cloud/local回填性能。最后运行git diff --check检查记录和保留工作区，结果在下方追加。

本轮结果：已阻塞，保留编号文件、总计划只读、不提交主仓库；不声称359任务或本提交完成。

## 2026-10-02 授权续接


用户明确授权补齐本提交验证必需的ADD INDEX持久化DDL分派与回填worker模式接线，复用已有ingest/DXF路径；范围限于使initForReorgIndexes选择顺序在真实Rust生产入口可执行，不做完整DDL子系统重写。前节阻塞停止点转为历史记录，继续在本编号实现及取得真实红绿和Ready证据。前节git diff --check退出0。尚未取得行为验收证据，不标完成。

### 续接里程碑与测试设计


共享初始化首先按Go父提交顺序建立可调用代码，独立backfilling_test.rs新增cloud_storage_precedes_disk_selection，注入磁盘故障边界（URI s3://bucket/dxf/、job2、fast/dist true、一个真实IndexInfo结构）。首次错误类型路径编译失败不计红灯；修正group_3类型后运行前缀退出101，1 failed/0 passed，返回mock ingest environment check failed。这是新移植初始化函数的前序顺序红灯，不冒充已有Rust生产路径红灯。随后移动加载到选择之前，真实worker回归仍需补齐，不以该helper测试单独验收。

决定限定为共享初始化阶段：复用JobWorker::transit_persisted_job_step与SystemSessionPool拥有的真实KV/SQL事务，新增initialize_persisted_index_reorg阶段入口和persistent_actions::initialize_reorg_indexes阶段分派（ADD INDEX、ADD PRIMARY KEY、MODIFY COLUMN）；初始化不执行完整action、不发布Public，不静默将ingest/DXF转为Txn。与已存在run_transactional_index_backfill相同，这是一段可由action驱动的生产worker操作，不宣称完整ADD INDEX action已实现。需要当前非零真实worker测试证明job行持久化、owner lease、URI读取和磁盘错误。

接下来真实测试在session crate独立文件，前缀go_commit_d0dfde35b7：真实表有行、真实IndexInfo来自表列，真实持久化job行；cloud路径配置URI并注入Go命名磁盘故障后成功Ingest/UseCloudStorage/BackfillRunning，本地路径相同故障需失败且持久化job不改变；空索引不加载URI；started type保持；非fastTxn及非initializedTxnMerge/部分索引拒绝。共享helper边界测试扩展用于不改变全局配置的全部分支；真实阶段测试验证生产worker适配而非复制结果。


## 2026-10-02 最终覆盖与验证（覆盖历史阻塞结论）

用户再次明确授权将正常 ADD INDEX 分派接入初始化阶段，后续未实现阶段明确报错，不扩展整个 DDL/backfill 子系统。已将 persistent_actions::handler_available/step 接入 action 7/32；NormalDdlExecutor 原有事务/错误/历史机制直接调用 initialize_prepared_index_action，再进入共享初始化。该生产阶段从实际 KV 表元数据及 Go-wire ModifyIndexArgs 取得已准备的 StateNone 索引，不依赖测试提供的环境模型。缺少已准备索引、rollback、后续 schema/backend 阶段均明确报错。初始化成功只持久化模式及 BackfillState，不冒充 schema 已推进或索引已发布。

**边界必须保留：**新建索引元数据的 preparation 阶段、完整 SQL ADD INDEX 状态机、ingest/DXF 后端启动及实际回填仍未实现。本任务完成的是用户限定的 d0dfde35b7 初始化顺序/模式/错误/副作用以及正常持久化分派可达性，绝不表示任意 SQL ADD INDEX 已可用。正常入口回归使用真实有行表、真实预备索引元数据、真实持久化 job、NormalDdlExecutor 和 KV/SQL 提交；它不执行 SQL ADD INDEX 从新索引构建到 Public 的完整周期，也不以直接 helper 测试作为充分证据。

### 逐段最终映射

| Go 条件、顺序、错误与副作用 | Rust 实现及真实接线 | 当前验证 |
| --- | --- | --- |
| 空索引直接返回，不访问环境 | index::init_for_reorg_indexes 首个判断；persistent_actions::initialize_reorg_indexes 同样跳过 context | empty_indexes_do_not_access_environment；无 ReorgMeta 也成功，环境不可访问 |
| loadCloudStorageURI 先于选择，缓存 owner jobContext，UseCloudStorage=URI 非空且 dist | ConcreteJobExecutionContext 实现 ReorgIndexEnvironment；读取真实 CloudStorageURI，复用 DXF resolve_cloud_storage_uri/store clusterID/SEM；owner epoch 改变清空 URI cache；正常分派 → initialize_prepared_index_action → initialize_reorg_indexes → init_for_reorg_indexes | cloud_storage_precedes_disk_selection 在 afterLoad 验证 cloud=true/ReorgTp=None；persisted_worker_cloud_skips_real_disk_fault 验证实际 owner URI s3://bucket/dxf/及 hook 1次；normal_dispatch 测正常 executor |
| 已选 ReorgTp 保持；非 fast Txn；初始化且 cloud Ingest 绕过磁盘；local 预检失败 None/error，成功 Ingest；未初始化 TxnMerge | index::pick_job_backfill_type 使用 group_3::Job/DDLReorgMeta/ReorgType；生产 ingest::env initialized_disk_root；DiskRoot::pre_check_usage 实际 mkdir/fs2 filesystem probe，保留 ErrIngestCheckEnvFailed 和 Go 命名 failpoint、macOS 风险豁免 | preserves_started_type_and_go_fallbacks 7分支；cloud/native/local 真故障边界；disk_root 真实路径错误红绿 |
| Txn/TxnMerge 拒绝 partial；错误先后不可变化；merge telemetry 一次，全部 BackfillRunning | index::init_for_reorg_indexes 原样保留 ErrUnsupportedAddPartialIndex、NeedMergeProcess 分支，实际 telemetry counter 和 canonical IndexInfo | partial_index_errors_follow_selection_and_loading；fallback test 测两个索引及真实计数器增量，不仅 flag |
| TestPickBackfillType 新 cloud 子测试 fast/dist、URI、ingest 初始化、磁盘强制失败但 Ingest 成功 | 原 helper 测 ID2；真实阶段及正常 executor 测 Go 命名 disk failpoint + 配置恢复；源/测试独立文件 | 4个 ddl目标、4个 session目标、1个原生 disk目标，均非零/无 ignored |
| onCreateIndex 调用初始化失败取消，后续不虚报成功 | 正常 action 初始化失败置 Cancelled，由既有 NormalDdlExecutor 清理 statement、保留原表、写 KV/SQL history；后续阶段错误显式持久化 | normal_dispatch：cloud 成功 Running/None/Ingest/BackfillRunning；下一次 error_count=1、明确未实现、仍非 Public；local 标准8256错误、Cancelled历史、queue 删除、table BackfillState 未改变、原数据保留 |
| owner 换届禁止旧初始化提交 | initialize_persisted_index_reorg 捕获 epoch 并复用 transit_persisted_job_step fencing | initialization_fences_replacement_owner：afterLoad期间epoch1→2，失败not DDL owner，queue bytes不变；不宣称已修改全局 normal executor 的所有epoch契约 |

### 修改文件（仅本任务）

- pkg/ddl/index.rs、backfilling_test.rs、job_worker.rs、persistent_actions.rs、Cargo.toml。
- pkg/ddl/ingest/disk_root.rs、disk_root_test.rs、env.rs、Cargo.toml。
- pkg/dxf/framework/handle/handle.rs（原 GetCloudStorageURI 共用 URI resolver，原矩阵保留）。
- pkg/session/runtime/system_session.rs、runtime.rs、runtime/normal_ddl_index_reorg_initialization_test.rs、pkg/session/Cargo.toml。
- Cargo.lock 只增本地 crate 正常依赖记录；保留批次2既有 prometheus 变更。
- 本编号任务文件。没有修改总 plan.md/prompt.md，没有提交、删除本编号或推进批次4；其他既有任务1/2修改保留。

没有改动 Go、Bazel、新Go import/top-level Test 或 Go模块依赖，不触发 bazel_prepare。未运行禁止默认运行的 bazel_lint_changed。外部 client-rust 没有改动；没有临时依赖副本、vendor、third_party、本地 patch 或外部 tag 修改。

### 红灯（编译错误不计行为红灯）

1. ddl 新初始化使用 Go 父提交先 pick 后 load：目标 cloud test 实际1 failed，退出101，mock ingest environment check failed；之后移到 load 在前。这是新移植旧顺序红灯，不冒充基线不存在的函数已有实现。
2. native precheck 临时仅替换为 HEAD 旧缓存实现：disk 前缀实际1 failed，退出101，期望真实路径错误却返回 Ok。Python finally 恢复文件；脚本 exit0 不当作测试绿灯。
3. 在真实 session 阶段临时只恢复父提交先 pick 后 load 顺序，运行 `cargo test --manifest-path pkg/session/Cargo.toml --lib go_commit_d0dfde35b7_persisted_worker_cloud_skips_real_disk_fault -- --nocapture`：实际1 failed/0passed，退出101，标准 `[ddl:8256]Check ingest environment failed: mock error`；finally 恢复最终顺序。
4. 新 normal_dispatch 用例加入但正常分派尚未接入：`cargo test --manifest-path pkg/session/Cargo.toml --lib go_commit_d0dfde35b7_normal_dispatch -- --nocapture` 实际1 failed/0passed，退出101，normal DDL persistent handler unavailable for action7。加入正常 action 阶段后当前同用例绿灯。其先前 mutable 引用/CIStr fixture 编译失败不计红灯。

### 最终当前命令与数量

WIP 为前缀定向回归与编译；交付使用 Ready（验证技能路径不存在，直接使用任务明确要求的 scoped tests/fmt/lint/NextGen/diff）。未以 task1/2 检查代替本任务。以下均退出0；没有 zero-test 或 ignored 验收。

    cargo test --manifest-path pkg/ddl/Cargo.toml go_commit_d0dfde35b7 -- --nocapture

4 passed，0 failed/ignored，610 filtered，最后运行7.31s构建/0.72s测试后再次针对最终分派运行4passed，4.75s构建/0.74s测试。

    cargo test --manifest-path pkg/session/Cargo.toml --lib go_commit_d0dfde35b7 -- --nocapture

最终4 passed，0 failed/ignored，561 filtered，构建20.13s/测试27.78s；包含真实 normal dispatcher、cloud/local故障、owner变更。正常分派单个用例内部覆盖cloud/local两次实际事务及成功后的下一次执行。

    cargo test --manifest-path pkg/ddl/ingest/Cargo.toml go_commit_d0dfde35b7 -- --nocapture
    cargo test --manifest-path pkg/ddl/ingest/Cargo.toml disk_root_test -- --nocapture
    cargo test --manifest-path pkg/ddl/ingest/Cargo.toml env_test -- --nocapture
    cargo test --manifest-path pkg/dxf/framework/handle/Cargo.toml test_handles_preserve_go_cloud_storage_prefix_matrix -- --nocapture

依次1/4/3/1 passed，全部0 failed/ignored。disk_root4包含目标1，只按唯一测试计数一次。env3与DXF1是受影响兼容性回归。

    cargo test --manifest-path pkg/session/Cargo.toml --lib normal_ddl_plan_backfill_owner_preserves_ingest_dxf_selection -- --nocapture

1 passed/0 failed/ignored；最终正常分派修改后复跑，1 passed/0 failed/ignored、564 filtered、测试15.37s、退出0。保证已有 transactional worker 不会把 ingest/DXF 静默转为Txn。

    cargo fmt --all
    cargo fmt --all -- --check
    make lint
    cargo check --manifest-path cmd/tidb-server/Cargo.toml --bin astersql-cmd-tidb-server --features nextgen
    git diff --check
    git diff -- .plans/2026-10-01-Go逐提交严格对齐Rust/plan.md .plans/2026-10-01-Go逐提交严格对齐Rust/prompt.md

最终Rust修改之后fmt check、make lint、NextGen(12.75s)均退出0。总计划diff空。最后记录文件修改后的diff check下方补记。

唯一目标回归9项（ddl4/session4/native1）；关联兼容回归8项（disk其余3/env3/DXF1/transaction-worker1），合计17项，重复执行不重复计数。

### 风险、验证限制与后续推荐

正确性：目标初始化顺序、真实模式字段、磁盘错误、partial拒绝、计数器与真实分派行为有证据；完整SQL ADD INDEX、新索引准备、schema transition、rollback及ingest/DXF执行仍明确未实现，不据此宣称全DDL可用。下一步应由后续拥有任务补足 prepared index 创建及后续 stage，在成功初始化的同一模式上继续回填，不能丢弃当前顺序/错误契约。

兼容性：保留真实Go-wire Job/IndexInfo、V1/V2 decoder、现有DXF URI矩阵和错误身份；normal新增可分派action7/32目前只有初始化阶段，后续错误仍通过既有 normal executor 计数/重试/取消机制记录。normal入口回归此次仅 action7/V2/预备索引；action32/V1完整生产周期未验证。afterLoad回调后同owner epoch更换保护在独立阶段入口测试，未修改整个通用 normal executor 的 owner 逻辑。

性能：云路径绕过实际磁盘probe；local路径新增真实filesystem查询与owner cache mutex，未做benchmark。macOS session test链接器提示__eh_frame超过16MB可能影响异常处理性能，没有编译/测试失败。

未验证：完整SQL ADD INDEX 到Public、实际cloud/local backfill后端、RealTiKV、集成SQL录制、Go回归、性能与非macOS真实disk-full。默认 session cargo test（未加--lib）在既有 pkg/session/tests/system_session.rs:234 的 crate::runtime 不存在处编译失败，改用真实目标所属--lib运行并取得非零证据；没有修复无关基线或以其豁免目标Rust失败。

最终确认：记录修改后 git diff --check 退出0；plan.md/prompt.md diff为空。Rust源码已恢复最终 load-before-pick 顺序，所有临时红灯修改已恢复。完成本提交初始化范围，不推进批次4，不提交主仓库。
