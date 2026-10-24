# PostgreSQL18方言语法与名称解析计划


目标：补齐PG语法与名称绑定的保真交接，逐行为取得真实解析、诊断、恢复及双方言隔离证据。

范围：承接大计划2拆分批次2，覆盖ILIKE/ESCAPE/优先级、三类RETURNING、冲突目标/动作/引用域、ARRAY/ROW、分段引用名/限定名/CTE/路径、原SQL诊断、身份生命周期和新增语法参数。每项交付代码或独立真实测试；解析证据与执行支持分开报告。

范围外：完整PG parser、第二套执行内核、递归执行、DML返回/冲突执行、通用类型codec、schema/ACL建设及其他大计划的整体实施。

假设：静态基线HEAD为0fdee3a372d61bc4dd98a2649b43f49bdb084d83；首批身份/int4/int8保留。当前未运行代码，历史18.4的94场景不等于当前通过，18.6仍缺实测。

## 设计决策


按单语法比较共享AST与受控PG入口，以真实原型选路；优先复用现有载体。禁止通用文本替换、全局方言开关和第二套执行内核。

## 架构说明


PG边界→共享Parser/AST→名称绑定→正常准备/规划链。协议负责原SQL诊断传输；未开放消费者在副作用前显式拒绝。

## 开发策略


对行为变更先失败验证、再最小生产修复、再通过验证；旧实现已正确则补真实覆盖，不伪造红灯。编号文件是按PLANS.md维护的活动ExecPlan；本指南生成后只读，不含任务进度。批次与线性顺序只在编号文件和prompt维护。

## 来源与最新证据


先读 `.plans/2026-10-24-postgresql18-后续整体实施/plan.md`、`承接与覆盖.md`、`2-方言语法与名称解析补全.md`，以及原蓝图任务2、首批plan/prompt和 `docs/postgresql-protocol-first-phase.md` 的首批差分节；全部来源plan只读。路线图第2章十二主题是行为契约，章末验收/后续细分也必须阅读，不能只按本指南摘要实现。

首批保留完整SHA：连接身份 `5e4ec4d724adf25d62f3eff32c1af45a4bb4f5d8`，预处理身份 `92d3f185237d5cac2b910493d87cf01209322074`，规划缓存 `b681f059ff2cc2dfcae88dc06b5c738a8fbf8cb6`，Cast AST `40f232b42095e92ec40a5e86c9738d5ac215bbe8`，int4 `797627cfdc1514f4ce802df92a7798c8115a8153`，int8 `b58e189a42d21adf29cbde03b24ae7cf3e4caee6`，参数推断 `3cc3247715046fb713cb85f380b44b1f7dddd267`，优先级修复 `899335615b01ae366796a7ae0550b72922900008`，18.4差分 `c84583fd558f6232fb400e19cfeb18aa0fe99a5c`。复用首批已有测试，不恢复已删除首批编号任务。

较新证据接口：`d6e314382d980bb9dfa05a4527b45a11d1ae42cf` 注册输入，`634ffa0d8c5611bb207a1300ba21e4a296201cfd` 分端归档，`b6dbca95fbdcaf5ed275a25bed23431b2bbdd5be` NULL，`e2855a57483cd24e6efd59e04b12bf9261155bb0` 多重集，`a10e3c0ac4e65688e681c00e0378da374ff2f10d` 元数据，`ef1784aaeace675ac8068675b101d7f5b17a34cf` 拒绝恢复，`004e62212d3cb23d4b4c17d387b24ddcd936503d` 来源，`e37c137c4dbc6f9d77066798f794503743d65701` 去重计数，`ae72080ecefd84c4dd0937b1992c8d560c792ef4` 白名单，当前HEAD能力门槛。仅源码存在，不声称本轮运行通过。

`pg_core_evidence.py` 的 `append_observation` / `compare_metadata` / `validate_provenance` / `summarize_run` 可复用；`CaseRegistry` 与拒绝比较器仍限定首批phase/OID/错误集合，不能直接套到语法失败。新语法仅按本任务需要扩展，不能放宽旧门槛。`pg_client_integration_test.rs::pg_core_libpq_integer_differential` 提供真实共享内核、双listener、MySQL checkpoint及清理模式；不把其94个case重新计为本计划新增场景。

## 现存落点与证据边界


已核 `astersql-parser` / `astersql-server` 的Cargo.toml，workspace resolver=3，默认features，无新增feature。server autotests=false，但lib.rs显式注册pg_name_test、pg_query_test、pg_client_integration_test等模块；新函数放这些独立文件，无需增加产品调试入口。目标包parser/ast/server/session/core/resolve没有doc.go；若后续触及core/base须读其doc.go；server子树AGENTS仅增加HTTP API说明要求。

Parser通过lib.rs包含checked-in `generated/main_tables.rs` / `hint_tables.rs` / `lexer_tokens.rs`；parsergen已有generate/check CLI。旧 `docs/rust/rust-parser-generation.md` 的build.rs描述已漂移，本轮不改原文，也不依它安排自动生成。修改语法须生成并审查RuleId和动作匹配，不编辑Go parser.y冒充Rust实现。

`pg_name::Resolver` 仅有database/public/edits和CTE名称集合；`adapt`读取DATABASE并重写后调用validate_relations。Extended普通Parse仍adapt→markers→command→prepare_statement(SQL)，没有已确认的prepare(AST)接口。共享AST已有Returning、Row、Cast；字段存在不表示完整PG源信息或消费者已支持。

## 局部依赖与方案门槛


大计划1有源码局部接口可供消费，工作区另有其计划拆分变更，应排除。后续执行先核HEAD与工作区，接口取证写在相关编号任务的发现/决策栏，不能拆成纯文档任务。ILIKE首任务以同一组输入做可运行保真原型，给出共享AST与受控入口的六维比较；优先已有Rust动作路径，若不能保真则保持未决并记录缺口，不强行选择。其余语法必须各自复核此门槛，不能从Cast或ILIKE自动推广。

大计划5/6承担数组/record和类型描述；10承担返回/冲突动作绑定及执行；11/13承担大小写对象、schema、有序有效路径和可见性。依赖只限当前行为实际需要的接口，未具备即在对应任务记录阻塞；不得为了过测补建整套子系统。Parse/Describe成功、拒绝门禁、完整执行各自报告，拒绝不能算功能成功。无法验证时保留任务文件。

## 交付约束


每个编号文件有完整Cargo共享槽位规则；本次只规划，不调用Cargo、不领取锁、不运行任何测试/数据库/客户端、不创建执行会话。实施期间WIP聚焦，交付Ready，代码改动要求make lint；verify-profile技能当前缺失，按根AGENTS/testing-flow执行并报告限制。不默认运行bazel_lint_changed；Bazel触发完整清单及Go failpoint规则按根执行，纯Rust测试函数不等于新增Go顶层Test触发。

Rust测试与源文件分离，保留PingCAP版权，修复可用后顶部加AsterSQL版权。外部依赖若真需变更必须独立上游提交/tag、统一Git tag，不允许vendor/本地patch；本计划不预设新增依赖。仅提交本任务实际变更，编号状态/删除单独不提交，来源plan与路线图永不自动改写。
