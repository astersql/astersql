# PostgreSQL18函数注册重载与双方言隔离计划

目标：让正式函数选择、结果身份、等价共享门槛与双方言专属策略由真实行为证据验收。

范围：承接大计划7的六个路线图主题；覆盖三层身份、候选过滤与工厂、重载/unknown/参数固定、signed ABS、COALESCE/NULLIF、CONCAT/极值、四个MySQL专属函数及对应客户端证据。

范围外：完整函数库、UDF/扩展加载、多态/domain生态、第二套类型或执行内核；不承接大计划8的date_trunc实现、19/20的全客户端矩阵或21整体回归。

假设：当前HEAD为5a7d126716bf234c27370fb642e500d6be939d9b，首批基线c84583fd558f6232fb400e19cfeb18aa0fe99a5c可解析。历史18.4差分只覆盖整数转换；当前未运行测试，18.6不能推定通过。

## 设计决策

- 复用正式注册表并按连接筛选签名；全局换表会污染连接，另建注册内核会重复能力，均不采用。
- 可复用底层算子；缺逐字段证据时保持方言签名分离，不按同名/同值批准别名。

## 架构说明

- builtin.rs内formal_registry拥有FunctionClassRegistry；builtin_registry.rs仅轻量快照，不能作为正式工厂落点。
- 会话负责分析及语义，server负责wire；特殊条件/极值表达式不硬套普通函数重载。

## 开发策略

- 对行为变更使用失败验证、通过验证和受限重构；先核实现状，已正确仅补覆盖，不制造红灯。
- 失败必须是目标运行期断言；编译失败、零测试、仅注册名字不能验收。
- 只处理单项行为必要局部接线；其他类型族或工厂缺口单列依赖，禁止递归扩大。
- plan.md生成后只读；活动ExecPlan、进度及证据在编号文件中维护。prompt.md仅为执行列表。

## 来源与继承边界

来源为 `.plans/2026-10-24-postgresql18-后续整体实施/7-函数注册重载与双方言隔离.md`、同目录《承接与覆盖.md》以及 `docs/postgresql-18-core-compatibility-roadmap.md` 第4章对应模块、目标、范围外、验收、后续细分正文。以章节标题和FUNC场景定位，避免依赖漂移行号；这些原始契约与已有plan.md只读。

原任务引用的2026-10-22蓝图任务4、2026-10-24首批plan.md/prompt.md当前路径不存在；不得重建或执行其删除任务。`docs/postgresql-protocol-first-phase.md` 的“2026-10-24 首批显式整数转换 libpq 差分”及保留源码是可用承接依据。当前各目标包doc.go不存在，执行时若新增则先读。RustCodeGraph已有索引，本轮status正常；执行者开始时重新检查新鲜度。

已定位：`pkg/expression/scalar_function.rs::newFunctionImpl`直接查funcs/extensionFuncs；`pkg/session/runtime/relational_value.rs::relational_expression_value`已有COALESCE/NULLIF/CONCAT/极值分支，其中CONCAT和极值见NULL即返回；`pkg/session/runtime/planning.rs`已消费方言缓存键；`pkg/server/pg_query_test.rs`、`pg_extended_test.rs`和`pg_client_integration_test.rs`在server lib中挂载。不能据此静态证据宣称运行通过。

## 接口依赖与边界

大计划1提供来源证据格式，大计划2提供连接方言/名称环境，大计划6提供I资格、unknown来源和参数/结果类型身份。使用现有接口满足当前单行为即可，不等待整个工作流完结。任务23的date_trunc正向成功单独依赖大计划8；缺工厂不能在本计划补建时间函数。任意schema、默认/variadic候选载体不足在任务7/8保留具体缺口，不承诺完整UDF。保留正常、边界、非法输入、NULL、恢复及清理目标，不把安全拒绝当成功实现。

## 证据与交付约束

每个生产修改须在对应编号文件记录它解决的来源场景或不可缺少局部接线。PG成功分别记录值/NULL、OID/typlen/typmod、参数描述、格式及零行描述；错误记录S/V/C/M及D/H/P的实际存在性和阶段。MySQL逐列记录自己的code/flags/charset/length/decimals及警告，内部类型枚举不能直接当wire code。未采集字段标缺证，不默认相等。共享候选的NULL、输入域、求值条件、返回类型、会话/collation、折叠/缓存、下推资格逐项核实；不新增下推授权。

真实参考记录version()/server_version_num、客户端版本、构建SHA、会话设置和环境；18.4与18.6分别报告，精确18.6缺失不降级目标。缺参考环境保留相关任务待回归，不能宣称全部完成。进程、连接、结果、临时表/库由自己的finally/Drop清理，不操作用户已有服务或数据，DSN/密钥不得进入日志。

本轮只生成Markdown计划，不运行Cargo/Go/数据库/客户端，不创建执行会话。后续交付执行Ready适用检查，代码变更必须make lint；verify-profile技能当前缺失，按根AGENTS.md与testing-flow执行，不声称技能通过。Bazel准备依完整触发清单；不默认bazel_lint_changed。实现须保留PingCAP许可，修复可用Rust文件加/留AsterSQL标记；测试与生产分文件。外部依赖移植遵守独立上游/tag，不能vendor或本地patch。
