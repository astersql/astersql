# PostgreSQL 18 核心兼容路线图

本路线图用于定义 AsterSQL 的 PostgreSQL 核心兼容目标及后续验收契约，组织为下列八个工作流。路线图是文档交付，不表示相应兼容功能、差分平台或运行环境已经实现。

总范围涵盖兼容契约、方言与解析、类型与转换、函数与运算符、SQL 会话与事务、系统目录、线协议以及客户端和双方言共存验收。各项均为待定义和验证的目标，不构成当前支持声明；本轮交付八章骨架和第 1 章的兼容契约与差分验证要求。

## 1. 兼容契约与差分验证

### 模块

本章建立契约及取证要求，模块定位从 [历史 PG 边界说明](postgresql-protocol-first-phase.md)、[连接模块说明](../pkg/server/pg_conn.rs.md) 和 [真实客户端测试入口](../pkg/server/pg_client_integration_test.rs) 开始。PG 专属连接与协议边界入口为 `pkg/server/pg_conn.rs`；共享会话和执行接入的调研入口为 `pkg/server/conn.rs`、`pkg/server/driver.rs`，沿现有内核继续定位，不另建存储、优化器或执行器。模块说明仅供导航，后续取证须以能力 ID 关联实际源码符号、独立测试和提交 SHA；这些入口及其中的历史测试记录不作为本轮当前运行证据。

### 目标行为

#### 范围与非目标

核心兼容目标是为应用使用 PostgreSQL SQL 与客户端接口时可观察的行为建立明确契约，并在后续任务中定义差分验证要求。契约应区分共享语义与方言专属语义，覆盖查询结果、类型及元数据、NULL、错误、会话与事务状态和客户端交互；本小节划定范围，能力矩阵、证据规则和用例格式分别由下列同名小节定义。

PostgreSQL **18.6** 是来源计划指定的目标基线；本轮未核实该补丁版本的发布状态，也未运行真实 PostgreSQL 对照。握手中报告的版本、协议协商成功或客户端可连接，都不能据此宣称当前具备 PostgreSQL 18.6 的 SQL、事务或完整功能兼容性。

兼容工作共享现有规范执行内核，通过明确的方言边界承接 PostgreSQL 目标行为，并保持 MySQL 方言既有契约。这里的共享内核指复用现有执行能力，不另建存储、优化器或执行器；共享实现本身也不证明两种方言的可观察语义相同。

本路线图明确排除扩展加载生态：CREATE EXTENSION、PostGIS、FDW、自定义过程语言、C 扩展、自定义类型/运算符类及第三方扩展加载生态均为非目标，不列为核心兼容交付或验收承诺。

本轮文档任务不实现生产代码、差分执行器、数据库环境或 CI；不扩展兼容能力，不撰写第 2–8 章正文。后续目标必须有相应实现和当前验证证据后，才能成为当前支持结论；既有说明或历史测试不能替代该证据。

#### 能力矩阵

矩阵以可独立验收的方言契约为单位，先记录目标，再逐项补齐当前证据。能力 ID 稳定标识契约，不随测试名称或实现位置改变；示例 ID 仅用于说明模板，不代表已经登记的支持能力。

| 字段 | 填写约定 |
| --- | --- |
| 能力 ID | 唯一标识场景与方言契约；同一场景有双方言契约时，用 `/pg`、`/mysql` 分别标识并关联。 |
| SQL/客户端场景 | 写明方言、输入 SQL 或客户端操作、参数、编码及必要会话前提，避免仅用函数名表达范围。 |
| 所属工作流 | 使用本路线图第 2–8 章的编号与标题指定责任归属；跨工作流时列主责及协作方。 |
| 分类 | 仅取共享语义、PG 专属、MySQL 专属、明确不支持；表示目标归属，不表示当前已经支持。 |
| 当前证据 | 未采集当前运行证据时填“未验证”；历史文档与测试路径只能注明为调研入口，不能当作实测结论。 |
| 目标结果 | 分方言记录期望的值、行或拒绝行为及适用边界；成功与拒绝均可成为验收目标。 |
| 类型与元数据 | 分方言记录预期结果类型、列名、可空性及协议元数据；PG OID 与 MySQL 类型标志各自验收，不要求相等。 |
| SQLSTATE | 记录各方言预期错误码及触发条件；成功场景填“不适用（成功）”，不能用无错误码表示未验证。 |
| 依赖 | 写明完成契约所需的工作流、能力 ID 或环境前提；无前置依赖时明确填“无”。 |
| 验收入口 | 标明后续用例的定位入口、双方对照或拒绝检查方式；计划入口须注明“待新增”，不能冒充已存在的测试。 |

分类中的“共享语义”要求列明范围内的可观察行为一致，仍分别核对两种方言的类型及协议元数据。“PG 专属”和“MySQL 专属”各自保留方言契约，不因共用执行内核就合并；“明确不支持”要求记录拒绝范围及错误目标，不能把尚未实现或未验证的目标自动归入这一类。

“同名异义分流”不是第五种分类：同一名字在两种方言下语义不同时，分别建立 PG 专属与 MySQL 专属契约及验收条件，并关联两个能力 ID。下表示意将这两个契约并列在一行以便比较；后续登记与验收分别追踪，不将其标为共享语义，也不以共享实现推导结果一致。

字段不得以空白或裸 `null` 隐去信息。“待定义”表示目标尚未细化，“未验证”表示缺少当前运行证据，“不适用（原因）”表示该字段在此场景无含义；这些状态不能相互替代。若目标要求返回 SQL NULL，应明确写“SQL NULL”及对应的类型、可空性；它是结果值，不是矩阵缺项。

以下仅有四条说明性示例，分别演示共享函数、同名异义、PG 专属与明确拒绝。所有结果和 SQLSTATE 都是待验收目标；类型细节标为待定义的条目须在后续实施前补齐。四条当前证据均为未验证，本轮没有运行 AsterSQL、PostgreSQL 或 MySQL 对照，示意行不能充当支持证明。

| 能力 ID | SQL/客户端场景 | 所属工作流 | 分类 | 当前证据 | 目标结果 | 类型与元数据 | SQLSTATE | 依赖 | 验收入口 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 示例 ABS-POS | PG 与 MySQL：`SELECT abs(1) AS v`，限定正整数字面量 | 4. 方言函数与运算符契约；协作 3. 共享类型与转换契约 | 共享语义 | 未验证 | 两方言均返回一行值 1；不由此推广到溢出、NULL 或其他重载 | 列名 v、整数结果；精确类型、可空性、PG OID 与 MySQL 类型标志待定义并分别核对 | 不适用（成功） | 工作流 2、3 的字面量解析与类型契约 | 待新增：工作流 4 的双方言函数用例，分别对照参考端的值及元数据 |
| 示例 LENGTH/pg；示例 LENGTH/mysql | 各方言 `SELECT length('é') AS v`，输入编码均为 UTF-8 | 4. 方言函数与运算符契约；协作 3. 共享类型与转换契约 | PG 专属；MySQL 专属（两个契约） | 两个契约均未验证 | PG 契约按字符数，目标 1；MySQL 契约按字节数，目标 2；后续分别采集对照证据 | 两端列名 v、整数结果；精确类型、可空性及各自协议元数据待定义 | 两个契约均不适用（成功） | 工作流 2、3 的方言分流与字符串编码契约 | 待新增：工作流 4 的两条关联用例，分别对照对应方言参考端，禁止跨方言要求值相等 |
| 示例 SEARCH-PATH/pg | PG 同一会话依次 `SET search_path TO public`、`SHOW search_path` | 5. SQL会话与事务语义；协作 2. 方言身份与解析边界 | PG 专属 | 未验证；历史说明仅为调研入口 | 设置后 SHOW 反映 public 搜索路径；此示例不承诺完整 schema 解析或 MySQL 同名操作 | SET 无结果列；SHOW 的文本结果类型、列名、可空性及 PG 元数据待定义 | 不适用（成功） | 工作流 2 的 PG 会话与名称边界 | 待新增：工作流 5 的同会话设置与读取用例，对照 PG 参考端 |
| 示例 EXTENSION-REJECT/pg | PG：`CREATE EXTENSION postgis`，假定连接及 SQL 传输有效 | 2. 方言身份与解析边界 | 明确不支持 | 未验证 | 按扩展加载非目标拒绝；不得创建扩展对象，不扩展为扩展生态实现任务 | 不适用（拒绝命令，无成功结果集） | PG 目标 0A000（不支持功能）；当前映射未验证 | 工作流 2 的拒绝边界及工作流 7 的错误响应 | 待新增：工作流 2 的拒绝用例，核对错误码与无对象副作用；不要求和可加载扩展的 PG 参考端结果相等 |

#### 证据规则

兼容结论必须绑定能力 ID、具体场景及证据范围。证据按用途分级，不能仅按文档数量或测试名称升级结论；下表定义后续取证要求，本轮没有采集当前客户端或差分运行记录。

| 证据类别 | 可证明的范围 | 结论边界 |
| --- | --- | --- |
| 官方 PostgreSQL 18 行为来源 | 为目标行为提供规范依据，记录官方手册的具体版本、章节与链接；存在歧义时补充对应版本的参考端复现 | 定义预期，不能证明 AsterSQL 已实现；大版本手册不能单独证明某个补丁版本的实测结果。 |
| 源码/独立测试定位 | 以仓库提交 SHA、源码路径及符号、独立测试路径及用例名定位实现与断言意图；入口包括 [连接模块说明](../pkg/server/pg_conn.rs.md) 和 [客户端测试文件](../pkg/server/pg_client_integration_test.rs) | 阅读源码或找到测试不等于测试已执行；模块说明只作导航，运行结论仍需真实源码、独立测试及当前记录相互对应。 |
| 历史测试记录 | 追溯过去在已知版本、环境和命令下报告的行为；[历史 PG 边界说明](postgresql-protocol-first-phase.md) 是调研入口 | 没有本轮复现时统一标注“历史记录，未复测”；缺失字段照实标明，不能据此填入矩阵的当前通过证据。 |
| 当前真实客户端记录 | 证明本轮指定服务端、客户端及配置下，实际执行的连接或 SQL 场景得到所记录结果 | 仅覆盖执行及断言的内容；连接成功不证明 SQL，SQL 执行成功也不证明与 PostgreSQL 语义相同。 |
| 当前差分记录 | 对同一能力契约，在 AsterSQL 与指定 PostgreSQL 参考端上执行相同场景，逐项比较预期与实际结果 | 仅覆盖所运行的输入、类型、元数据、NULL、SQLSTATE、会话/事务状态及副作用等适用断言；需列出未比较项与规范化规则，不能推广为全部 PostgreSQL 兼容。 |

每条证据记录至少包含下列可追溯字段；不适用项写明原因，缺失项不得用空白掩盖：

| 字段 | 记录要求 |
| --- | --- |
| 关联与来源位置 | 能力 ID、场景/用例 ID、记录日期、证据类别、官方来源版本/章节/链接、源码及独立测试路径/符号、日志或产物位置；本地来源绑定仓库提交 SHA 及影响结果的未提交改动。 |
| 版本 | AsterSQL 构建与提交、PostgreSQL 参考端精确版本、客户端/驱动及依赖版本；未使用参考端须明示，不得由 server_version 推断实际构建或参考端版本。 |
| 环境 | 操作系统、部署方式、连接与协议模式、数据库初始状态、编码、时区、会话参数、必要配置及数据准备/清理步骤；记录不得暴露凭证。 |
| 命令与退出码 | 可复现的完整命令、筛选条件、执行时间及每个必要步骤的实际退出码；注明跳过、忽略、失败和环境缺项，命令退出 0 不代替断言结果。 |
| 有效用例数量 | 实际执行并完成适用断言的用例数，另列发现、跳过、忽略与失败数量；重试不得重复计入有效覆盖，零测试不得记为通过。 |
| 预期与实际结果 | 预期行为及依据、双方实际值与适用类型/元数据、错误和状态等断言、比较结果；保留原始输出位置，解释规范化或排除项及其影响，不能只写“通过”。 |
| 结论与限制 | 可连接、可解析、SQL 子集或语义兼容中的具体结论，覆盖条件及未验证项；缺证、版本不明或有效用例数量为零时记录“未验证”及原因。 |

结论用词有以下不同的验收条件，不构成自动递进关系：

- **可连接**：当前客户端完成所声明的握手、认证及连接就绪断言；只覆盖指定客户端、协议与配置。
- **可解析**：具体输入通过指定解析入口及 AST/拒绝边界断言；不能由握手或命令返回成功推定，也不说明执行结果正确。
- **SQL 子集**：列明实际执行并验收的语句、参数、数据和边界用例，说明检查了哪些结果；不能将少量成功语句概括为整个语法或功能兼容。
- **语义兼容**：仅对明确的能力契约和覆盖条件作结论，要求当前差分记录及官方行为依据，适用的值、类型、元数据、NULL、错误、状态与副作用断言完整通过；未覆盖重载、边界或其他客户端必须单列为未验证。明确不支持项按能力矩阵的拒绝目标验收，不因参考端可执行而要求结果相同，也不称为该功能已支持。

反例（仅示意，非本轮实测）：某记录只证明一个指定客户端完成握手，收到 server_version 和 ReadyForQuery，未发送 SQL。错误表述是“PostgreSQL 18 SQL 及事务语义兼容”；正确表述是“在该记录的版本、环境和配置下可连接；SQL 解析、执行、事务及语义兼容未验证”。若该记录来自历史说明，还必须加注“历史记录，未复测”，不能写作当前可连接结论。server_version 是服务端报告值，模块说明文件是调研资料，两者均不能单独证明功能通过。

证据缺失、版本不明、零测试或缺少适用断言的条目，相关当前结论统一标“未验证”，注明缺口；历史记录继续保留其历史身份。新证据只能支撑明确覆盖的契约，经核对来源、运行记录和断言后再更新对应当前证据，不得自动升级支持状态，也不得覆盖既有记录的限制。本轮只建立这些规则，不变更已有支持结论。

#### 差分用例清单

清单使用 JSON 对象描述单个用例，以 `case_id` 关联能力矩阵中的 `capability_id`。`case_id` 在整个清单内唯一，`capability_id` 引用唯一的能力契约，允许多个用例覆盖同一能力；两者均不随文件排列、执行顺序或重试变化。双方言不同契约分别建用例，通过 `related_case_ids` 关联。以下沿用矩阵的说明性能力 ID，不登记新的支持能力。

本轮选择文档内 JSON 示例：仅用 Markdown 表格无法直接作为结构化运行输入；独立 JSON Schema 会引入额外文件、维护和实现约束，因此留待运行器任务决定。这里仅定义格式契约，没有新增清单文件、校验器、执行器或 CI。

| 字段 | 含义与约束 |
| --- | --- |
| `format_version`、`case_id`、`capability_id`、`related_case_ids` | 格式版本与稳定关联；关联列表可为空，表示没有关联用例。格式演进不能重用旧 ID 表示另一契约。 |
| `dialects.pg`、`dialects.mysql` | 两个方言分别声明适用状态和原因。适用项各自记录 `sql`、`setup_sql`（准备）、`cleanup_sql`（清理）、`parameters`、`session`、`expected`。SQL 可不同，但须说明映射和覆盖边界；不能直接把 PG SQL 发送给 MySQL 并忽略错误。 |
| `parameters` | 按绑定位置记录参数值、逻辑类型、方言类型及文本/二进制格式；PG OID、MySQL 类型标志分别记录。参数值使用下述带标签的值表示。无参数用空数组；需要但尚未知的类型填“待定义”，不能执行时猜测。 |
| `session`、`setup_sql`、`cleanup_sql` | 分方言明确编码、时区、会话参数、连接/协议模式、初始数据和事务条件；准备/清理数组按顺序执行，空数组表示无需 SQL 操作。清理失败、准备失败及会话不满足须进入运行记录，不能算有效通过。 |
| `expected` | 目标依据、成功或预期拒绝、结果值及顺序、类型与元数据、SQLSTATE、会话/事务状态与副作用断言。`output`、`sqlstate`、`state_and_effects` 分别承载结果、错误及状态/副作用断言；`output` 沿用观测结果的行、列及顺序结构。目标未细化时标“待定义”；真实参考端的观测结果独立存放，禁止用目标值填充观测值。 |
| `comparison` | 每方言 AsterSQL 与对应真实参考端分别比较；规定行顺序、重复行、列顺序和规范化。无 ORDER BY 时若契约不要求行序可按多重集合比较，仍保留重复行及原始输出；列序和类型不得顺便忽略。每个排除项需说明理由和限制，未知规则标“待定义”。 |
| `observations` | 分别保存 `aster_pg`、`reference_pg`、`aster_mysql`、`reference_mysql` 的实际版本、客户端信息、输出、SQLSTATE、状态及副作用和证据引用。每个必需观测缺失时显式标“未采集”，不允许以空数组冒充零行成功。 |
| `evidence_refs`、`verdict` | 引用上一节证据记录的稳定 ID 或可定位产物（附提交 SHA），并能追溯日期、环境、完整命令及退出码、有效/跳过/失败数量、原始输出和限制。`verdict` 仅在全部适用断言及证据齐全后可记通过；否则记“未验证”或实际失败，缺证不能通过。 |

输入及采集后的每个结果单元用 `{"kind":"value","value":"1"}` 表示非 NULL 值，用 `{"kind":"sql_null"}` 表示 SQL NULL；具体类型由参数或结果列元数据提供。字符串 `"NULL"`、空字符串、零行和 SQL NULL 必须区分。JSON 的裸 `null` 不用作缺证或 SQL NULL。采集前整个字段用 `{"status":"未采集"}`，目标字段未细化用 `{"status":"待定义"}`，不适用用 `{"status":"不适用","reason":"具体原因"}`；这些状态均不等于空的真实结果。

`observations` 中每个采集完成的对象至少包含 `status`、`version`（实际服务端精确版本及 AsterSQL 构建/提交）、`client`（客户端/驱动及依赖版本、协议模式）、`output`、`sqlstate`、`state_and_effects`、`evidence_refs`。`output` 内含 `rows`（行数组，单元按上述标签编码）、`columns`（按列序记录列名、类型、可空性及 PG OID/MySQL 类型标志等元数据）和 `row_order`；命令无结果集需明确说明原因。成功的 `sqlstate` 记“不适用（成功）”，错误时保留实际码和原始错误输出。实际状态、副作用和客户端信息同样不得由预期推断。版本字段中另记录计划基线及参考端实际版本，二者不同需说明对结论的限制；不能把握手报告版本当作构建版本。

下面是唯一的格式示例，SQL 和参数只是待执行输入。PG/MySQL SQL 分别使用自身的参数占位符；示例限定正整数输入，尚不覆盖 NULL 输入或其他重载。两个方言均需采集 AsterSQL 与真实参考端，所有真实对照值、类型、SQLSTATE 和版本尚未采集，目标断言也须在后续实施前细化，不构成有效用例或支持证据。PostgreSQL 18.6 仅沿用来源计划的目标，本轮未核实其发布状态。

```json
{
  "format_version": "1",
  "case_id": "example.abs-pos.bound-int",
  "capability_id": "示例 ABS-POS",
  "related_case_ids": [],
  "mapping": "分别绑定正整数 1；仅覆盖此输入，不推广到其他重载",
  "dialects": {
    "pg": {
      "status": "适用",
      "sql": "SELECT abs($1) AS v",
      "setup_sql": [],
      "cleanup_sql": [],
      "parameters": [
        {
          "position": 1,
          "logical_type": "有符号 32 位整数",
          "dialect_type": "int4",
          "protocol_type": {"status": "待定义"},
          "format": "text",
          "value": {"kind": "value", "value": "1"}
        }
      ],
      "session": {
        "encoding": "UTF-8",
        "timezone": "UTC",
        "protocol": "扩展查询",
        "initial_state": "独立空闲会话，无需表数据",
        "settings": {"status": "待定义"}
      },
      "expected": {
        "outcome": "成功",
        "basis": "能力矩阵示例 ABS-POS；官方行为来源待补齐",
        "output": {"status": "待定义"},
        "sqlstate": {"status": "待定义"},
        "state_and_effects": {"status": "待定义"}
      }
    },
    "mysql": {
      "status": "适用",
      "sql": "SELECT abs(?) AS v",
      "setup_sql": [],
      "cleanup_sql": [],
      "parameters": [
        {
          "position": 1,
          "logical_type": "有符号 32 位整数",
          "dialect_type": {"status": "待定义"},
          "protocol_type": {"status": "待定义"},
          "format": "binary",
          "value": {"kind": "value", "value": "1"}
        }
      ],
      "session": {
        "encoding": "UTF-8",
        "timezone": "UTC",
        "protocol": "预处理语句",
        "initial_state": "独立空闲会话，无需表数据",
        "settings": {"status": "待定义"}
      },
      "expected": {
        "outcome": "成功",
        "basis": "能力矩阵示例 ABS-POS；参考端行为依据待补齐",
        "output": {"status": "待定义"},
        "sqlstate": {"status": "待定义"},
        "state_and_effects": {"status": "待定义"}
      }
    }
  },
  "comparison": {
    "pairs": [["aster_pg", "reference_pg"], ["aster_mysql", "reference_mysql"]],
    "row_order": "按返回顺序逐行比较",
    "column_order": "按返回顺序逐列比较",
    "normalization": [],
    "excluded_assertions": []
  },
  "observations": {
    "aster_pg": {"status": "未采集"},
    "reference_pg": {"status": "未采集"},
    "aster_mysql": {"status": "未采集"},
    "reference_mysql": {"status": "未采集"}
  },
  "planned_reference_versions": {"pg": "18.6（来源计划目标，未核实）", "mysql": "待定义"},
  "evidence_refs": {"status": "未采集"},
  "verdict": {"status": "未验证", "reason": "目标及参数协议类型待细化，尚无任何真实对照记录"}
}
```

PG 专属或 MySQL 专属场景中，另一方言对象用 `status: 不适用` 并记录原因；这是覆盖边界，不能计为该方言通过。明确不支持项使用 `expected.outcome: 预期拒绝`，记录拒绝目标 SQLSTATE、状态恢复及无副作用要求；目标码未知时标“待定义”。若参考端允许操作，保留其真实成功结果，并将比较方式明确为 AsterSQL 的拒绝契约验收，不能强求成功与拒绝相等，也不能忽略任一错误。普通成功用例遇到“不支持”错误是失败或阻塞证据，不能临时转成预期拒绝使其通过。类型、元数据或其他适用断言未采集时仍为未验证，只有完整符合拒绝契约的当前证据才能支持“拒绝边界已验收”，不代表功能已支持。

#### 依赖与验收交接

本轮八个工作流的文档交接顺序为：契约→方言→类型→函数→SQL/会话/事务→目录→协议→客户端验收。下表将此依赖图对应到章节和交接内容；它是路线图编写顺序，实际实现须按真实源码、行为依赖、测试及资源约束重新核实批次，不能据此假定类型或 SQL 实现必须等待协议实现。

| 工作流 | 前序输入与交接产物 |
| --- | --- |
| 1. 兼容契约与差分验证 | 建立范围、能力矩阵、证据规则和 JSON 用例契约，为后续工作流提供统一验收语言。 |
| 2. 方言身份与解析边界 | 读取工作流 1，明确方言身份、语法及名称解析责任，给类型与后续工作流提供可定位的输入边界。 |
| 3. 共享类型与转换契约 | 读取工作流 2 的解析边界，明确类型、转换、NULL 与元数据目标，交给函数及 SQL 语义契约。 |
| 4. 方言函数与运算符契约 | 读取工作流 3，区分共享语义与同名异义契约，为 SQL 表达式验收提供边界。 |
| 5. SQL会话与事务语义 | 读取工作流 4 及方言、类型契约，描述会话/事务状态与副作用目标，交给目录可见性和协议状态验收。 |
| 6. 系统目录与内省契约 | 读取工作流 5 及名称、类型契约，明确目录对象、可见性和内省输出目标，交给协议及客户端用例。 |
| 7. 线协议认证与数据通道 | 读取工作流 6 及类型、错误、会话契约，明确协议承载和错误恢复目标，交给真实客户端验收。 |
| 8. 客户端与双方言共存验收 | 汇合前七项契约，定义客户端场景、方言隔离和当前运行证据要求；只对覆盖的能力作结论。 |

模块定位使用本章“模块”的 PG 边界、共享内核和独立客户端测试入口；模块路径存在或历史测试被记录，均不等于该能力已有当前证据。验收按“能力矩阵”的能力 ID 与目标，关联“差分用例清单”的 `case_id`、`capability_id` 和“证据规则”的版本、环境、命令、退出码、有效用例数量、预期与实际结果等字段；每项适用断言均须能回溯来源，缺项保持未验证。

交接给源蓝图任务 2“方言身份与解析边界”的只读输入是本路线图第 1 章及第 2–8 章标题骨架、四类矩阵示例、证据分级与唯一 JSON 格式示例，以及本章列出的调研入口。任务 2 的定义和范围见 `.plans/2026-10-22-postgresql18-核心兼容蓝图/2-方言身份与解析边界.md`；此处只定义交接，不执行该任务。

文档交接通过条件：范围排除完整，八章顺序及第一章五个责任标题齐全；矩阵的四类示例明确目标与未验证状态；JSON 的能力 ID 可对应矩阵，用例 ID 唯一，SQL NULL、未采集、待定义与不适用含义一致；证据规则不将历史记录或可连接升级为语义兼容；引用路径存在且计划产物标明待新增；任务 2 可据此继续描述方言责任，无需假定兼容能力或运行器已实现。目标中的待定义字段由对应后续能力任务细化，不能视作已验收。

后续细分只按单一能力的可观察契约、单一用例族或独立差分运行器拆分。各任务须声明输入、依赖、适用断言、证据缺口与通过条件；运行器单独立项，本轮不创建执行平台、环境或 CI。文档交付验收只表示契约可交接；所有尚无当前运行记录的能力仍为未验证。

### 范围外

核心非目标统一以“范围与非目标”小节为准。本章不实现生产行为、运行器、参考数据库环境或 CI，不撰写第 2–8 章正文；交接顺序不扩展为各子系统实现计划。

### 验收

文档验收以“依赖与验收交接”的通过条件为准，逐项检查范围、矩阵、证据字段、JSON 关联、章节结构和路径。行为验收则须按矩阵目标及证据规则另行采集当前记录，不能用本章完成、JSON 语法通过或零测试替代。

### 后续细分

按“依赖与验收交接”先交给源蓝图任务 2，再沿后续工作流细化；实施任务限定单一能力、单一用例族或独立运行器。各项在实现前补齐目标及验证入口，按实际依赖重新排批次；本章不展开这些实现任务。

## 2. 方言身份与解析边界

### 模块

[章节验收与后续交接](#章节验收与后续交接)汇总本章来源映射；复核入口为 pg_name.rs、pg_session.rs、pg_extended.rs 及独立的 pg_name_test.rs、pg_query_test.rs，仅作静态证据。

[解析承载方案决策](#解析承载方案决策)比较共享 AST 扩展与独立受控 PG 解析入口；取证入口为 yy_parser.rs、parser.y、ast/expressions.rs、ast/dml.rs、ast/lib.rs、[optimize.rs](../pkg/planner/optimize.rs) 与 pg_extended.rs，独立测试入口为 yy_parser_test.rs、pg_extended_test.rs。只核对必要局部交接。

[方言身份传递](#方言身份传递)负责连接到共享内核的身份边界。实际源码入口为 [pg_conn.rs](../pkg/server/pg_conn.rs) 的 `PgService::start` / `negotiate`、[pg_extended.rs](../pkg/server/pg_extended.rs) 的 `Extended::new` / `handle` / `process`、[pg_session.rs](../pkg/server/pg_session.rs) 的 `PgSession` / `SessionQuery::parse`，以及 [conn.rs](../pkg/server/conn.rs) 的 `ClientConn::dispatch`、`SessionDriver::open_ctx` 和 `TiDBContext`。另核对 [driver.rs](../pkg/server/driver.rs) 的 `IDriver::OpenCtx`：它是另一层生命周期抽象，不能冒充 PG 当前调用的 `SessionDriver`。本块不指定尚未取证的内核类型或接口名。

[类型转换语法边界](#类型转换语法边界)的调研入口为 [yy_parser.rs](../pkg/parser/yy_parser.rs)、[expressions.rs](../pkg/parser/ast/expressions.rs) 和上述扩展查询入口；实际共享表达式转换载体另定位到 [ast/lib.rs](../pkg/parser/ast/lib.rs) 的 `ExprKind::Cast`。只核对本块必要的 AST 与参数接线，不展开类型执行子系统。

[大小写匹配语法边界](#大小写匹配语法边界)的源码入口为 [expressions.rs](../pkg/parser/ast/expressions.rs) 的 `PatternLikeOrIlikeExpr`、[parser.y](../pkg/parser/parser.y) 的 `PredicateExpr` / `IlikeOrNotOp` / `LikeOrIlikeEscapeOpt` 与 [pg_name.rs](../pkg/server/pg_name.rs) 的 `rewrite` / `adapt`。语法文件与 Rust 载体分别取证，不以 Go 语义动作推断 Rust 运行接线。

[返回子句语法边界](#返回子句语法边界)的指定入口为 [ast/dml.rs](../pkg/parser/ast/dml.rs)、[parser.y](../pkg/parser/parser.y) 与 [pg_result.rs](../pkg/server/pg_result.rs)；实际共享 DML/投影载体在 [ast/lib.rs](../pkg/parser/ast/lib.rs)，以 `Returning`、`SelectField`、`command` / `encode_formats` 为必要局部交接点。

[冲突子句语法边界](#冲突子句语法边界)沿用 [ast/dml.rs](../pkg/parser/ast/dml.rs)、[parser.y](../pkg/parser/parser.y) 与 [pg_extended.rs](../pkg/server/pg_extended.rs)；实际 INSERT 载体为 [ast/lib.rs](../pkg/parser/ast/lib.rs) 的 `InsertStmt` / `Assignment`，只补充本块必要的局部取证。

[数组构造语法边界](#数组构造语法边界)的入口为 [expressions.rs](../pkg/parser/ast/expressions.rs) 的 `Expr` / `RowExpr` / `ParamMarkerExpr`、[parser.y](../pkg/parser/parser.y) 的 `SimpleExpr` / `ArrayKwdOpt` 和 [pg_extended.rs](../pkg/server/pg_extended.rs) 的 `Extended::process` / `markers`，仅取证构造与类型/参数交接。

[行构造语法边界](#行构造语法边界)的入口为 [expressions.rs](../pkg/parser/ast/expressions.rs) 的 `RowExpr` / `Expr::restore`、[parser.y](../pkg/parser/parser.y) 的 `SimpleExpr`、[pg_name.rs](../pkg/server/pg_name.rs) 的 `Resolver::close` / `scope` / `relation`；独立测试入口为 parser_test.rs 与 pg_name_test.rs，只作静态取证。

[引号与大小写规则](#引号与大小写规则)以 [pg_name.rs](../pkg/server/pg_name.rs) 的名称扫描/改写与 [expressions.rs](../pkg/parser/ast/expressions.rs) 的名称载体为入口；独立测试为 pg_name_test.rs、pg_query_test.rs，仅作静态证据。

[限定名称解析规则](#限定名称解析规则)以 [pg_name.rs](../pkg/server/pg_name.rs) 的 `Resolver::relation` / `validate_relations` 与 [ast/dml.rs](../pkg/parser/ast/dml.rs) 为入口，实际共享 TableName 另核对 [ast/lib.rs](../pkg/parser/ast/lib.rs)；独立证据入口为 pg_name_test.rs、pg_query_test.rs。

[公共表表达式遮蔽规则](#公共表表达式遮蔽规则)以 [pg_name.rs](../pkg/server/pg_name.rs) 的 `Resolver::scope` / `relation` 为入口；[ast/dml.rs](../pkg/parser/ast/dml.rs) 为指定导航，实际 CTE 载体在 [ast/lib.rs](../pkg/parser/ast/lib.rs) 的 `WithClause` / `CommonTableExpression`。独立测试入口为 pg_name_test.rs、parser_test.rs，仅静态阅读。

[模式搜索路径解析契约](#模式搜索路径解析契约)以 [pg_session.rs](../pkg/server/pg_session.rs) 的路径访问器、[pg_name.rs](../pkg/server/pg_name.rs) 的关系门禁和 [pg_catalog.rs](../pkg/server/pg_catalog.rs) 的目录遮蔽接线为入口；独立测试入口为 pg_session_test.rs、pg_query_test.rs，仅静态阅读。

### 目标行为

[章节验收与后续交接](#章节验收与后续交接)以来源四步与完成条件的映射收束本章，给工作流 3 提供解析结构、名称绑定及类型判定边界。

#### 方言身份传递

本块承接第 1 章的共享内核、双方言独立契约与证据规则，对应源工作流 2 步骤 1；只定义身份传递，语法与名称规则留给本章后续文档块。连接建立时应由所属入口确定 PG 或 MySQL 身份，由该连接的会话持有，贯穿简单查询、扩展查询及后续执行上下文。身份在连接生命周期内稳定；会话设置、事务结束、预处理语句关闭和错误恢复不得把另一连接或另一入口切换方言。可变进程全局开关不能承担此职责。以下“应”均为目标契约，当前源码定位不等于全链路实现或运行通过。

责任链为 `listener → session → parser → resolver（名称绑定阶段） → planner → expression context`；箭头表示需要交接的身份责任，不表示已存在同名字段或静态调用边。

| 阶段 | 当前入口与实际边界 | 状态拥有者、生命周期与目标缺口 |
| --- | --- | --- |
| listener | PG 的 `PgService::start` 接受连接，`negotiate` 调用 `driver.open_ctx(id, 0, 45, "", None)`；MySQL 的 `ClientConn::dispatch` 经 `openSession` 取得上下文并分派 Query / Stmt 命令。 | PG service 拥有监听与工作线程，`ClientConn` 拥有 MySQL 连接状态；目标由连接所属入口确定身份，不能把 service 级共享对象变成可变当前方言。启动协议版本和握手版本字符串不是 SQL 方言身份或兼容证据。 |
| session | PG 每个连接循环创建一个 `Extended`，其 `session: PgSession` 被 Q 与扩展查询路径共用；`PgSession` 当前仅有 `path`、`failed_transaction`。`SessionDriver::open_ctx` 返回共享 `TiDBContext`。 | `Extended`、其语句及 portal 映射属于当前连接，连接循环结束后释放；`PgSession` 是真实局部状态入口，但没有全链路方言标记。目标将连接身份与可变 search_path/事务状态分开，关闭连接后不得被后续连接继承。 |
| parser | Q 先尝试 `SessionQuery::parse` 与 `CatalogQuery::parse_session`，普通路径经 `pg_name::adapt`、`pg_sql::adapt_with_context` 等适配后交给 `TiDBContext::execute_query`；扩展 Parse 在 `Extended::process` 做相应识别、适配与 `markers` 映射，再进入 `prepare_statement`。 | 当前受控识别及文本适配位于 PG 连接边界，不是完整 PG parser。目标每次解析显式获得本连接身份，生成的语句与预处理对象保留身份来源；解析器重用不能继承上一连接身份。共享 parser 内部的实际承载接口尚未在本块定位，后续需局部取证，不能从 SQL 文本或占位符反推身份。 |
| resolver | 当前可定位的名称入口是普通 SQL 的 `pg_name::adapt` 与目录查询的 `parse_session`，均收到当前 `PgSession`；共享 `TiDBContext` 接口没有显式方言参数。 | resolver 表示名称绑定阶段，不假定存在同名模块。目标名称绑定读取语句所属会话身份及适用的名称环境；该环境在解析/绑定/执行间的生效时点由后续规则定义。本块没有证明身份已传入共享名称绑定阶段，也不把边界名称适配当作该证明。 |
| planner | 普通 Q 的可定位共享入口为 `TiDBContext::execute_query`；普通扩展查询为 `prepare_statement`、`execute_prepared_statement`，参数是 SQL 或 statement ID/绑定值等，没有显式方言参数。 | 目标规划及计划重用保留产生该语句的方言语义；跨连接缓存不得混用两方言语义。当前只核实上述交接入口，未核实 planner 内部接线、缓存归属或键设计，不强定新增字段、类型及缓存接口。 |
| expression context | 普通执行仍由上述 `TiDBContext` 接口承接；`driver.rs::Expression` 是协议参数/结果值枚举，不是内核表达式求值上下文。 | 目标每条语句及其执行/重执行期间获得所属连接身份，函数分流和求值不得读取其他连接的可变状态。尚无共享表达式上下文接收 PG 身份的取证；具体落点交给后续局部实现调研，函数、转换的语义契约归工作流 3、4。 |

扩展查询目标要求 Parse、Bind、Execute、Describe、Close 与 Sync 始终属于同一连接身份；命名语句与 portal 不得跨连接解析、绑定或执行。简单查询也必须使用同一身份源。身份稳定不意味着冻结所有会话设置：现有 `pg_introspection_namespace_session_state` 的断言意图包括 Parse 后 SET，再执行 `current_schema()` 时观察当前会话值；search_path 状态执行和完整时点规则由工作流 5 承接，目录可见性由工作流 6 承接，本块不推广这条断言到全部名称绑定或计划失效行为。

当前证据固定为源码/独立测试阅读（基于仓库提交 `d55446b4a3`，本轮无行为执行）：[pg_session_test.rs](../pkg/server/pg_session_test.rs) 的 `pg_introspection_namespace_parser_boundaries` 断言有界会话语句的接受/拒绝；[pg_query_test.rs](../pkg/server/pg_query_test.rs) 的 `pg_introspection_namespace_session_state` 断言两 PG 连接 search_path 交错隔离及简单/扩展查询局部状态承接，`postgres_create_table_simple_query` 提供普通 Q 接入的调研入口。这些测试没有证明 PG/MySQL 同时连接时的 parser→resolver→planner→expression context 身份隔离，本轮也没有运行它们。`pkg/server/doc.go` 不存在，已读包内模块说明及源码；不以历史通过记录补当前证据。

以下为待新增的隔离验收用例族，沿第 1 章分别登记能力 ID、case ID、结果元数据和观测记录；表内是目标输入及结果，不是执行结果。成功项 SQLSTATE 均记“不适用（成功）”；没有身份专用 SQLSTATE，不凭空创造错误码。

| 场景与正反例 | PG 适用性、目标结果 | MySQL 适用性、目标结果 | 错误与证据责任 |
| --- | --- | --- | --- |
| DIALECT-IDENTITY-COEXIST：同时保持 PG A、MySQL M 活跃，交错执行 `SELECT 1 AS v`；PG 分别经 Q 及零参数 Parse/Bind/Execute，M 经普通查询及自身预处理协议。反例是 PG 查询改变 M 的后续解析身份。 | 适用；每次返回一行 v=1，所属连接身份始终 PG。 | 适用；每次返回一行 v=1，所属连接身份始终 MySQL；精确类型与协议元数据分别按工作流 3、7 补齐。 | 无错误位置（成功）；相同值本身不足证明身份隔离，后续待新增连接/语句上下文断言，覆盖六段责任链且不引入产品可见调试接口。全部未验证。 |
| DIALECT-IDENTITY-PG-PAIR：A、B 两条 PG 连接；A 设置 `search_path TO pg_catalog, public`，B 显式设置 `search_path TO public`，交错 `SHOW search_path`，Q 和扩展协议交叉使用，再各自关闭并重连。反例是 A 的设置、Sync 或断开改变 B。 | 适用；各自 SHOW 反映本连接显式设置，稳定身份均 PG；重连重新初始化，不能继承 A 的状态。具体默认值另按工作流 5 契约核对，不以现有 public 默认值代表 PG 完整默认行为。 | 不适用这些 PG 设置语句；保持 M 的 `SELECT 1 AS v` 作为旁路隔离检查，不向 M 投递 PG 设置并忽略错误。 | 成功项无错误位置；状态取值归工作流 5，身份隔离归本块。已有测试仅有局部断言意图；完整场景未验证。 |
| DIALECT-IDENTITY-ERROR：A 通过扩展 Parse 发送 `SELECT )`，随后 Sync，再执行 `SELECT 1 AS v`；B、M 同时继续各自正例。反例是 A 的失败恢复令 B/M 切换身份或执行 A 的语句。 | 适用；A 语法错误目标 42601，恢复后可继续，B 保持自己的状态与身份。 | 不向 M 发送此 PG 协议负例；M 的正例应继续返回 1，MySQL 语法错误码及恢复不由本块猜定。 | parser 负责原始 SQL 的定位，PG 协议边界输出 SQLSTATE 和错误字段；位置目标指向 `)`，精确参考端位置须补采集。`write_error` 当前只写 S/V/C/M，没有 P 字段；不能宣称已支持错误位置。恢复契约协作工作流 7，全场景未验证。 |

外部目标依据为 PostgreSQL **18** 官方手册：§54.2.2/§54.2.3 [简单与扩展查询消息流](https://www.postgresql.org/docs/18/protocol-flow.html)，用于区分查询生命周期、预处理语句与 portal 的责任；§54.8 [错误字段](https://www.postgresql.org/docs/18/protocol-error-fields.html)，规定 P 若提供须基于原始查询、从 1 起按字符计数，不能直接返回改写后字节偏移；[附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html) 给出 42601 的语法错误分类。文档于 2026-10-23 查阅，未运行参考端；18.6 仍仅为来源指定目标，无该补丁版本实测结论。连接私有身份及禁止可变全局切换是 AsterSQL 双入口架构目标，不把它表述为 PostgreSQL 手册中的内部接口要求。

#### 类型转换语法边界

本块对应源工作流 2 步骤 2 的 `::` 分类，继承[方言身份传递](#方言身份传递)的连接私有身份契约。目标只确定表达式输入、优先级、嵌套、参数位置和诊断保真；类型解析后的转换可行性、舍入、溢出、NULL、结果类型与元数据由工作流 3 定义。本轮仅交付文档，以下用例均未运行，不能据此宣称已经支持。

当前源码证据基于提交 `a19b8cba62`：`Parser::ParseSQL` 在 `yy_parser.rs` 中让 `Scanner` / `yyParse` 决定语法接受与错误，`ParseOneStmt` 限定单语句，`yySetOffset` 向表达式记录原始偏移；现有 MySQL SQL mode 不能当作已存在的 PG 身份入口。`expressions.rs` 的 `ParenthesesExpr`、`BinaryOperationExpr`、`ParamMarkerExpr` 提供括号、运算树和参数顺序/偏移的调研入口；不能把该文件名当成 PG cast 节点定义。实际 `ast/lib.rs::ExprKind::Cast` 已有 `Expr`、`Tp: FieldType`、`FunctionType`、`ExplicitCharSet`，同一枚举有 `Parentheses` 和带 `Offset` 的 `ParamMarker`；这些字段说明可复用候选，不证明已能保留 PG 原始类型拼写、引用规则或 `::` 源位置。

`Extended::process` 的普通 Parse 路径先做有界适配，再调用 `markers` 和共享 `prepare_statement`；`markers` 仅将引号/注释之外的 `$n` 映射到 `?` 并返回零基参数索引序列，不解析转换语法。当前普通扩展路径在参数 OID 缺省/为零时返回 0A000，要求显式 OID；因此 `($1)::int4` 的语法目标不能自动升级为已支持从转换推断参数类型。目录/会话专用识别分支也不能证明通用 SELECT 转换支持。已阅读独立测试 [parser_test.rs](../pkg/parser/parser_test.rs) 的 `test_parser_err_msg`（MySQL near/line 与注释诊断），以及 [pg_extended_test.rs](../pkg/server/pg_extended_test.rs) 的 `marker_mapping_preserves_literals_comments_and_index_order`（字面量、注释、重复/乱序参数保护及拒绝边界）、`canonical_prepare_parameter_syntax_probe`（共享入口 `?` 与 `$1` 的断言意图）；本轮未执行测试，所读断言不构成 `::` 的通过证据。`pkg/parser/doc.go`、`pkg/parser/ast/doc.go`、`pkg/server/doc.go` 均不存在。

承载方案留待后续最小语法取证：优先评估复用现有 Cast；若需要独立受控 PG 解析入口，其输出仍须可交给共享内核。两种方案都应保留以下信息，未定位的新接口不在此命名或假定存在：

- 操作数表达式树、显式括号及每层转换的顺序；`::` 左结合，不能把链式转换合并为最后一个目标类型，也不能把 `1 + 2::int4` 解析成 `(1 + 2)::int4`。
- 目标类型的原始拼写、限定名分段、引号与大小写信息，以及出现时的类型修饰参数、数组维度等语法信息；这是保真需求，不承诺本块实现完整类型语法、数组或自定义类型。支持集合及类型绑定交工作流 3，不能直接将任意 PG 类型名映射为 MySQL `FieldType` 后丢弃来源。
- 原始 SQL 中操作数、`::`、类型及参数的跨度、原始参数编号与出现顺序，并继承所属连接身份；改写时需有可回溯原 SQL 的位置映射。重复 `$1` 仍绑定同一个外部参数，不能因转换嵌套重排、复制绑定值或将值拼入 SQL。
- 词法层保护字符串、带引号标识符及注释中的 `::` / `$1`。不能用全局文本替换把 `::` 通用翻译为 CAST，也不能删除字符串/注释中的片段；现有 `markers` 拒绝 dollar quote 且不承诺 PG 嵌套块注释，后续要单独取证并明确拒绝边界，不能静默按 MySQL 词法解释。

下表是待新增验收用例族的输入契约，后续按第 1 章登记能力 ID 与 case ID。PG 成功项 SQLSTATE 为“不适用（成功）”，无错误位置；结果值只是目标，精确类型/OID及列元数据交工作流 3、7 补齐。所有实际输出与参考端错误位置均待采集。

| 输入与正反例 | PG 目标 | MySQL 对照与适用性 | 错误位置、责任与证据状态 |
| --- | --- | --- | --- |
| 正例 `SELECT 1::int4` | 解析为整数常量的显式转换，目标值 1；类型名 int4 是 PG 既有名称。 | 原 SQL 作为方言隔离负例，应拒绝 `::`；自身正例 `SELECT CAST(1 AS SIGNED)` 目标值 1，不要求与 PG 类型元数据相同。 | PG 成功；MySQL 负例定位冒号附近，精确诊断/码待参考端及本地取证；双方未验证。 |
| 正例 `SELECT ($1)::int4`，扩展 Parse/Bind/Execute，绑定整数 1 | 保留括号、参数编号 1、转换目标，目标值 1。分别覆盖显式 OID 与省略 OID 后由类型上下文推断；后者为目标缺口。 | 原 SQL 不作 MySQL 正例；自身预处理正例 `SELECT CAST((?) AS SIGNED)` 绑定 1，目标值 1。 | 参数不在普通 Q 文本中直接绑定；PG Parse 语法与类型推断分责，Bind 值解码另验收。当前 OID 限制如上，两协议均未运行。 |
| 优先级正例 `SELECT 1 + 2::int4` 与 `SELECT (1 + 2)::int4` | 分别为右操作数转换与括号和式转换，值均目标为 3；必须检查不同树形，不能仅比较相同结果。 | `::` 均为负例；自身正例分别为 `SELECT 1 + CAST(2 AS SIGNED)`、`SELECT CAST((1 + 2) AS SIGNED)`。 | 语法树责任归本块，求值归工作流 3、4；无当前 AST 或运行证据。 |
| 嵌套正例 `SELECT 1::int4::int8` 与 `SELECT ((1)::int4)::int8` | 前者按左结合，均保留两层转换，目标值 1；不能直接消去内层转换。 | `::` 均为负例；自身正例 `SELECT CAST(CAST(1 AS SIGNED) AS SIGNED)` 仅对照嵌套形态，不宣称两层类型等价。 | 类型链执行及元数据归工作流 3；树形与位置待新增断言，未验证。 |
| 参数位置正例 `SELECT ($2)::int4, ($1)::int4, ($2)::int4`，绑定 1、2 | 目标行 `(2,1,2)`，映射顺序为 `[1,0,1]`，各转换保留来源位置。 | 自身正例 `SELECT CAST(? AS SIGNED), CAST(? AS SIGNED), CAST(? AS SIGNED)` 按出现顺序绑定 2、1、2；不把 MySQL 的三个位置冒充 PG 两个参数。 | 现有 marker 测试仅覆盖一般映射意图，带转换的完整 Parse/Bind 仍未验证。 |
| 缺目标类型负例 `SELECT 1::`、`SELECT ($1)::`（后者走扩展 Parse） | 拒绝不完整转换，语法错误目标 42601；不能忽略 `::` 或退化为普通表达式。 | 同样不是 MySQL 合法转换；自身缺类型负例 `SELECT CAST(1 AS)` 应拒绝。 | PG 定位缺失类型的输入末尾，精确字符位置待参考端采集；MySQL 可更早在冒号处拒绝，不能强求错误位置相等。parser 保留原 SQL 定位，PG 协议边界输出错误字段。 |
| 词法保护正例 `SELECT '1::int4 $1' /* :: $2 */`，以及扩展输入 `SELECT ($1) /* :: $2 */ ::int4` 绑定 1 | 前者字符串值原样保留且无参数；后者仅绑定一个参数，目标值 1，注释不产生转换/参数节点。 | 前者可作 MySQL 自身正例；后者 `::` 是负例，对照用 `SELECT CAST((?) /* :: $2 */ AS SIGNED)`。 | PG/MySQL 仅按各自词法解释；成功无位置。dollar quote、嵌套注释另建边界负例/支持用例前须取证，本轮未验证。 |

官方依据（2026-10-23 查阅）为 PostgreSQL **18** §4.2.9 [Type Casts](https://www.postgresql.org/docs/18/sql-expressions.html#SQL-SYNTAX-TYPE-CASTS)（CAST 与 `::` 两种形式）、§4.2.2 [位置参数](https://www.postgresql.org/docs/18/sql-expressions.html#SQL-EXPRESSIONS-PARAMETERS-POSITIONAL)、§4.1.6 [运算优先级](https://www.postgresql.org/docs/18/sql-syntax-lexical.html#SQL-PRECEDENCE)（`::` 左结合且高于加法）、§8.1 [数值类型](https://www.postgresql.org/docs/18/datatype-numeric.html)（int4/int8）、§54.8 [错误字段](https://www.postgresql.org/docs/18/protocol-error-fields.html)（P 基于原查询、从 1 起按字符定位）与[附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html)（42601）。MySQL 对照采用 **8.4** §14.10 [Cast Functions and Operators](https://dev.mysql.com/doc/refman/8.4/en/cast-functions.html)，仅作自身 CAST 形式的文档依据，不替代实际参考版本选择或运行记录。语法错误、未知类型和转换失败须按阶段区分，不能全映射为 42601；后两者具体目标码由工作流 3 按能力取证。本轮未运行真实数据库，18.6 仍为来源指定目标，无该补丁版本实测结论。

#### 大小写匹配语法边界

本块对应源工作流 2 步骤 2 的 ILIKE 分类，继承[方言身份传递](#方言身份传递)的连接身份契约；只定义 `ILIKE`、`NOT ILIKE`、`ESCAPE` 的语法承载、优先级和诊断责任。匹配算法、locale/collation、NULL、类型转换及运算符重载交工作流 3、4；名称大小写折叠由后续名称规则块定义。以下目标与当前静态证据分开登记，所有运行结果均未验证。

当前源码证据基于提交 `5780b7041ac54cf696b55a9a43fd170e47a4cfc0`：Rust `PatternLikeOrIlikeExpr` 已有 `expr`、`pattern`、`not`、`is_like`、`escape: u8`、`escape_explicit` 及模式缓存字段；这是可复用载体，不证明 PG 输入已完整接线。未纳入图索引的 `parser.y` 有 `ilike` token、`IlikeOrNotOp` 和 `PredicateExpr` 分支，以 `IsLike: false`、`Not` 区分操作类别和否定；`LikeOrIlikeEscapeOpt` 只接收字符串字面量，缺省为反斜杠且 explicit=false，显式空串产生零 escape，超过一个字节时报 ESCAPE 参数错误。该文件含 Go 语义动作，不能冒充 Rust parser 的当前通过证据；字节转义载体也不能直接宣称覆盖 PG 的字符/表达式转义边界。优先级声明将 like/ilike 与比较操作符放在同层，而 PG 18 表 4.2 将模式匹配置于比较之上，因此不能照搬共享声明即宣称 PG 优先级已对齐。

`pg_name::source_tokens` / `rewrite` 保留非编辑区的原 SQL，主要改写关系名和引号；`adapt` 无原生关系时可直接返回改写结果，不是 ILIKE 的完整解析器或求值器。已阅读独立测试 [parser_test.rs](../pkg/parser/parser_test.rs) 的 `test_high_not_precedence_mode`（用 BETWEEN 检查 MySQL SQL mode 下 NOT 树形）和 `test_parser_err_msg`（near/line 与注释诊断），以及 [pg_name_test.rs](../pkg/server/pg_name_test.rs) 的 `pg_introspection_names_token_scopes`（字面量/注释保护及名称改写）、`pg_introspection_names_rejection_boundaries`（引号、嵌套注释等拒绝边界）。这些用例没有给本块 ILIKE 提供专项通过证据，本轮未执行测试；`pkg/parser/doc.go`、`pkg/parser/ast/doc.go`、`pkg/server/doc.go` 均不存在。

承载选择留待最小局部取证：优先比较复用共享 AST 与增加受控 PG 解析入口；前者须核对 Rust parser 实际构造、访问/恢复及共享消费者是否保真，后者须将相同信息交给共享内核，并保持原 SQL 位置。两方案都必须保留操作符类别、左操作数、右模式表达式、否定标志、ESCAPE 是否显式出现及其原始表达式/文本与跨度；省略 ESCAPE 与 `ESCAPE ''` 不得合并。现有 u8 字段不足时只规划必要的表示扩展，不在此规定新接口或补建匹配系统。不能把 ILIKE 无条件替换为 LIKE，也不能全局改写字符串/注释中的关键字，不能用 lower() 包装替代 PG 匹配契约。

PG 目标按模式匹配高于比较、IS、NOT、AND、OR 的顺序保留树形；`NOT ILIKE` 的 NOT 属于该谓词，前置逻辑 NOT 仍需保留其外层位置，括号可显式改变组合。MySQL SQL mode 的 NOT 优先级不能泄漏到 PG 连接。ESCAPE 只关联所属匹配谓词，表达式转义的完整允许集合由后续对照取证；当前字面量规则不能被当成完整 PG 规则。`ILIKE ANY`、符号别名 `~~*` / `!~~*` 及其他匹配家族不纳入本块承诺，后续单独登记，不能借本小节推导已支持。

下表为待新增用例族，按第 1 章分别登记 PG/MySQL 能力 ID 与 case ID。PG 成功项 SQLSTATE 均为“不适用（成功）”，无错误位置；PG 布尔类型与协议元数据交工作流 3、7。MySQL 原 ILIKE 输入作为隔离负例，目标拒绝；共享语法已有扩展候选，实施前须核对现有 MySQL 入口基线并显式解决差异，不在此改动它。MySQL 自身 LIKE 对照不视为 ILIKE 的等价翻译，大小写结果依赖所选 collation，未指定时不猜值或错误码。所有场景均未实测。

| 输入与正反例 | PG 目标 | MySQL 适用性与目标 | 错误位置、责任与证据状态 |
| --- | --- | --- | --- |
| 正例 `SELECT 'Ab' ILIKE 'a%'` | 在支持 ASCII 大小写折叠的确定性 locale/collation 前提下，接受匹配谓词，目标 true；前提需在后续参考环境明确记录。 | 原 SQL 为隔离负例；自身正例 `SELECT 'Ab' LIKE 'a%'` 接受，值待选定 collation 后按工作流 4 取证。 | PG 成功；MySQL 拒绝目标在 ILIKE 附近，精确 SQLSTATE/位置待参考端及本地核实。双方未验证。 |
| 正例 `SELECT 'Ab' NOT ILIKE 'a%'` 与 `SELECT NOT ('Ab' ILIKE 'a%')` | 同上述前提，均目标 false；前者保留谓词否定，后者保留外层 NOT 与括号，不能只凭相同结果证明树形。 | 原 SQL 均为负例；自身 `SELECT 'Ab' NOT LIKE 'a%'` 与 `SELECT NOT ('Ab' LIKE 'a%')` 接受，结果按自身 collation 核实。 | PG 成功；MySQL 原输入的精确拒绝码/位置未验证。 |
| 优先级正例 `SELECT 'Ab' ILIKE 'a%' = TRUE`、`SELECT NOT 'Ab' ILIKE 'a%' AND TRUE` | 分别为匹配结果再比较、外层 NOT 作用于匹配后再 AND；上述前提下目标 true、false。需树形断言，不能把比较纳入模式操作数或将 NOT 作用于左字符串。 | 原 SQL 为负例；MySQL 自身组合以 LIKE 取代关键字单独登记，保留其 SQL mode 语义，不强求 PG 树形。 | 语法组合归本块，求值归工作流 4；当前没有专项 AST 或运行证据。 |
| ESCAPE 正例 `SELECT 'A_b' ILIKE 'a!_b' ESCAPE '!'`，及 `SELECT 'Ab' NOT ILIKE 'a%' ESCAPE '!'` | 保留显式转义及否定；上述前提下目标 true、false。 | 原 SQL 为负例；自身 LIKE / NOT LIKE 配合 `ESCAPE '!'` 接受，值按自身 collation 验收。 | PG 成功；两方言分别核对转义原文与结果，未验证。 |
| 区分省略与空串：`SELECT 'a_b' ILIKE 'a!_b'` 和 `SELECT 'a_b' ILIKE 'a!_b' ESCAPE ''` | 两者都接受但 ESCAPE 出现状态不同；显式空串表示禁用转义。本行仅验收语法信息，不扩写匹配算法。 | 原 SQL 为负例；自身 LIKE 的省略/空串受 MySQL SQL mode 约束，需记录 NO_BACKSLASH_ESCAPES 后另取目标，不直接套用 PG 默认值。 | PG 成功，MySQL 自身形式的具体值/拒绝码待取证；均未验证。 |
| 缺右操作数负例 `SELECT 'Ab' ILIKE`、`SELECT 'Ab' NOT ILIKE`；缺转义负例 `SELECT 'Ab' ILIKE 'a%' ESCAPE` | 拒绝不完整谓词，目标语法错误 42601；不得吞掉 ILIKE、NOT 或 ESCAPE 后执行剩余表达式。 | 原 SQL 应拒绝，可早在 ILIKE 附近失败；自身负例 `SELECT 'Ab' LIKE`、`SELECT 'Ab' LIKE 'a%' ESCAPE` 同样拒绝。 | PG 定位缺失表达式的输入末尾，精确字符位置待参考端采集；MySQL 不要求同位置/同码。parser 保留原 SQL 跨度，协议工作流 7 输出 SQLSTATE/P，当前未验证。 |
| 词法保护正例 `SELECT 'ILIKE NOT ILIKE ESCAPE' /* ILIKE */` | 返回原字符串，字符串/注释不产生匹配节点。 | 自身正例，返回原字符串；无 ILIKE 运算符。 | 双方言成功、SQLSTATE 不适用；现有名称测试只覆盖类似保护意图，本输入仍未验证。 |

官方依据（2026-10-23 查阅）为 PostgreSQL **18** §9.7.1 [LIKE 与 ILIKE](https://www.postgresql.org/docs/18/functions-matching.html#FUNCTIONS-LIKE)（ILIKE、否定及 ESCAPE 规则）、§4.1.6 [操作符优先级](https://www.postgresql.org/docs/18/sql-syntax-lexical.html#SQL-PRECEDENCE)、§54.8 [错误字段](https://www.postgresql.org/docs/18/protocol-error-fields.html)（P 从 1 起按原始 SQL 字符定位）与[附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html)（42601）。MySQL 自身形式参考 **8.4** §14.8.1 [字符串比较操作符](https://dev.mysql.com/doc/refman/8.4/en/string-comparison-functions.html)，不以其替代本地扩展基线取证。多字符或非字面量 ESCAPE 的合法性、类型/求值错误及其精确 SQLSTATE 尚未取证，由工作流 3、4 补齐；不能统一按 42601 处理。本轮没有数据库或该输入的行为执行证据，18.6 仍仅为来源指定目标，无补丁版本实测结论。

#### 返回子句语法边界

本块对应源工作流 2 步骤 2 的 RETURNING 分类，继承[方言身份传递](#方言身份传递)的连接身份；只定义 INSERT/UPDATE/DELETE 的子句位置、投影保真、语法拒绝及 planner/结果描述交接。真实写入、返回行求值与事务副作用归工作流 5，类型归工作流 3，wire 编码归工作流 7。以下是待实现、待验收契约，不能以删除 RETURNING 后执行原语句、追加 SELECT 或只返回受影响行数冒充成功。

当前静态证据基于提交 `2ca07d8705eeaba2fda8b2f7823468147b7c2e1d`：指定入口 `ast/dml.rs` 是 DML 基础类型，实际共享 `InsertStmt`、`UpdateStmt`、`DeleteStmt` 在 [ast/lib.rs](../pkg/parser/ast/lib.rs)，三者均有 `Returning: Vec<SelectField>`。`SelectField` 提供 `Expr`、`WildCard`、`AsName`、`Offset`、`OriginalText`，可作为复用候选，但不能由这些字段推导 PG 原文、跨度或所有消费者已保真。未索引的 `parser.y` 中 `ReturningClause` 为省略或 `RETURNING FieldList`，`InsertIntoStmt` 将其放在 `OnDuplicateKeyUpdate` 后，`UpdateStmtNoWith` 与单表 `DeleteWithoutUsingStmt` 在尾部承接；这些是带 Go 动作的共享语法，不能直接证明 PG 的 ON CONFLICT、UPDATE FROM、DELETE USING 或 Rust 接线已支持。

`pg_result::command` 在共享 ParseSQL 后，对上述三类 AST 的非空 Returning 明确返回 `0A000`；语法解析错误在该入口映射为 `42601`。这表示现有 PG 命令门禁有拒绝分支，不表示本轮已运行验证。`encode_formats` 以 `QueryResult.columns` / `native_types` 描述列并编码行，DML completion 使用 affected_rows；目前源表 OID/属性号填零、typmod 填 -1，不能据此声称完整 RETURNING 元数据已实现。已读 [parser_test.rs](../pkg/parser/parser_test.rs) 的 `test_parser_err_msg`（near/line 诊断）、[pg_query_test.rs](../pkg/server/pg_query_test.rs) 的 `simple_query_roundtrip`（普通 DML completion 与 SELECT 元数据）和 `simple_query_encoding_rejects_unrepresentable_results`（编码及命令拒绝意图）；它们不是本块三类 RETURNING 的专项通过证据。局部补读 [parser_actions/dml_test.rs](../pkg/parser/parser_actions/dml_test.rs) 的 `dml_delete_single_table_keeps_go_modifiers_and_returning`、`dml_insert_and_replace_keep_go_hints_and_returning`，仅有 Returning 长度等断言，未覆盖本契约的完整投影、UPDATE、PG 隔离及副作用。本轮未运行任何测试；`pkg/parser/doc.go`、`pkg/parser/ast/doc.go`、`pkg/server/doc.go` 均不存在。

PG 目标位置为 INSERT 的数据源及可选 ON CONFLICT 之后，UPDATE 的 SET、可选 FROM/WHERE 之后，DELETE 的可选 USING/WHERE 之后；不能照搬共享 MySQL ORDER BY/LIMIT 等扩展顺序。ON CONFLICT 本身由下一块定义，本块只保留衔接位置。投影按顺序保留表达式树、`*`/限定星号、显式或省略 AS 的输出别名及其原始拼写/引号、原 SQL 字符跨度；区分没有子句和出现 RETURNING 但投影为空。字符串和注释中的关键字不产生子句，不以文本截断或全局替换接入。

承载方案留待后续最小取证，比较共享 AST 与受控 PG 入口，不能凭候选字段定新接口。解析负责子句及字段保真；resolver 负责列、限定名、星号绑定，planner 负责保留投影及列顺序，绑定/类型阶段产生输出列名、类型、可空性及可用的来源信息，交工作流 3、7 描述。Describe 不得为获取元数据而执行写入；零行命中也有已确定的列结构。执行阶段由工作流 5 验证 INSERT/UPDATE 默认返回新行、DELETE 默认返回删除前行，并分别验证提交/回滚与受影响行数。PG 18 还含 OLD/NEW 引用及 `RETURNING WITH (OLD AS o, NEW AS n)`；本块不承诺其实现，后续须单独取证及明确支持集合，未支持合法形式以 0A000 在副作用之前拒绝，不能误称 PG 语法非法。MERGE、触发器和规则的实现不在本块范围。

下表为待新增验收用例族；各行独立初始化普通表 `t(id integer)`，无触发器、规则或默认值。UPDATE/DELETE 初始仅一行 id=1，INSERT 初始为空；具体 schema/连接前提由后续用例记录。PG 正例成功时 SQLSTATE 不适用、无错误位置；精确 OID/类型修饰等交工作流 3、7。MySQL 原 RETURNING 输入均作为隔离负例，拒绝目标在 RETURNING 附近，精确码/位置待参考端采集；自身去掉子句的 DML 是合法对照，仅验证自身副作用，不能作为 PG 返回行验收。共享语法已有 RETURNING 扩展候选，实施前须取证现有 MySQL 入口基线并解决差异，不能从 MySQL 官方语法推导本地已拒绝。所有行为均未实测。

| 输入与正反例 | PG 解析与列描述目标 | 执行交接及 MySQL 适用性 | 错误位置、责任与证据状态 |
| --- | --- | --- | --- |
| INSERT 正例 `INSERT INTO t(id) VALUES (1) RETURNING id` | 保留一个列投影，输出列名 id，类型取自绑定列。 | 工作流 5 目标返回 `(1)` 且仅插入一行；MySQL 原输入为负例，自身 INSERT 无返回投影。 | PG 成功目标；当前存在 0A000 门禁，未验证。 |
| UPDATE 正例 `UPDATE t SET id = id + 1 WHERE id = 1 RETURNING id + 10 AS next_id` | 保留算术表达式与别名 next_id，不能误取 SET 表达式或丢弃 WHERE。 | 工作流 5 目标返回 `(12)`，存储为 id=2；MySQL 原输入为负例，自身 UPDATE 后独立查询验证修改。 | PG 成功目标；列名、绑定类型及真实写入分别验收，未验证。 |
| DELETE 正例 `DELETE FROM t WHERE id = 1 RETURNING *` | 保留星号，由绑定阶段按表列顺序展开为 id；不得由返回行推断列数。 | 工作流 5 目标返回删除前 `(1)` 且表为空；MySQL 原输入为负例，自身 DELETE 验证删除。 | PG 成功目标；当前无全链路通过证据。 |
| 多投影/别名正例 `INSERT INTO t(id) VALUES (1) RETURNING id, id + 1 AS next_id, id label` | 三项有序，保留表达式、显式 AS 与省略 AS；输出列名 id、next_id、label。 | 工作流 5 目标行 `(1,2,1)` 且仅写入一次；MySQL 原输入为负例。 | PG 成功目标；树形、列顺序及副作用须独立断言，未验证。 |
| 零行正例 `UPDATE t SET id = 2 WHERE id = 9 RETURNING id` | 仍有 id 列结构，不能退化成无列命令。 | 工作流 5 目标返回零行、原表不变；MySQL 原输入为负例，自身 UPDATE 为零行命中对照。 | PG 成功目标；Describe/Execute 一致性待取证。 |
| 空投影负例 `INSERT INTO t(id) VALUES (1) RETURNING`、`UPDATE t SET id = 2 RETURNING`、`DELETE FROM t RETURNING` | 拒绝缺失投影，PG 目标 42601；不得当作省略子句。 | 解析失败不得执行写入；MySQL 原输入同为负例，可更早在 RETURNING 拒绝。 | PG 指向缺失投影的输入末尾，精确 P 待参考端采集；parser 保留原 SQL 定位，工作流 7 输出错误字段。 |
| 错位负例 `UPDATE t SET id = 2 RETURNING id WHERE id = 1` | 拒绝 RETURNING 后错置的 WHERE，PG 目标 42601。 | 不得删去尾部或子句后执行 UPDATE；MySQL 原输入为负例。 | PG 定位 WHERE 附近，精确字符位置待取证；未验证。 |
| 词法保护正例 `INSERT INTO t(id) VALUES (1) /* RETURNING */` 与 `SELECT 'RETURNING'` | 前者无返回投影，后者普通字符串投影；不制造 Returning 节点。 | 两方言自身正例；前者只插入，后者返回原字符串。 | 成功时 SQLSTATE 不适用；专项保护与无误改写证据待新增。 |

官方依据（2026-10-23 查阅）为 PostgreSQL **18** §6.4 [Returning Data from Modified Rows](https://www.postgresql.org/docs/18/dml-returning.html) 及 SQL 命令 [INSERT](https://www.postgresql.org/docs/18/sql-insert.html)、[UPDATE](https://www.postgresql.org/docs/18/sql-update.html)、[DELETE](https://www.postgresql.org/docs/18/sql-delete.html) 的 Synopsis/输出参数；错误依据为 [附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html) 的 42601/0A000 与 §54.8 [错误字段](https://www.postgresql.org/docs/18/protocol-error-fields.html) 的原查询字符位置 P（从 1 起）。MySQL 自身语法依据为 **8.4** §15.2.7 [INSERT](https://dev.mysql.com/doc/refman/8.4/en/insert.html)、§15.2.17 [UPDATE](https://dev.mysql.com/doc/refman/8.4/en/update.html)、§15.2.2 [DELETE](https://dev.mysql.com/doc/refman/8.4/en/delete.html)，其语法未列 RETURNING，不替代本地扩展及实际参考端取证。合法但未支持、语法非法、列绑定/类型失败必须分阶段，列绑定/类型失败的具体错误码待相关工作流采集，不能全部映射为 42601。18.6 仍仅为来源指定目标，本轮无该补丁版本或真实数据库实测结论。

#### 冲突子句语法边界

本块对应源工作流 2 步骤 2 的 ON CONFLICT 分类，只定义语法信息与名称绑定交接。PG 的冲突目标、动作及谓词不能直接等同于 MySQL `ON DUPLICATE KEY UPDATE`，也不能改写为 `INSERT IGNORE`、`REPLACE` 或空赋值列表。唯一性推断、并发、锁、更新执行及事务副作用归工作流 5；本块没有开放 upsert 能力。

当前证据为基于提交 `391d3895b8` 的源码和独立测试阅读，本轮没有运行行为验证。`ast/dml.rs` 的 `TableName`、`Join::restore` 是共享表/连接基础类型；实际 `InsertStmt` 位于 `ast/lib.rs`，其 `OnDuplicate: Vec<Assignment>` 仅提供赋值列表，`Assignment` 包含 `Column` / `Expr`，不能区分“无冲突子句”与 DO NOTHING，更没有 PG 冲突目标、目标谓词和更新条件的独立字段。`parser.y::InsertIntoStmt` / `OnDuplicateKeyUpdate` 将 MySQL 子句写入 OnDuplicate；这是 Go 语义动作的语法证据，不是 Rust PG 解析接线或成功证据。该语法文件未入图，直接读取核对。`pg_extended.rs::Extended::process` 的 Parse 分支经会话/目录识别、名称和 SQL 适配、`markers` 后调用 `prepare_statement`，未在此提供 PG 冲突 AST 或原 SQL 跨度映射；`markers` 的参数映射不能替代诊断位置映射。

已读 [parser_test.rs](../pkg/parser/parser_test.rs) 的 `test_insert_statement_memory_allocation`，断言普通 INSERT 的 1001 行及分配上限；已读 [pg_query_test.rs](../pkg/server/pg_query_test.rs) 的 `simple_query_roundtrip` 普通 INSERT/UPDATE/DELETE 命令完成及结果断言、`postgres_create_table_simple_query` 的建表入口。这些断言不覆盖 ON CONFLICT，不能作为本块通过证据。`pkg/parser/doc.go`、`pkg/server/doc.go` 均不存在；当前未证明 PG 冲突子句的接受、拒绝、错误码或位置，不从缺字段推断实际报错。

目标承载方案仍待本章解析方案任务比较共享 AST 扩展与受控 PG 入口；不预设新增类型或方法名。无论选择哪条路径，交接至少保留：

| 语法信息 | 目标保真及交接责任 |
| --- | --- |
| 子句及动作 | 区分未出现、DO NOTHING、DO UPDATE；保留 ON CONFLICT、DO、动作和 SET 的原 SQL 跨度。不能用空 OnDuplicate 隐去 DO NOTHING。 |
| 冲突目标 | 区分省略、索引推断列表、ON CONSTRAINT 名称；列表保留列与带括号表达式、顺序、显式 COLLATE/opclass 及各自跨度。DO NOTHING 可省略目标，DO UPDATE 必须指定目标；目标可解析不表示约束存在或可用。 |
| 两类谓词 | 推断列表后的 WHERE 属于目标谓词，SET 后的 WHERE 属于更新条件，分别保留表达式树、出现性、括号及跨度；不能合并为 INSERT/SELECT 的过滤条件。 |
| 更新与名称 | 保留有序赋值、单列/多列目标、DEFAULT/表达式/子查询形态；保留 `excluded.v` 与目标表或别名引用的限定名和引号，交名称绑定区分拟插入行与现有行，不能文本替换成 MySQL VALUES 或普通表。目标表名为 excluded 时显式别名的遮蔽关系须交后续名称任务核对。 |
| 原查询与协议 | Q 与扩展 Parse 使用同一 PG 契约；保留 `$n` 的身份、重复/乱序映射及原查询字符跨度。合法但暂不支持的形态须明确拒绝，不能丢字段后执行；错误传输和恢复交工作流 7。 |

下列均为待新增的 `CONFLICT-SYNTAX` 用例族，按第 1 章登记能力 ID/case ID；不是已实现测试。各例假设 `t(id, v, active)` 可绑定；需要执行冲突时再由工作流 5 创建合适唯一约束/索引和初始行。所有成功项 SQLSTATE 为“不适用（成功）”、无错误位置，均尚未运行。

| 正反例输入 | PG 目标结果与 AST 验收 | MySQL 适用性、错误及证据责任 |
| --- | --- | --- |
| 正例：`INSERT INTO t(id,v) VALUES (1,2) ON CONFLICT DO NOTHING`；再比较显式 `(id)` 与 `ON CONSTRAINT t_pkey` 目标 | 接受相应语法；分别保留省略、索引列、约束名及 DO NOTHING，不能等同于无子句。执行时忽略何种冲突由工作流 5 验收。 | PG 写法在 MySQL 契约下应拒绝；具体 MySQL SQLSTATE/位置待实际参考端与本地基线取证，不套用 PG 码。 |
| 正例：`INSERT INTO t AS dst(id,v,active) VALUES (1,2,true) ON CONFLICT (id) WHERE active DO UPDATE SET v=excluded.v WHERE dst.v<>excluded.v` | 接受语法；目标 WHERE 和更新 WHERE 分属两棵树，SET 中 excluded 与 dst 引用保持可绑定身份；冲突条件成立/不成立的真实写入与不更新结果另交工作流 5。 | 此 PG 形式应拒绝；MySQL 诊断仍待取证。不能仅删除目标或 WHERE 后转成 MySQL 更新。 |
| 正例形态：`ON CONFLICT ((lower(v)) COLLATE "C" text_ops) WHERE active DO NOTHING`（接在相应 INSERT 后） | 保留表达式括号、collation/opclass 名称及目标谓词；只验收解析保真，名称是否存在、类型及索引可用性另行取证，不承诺执行成功。 | PG 目标形式不适用 MySQL；目标名字有效性和相关错误码由名称/类型及工作流 5 补齐。 |
| 反例：`INSERT INTO t(id,v) VALUES (1,2) ON CONFLICT (id)`；以及尾部 `DO`、`DO UPDATE SET` | 缺少动作或赋值，PG 语法拒绝目标 42601；定位缺失项后的结束位置（或实际非法 token），解析器记录本子句跨度，精确 P 值待参考端采集。 | 同样不是合法 MySQL 冲突更新形式；MySQL 码与位置独立取证。禁止静默退化为普通 INSERT。 |
| 反例：`INSERT INTO t(id,v) VALUES (1,2) ON CONFLICT DO UPDATE SET v=excluded.v` | 缺少 DO UPDATE 所需冲突目标，应拒绝；具体参考端诊断阶段、SQLSTATE 与 P 是否提供须后续采集，不能只凭 Synopsis 的可选目标猜码。 | 不适用 MySQL；拒绝和定位单独验收，不作为 MySQL upsert 正例。 |
| MySQL 正例/PG 反例：`INSERT INTO t(id,v) VALUES (1,2) ON DUPLICATE KEY UPDATE v=3` | PG 方言目标语法拒绝 42601，错误跨度指向不属于 PG 子句的关键字，精确 P 待采集。 | MySQL 原生语法接受目标，OnDuplicate 保留赋值；真实冲突更新与 affected rows 交工作流 5，不能要求与 PG DO UPDATE 的结果天然一致。 |
| 词法及参数保护：字符串/注释中出现 `ON CONFLICT`、`excluded.v`；扩展 Parse 中 VALUES/SET/两类谓词含重复或乱序 `$1`、`$2` | 字符串/注释不产生冲突节点，实际参数绑定位置不因子句改动错位；按合法上下文构造完整 SQL，断言树、参数身份与原 SQL 跨度。 | MySQL 仅使用自身合法 INSERT/冲突更新及 `?` 参数，交错连接不继承 PG 解析身份；双方均未验证。 |

官方规则（2026-10-23 查阅）依据 PostgreSQL **18** SQL 命令 [INSERT 的 Synopsis 与 ON CONFLICT Clause](https://www.postgresql.org/docs/18/sql-insert.html)：目标、动作、两类谓词及 excluded 的责任分别保留。错误类别依据 [附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html)，错误位置依据 §54.8 [Error and Notice Message Fields](https://www.postgresql.org/docs/18/protocol-error-fields.html) 的 P：原查询从 1 起按字符计数，不直接使用改写后字节偏移。合法未支持路径的目标拒绝码为 0A000，不能冒充参考 PG 语法错误；本轮未证明当前门禁。MySQL 对照依据 **8.4** §15.2.7.2 [INSERT ... ON DUPLICATE KEY UPDATE](https://dev.mysql.com/doc/refman/8.4/en/insert-on-duplicate.html)，独立保留原生契约。18.6 仍仅为来源目标，本轮没有该补丁版本的实测证据。

#### 数组构造语法边界

本块对应源工作流 2 步骤 2 的数组构造分类，只定义 `ARRAY[...]` 的解析结构与类型交接。当前证据基于提交 `59c3c7c0535b3b54a1764401d0fccb244bfbcd8a` 的源码/独立测试阅读，没有行为实测。`expressions.rs::Expr` 有 Value、Raw、Row 等变体，`RowExpr::values` 是行字段列表，该枚举没有专门的数组构造变体；不能把 Raw 文本或 Row 列表当作数组结构保真的证据。未入图的 `parser.y` 已直接核对 `SimpleExpr` / `ArrayKwdOpt`：现有 ARRAY 关键字用于 CAST 等路径，`SetArray` 标志不等于 PG 的方括号构造语法，也不能从这些 Go 语义动作推断 Rust 实际接线或当前拒绝结果。

`pg_extended.rs::Extended::process` 的 Parse 分支经过会话/目录识别、名称及 SQL 适配和 `markers` 后准备语句；普通非目录路径的 OID 白名单没有数组 OID，未知参数类型另有 0A000 门禁。这只说明该参数入口的局部限制，不证明无参数数组 SQL 的接受/拒绝。`markers` 保存重复/乱序参数的映射，不保存原 SQL 诊断跨度。已读 [parser_test.rs](../pkg/parser/parser_test.rs) 的 `test_expression` 注册项和 `run_contract_table_with_options`：它们检查用例表接受性与顶层 AST 类型，不能证明数组嵌套树和类型保真；已读 [pg_extended_test.rs](../pkg/server/pg_extended_test.rs) 的 `parse_bind_execute_sync` 中 current_catalog 的 Parse/Bind/Describe/Execute/Sync 断言，该段不是数组用例。本轮未执行这些测试，亦未证明其他用例是否覆盖数组。`pkg/parser/doc.go`、`pkg/server/doc.go` 均不存在。

承载方案交本章后续解析方案任务比较共享 AST 扩展与受控 PG 入口，不预设新接口。解析层必须保留 ARRAY 构造类别、有序元素表达式树、每层方括号及逗号跨度、空列表、内层 ARRAY 的显式性、显式转换的类型名/数组标记及其作用层级、参数身份与原 SQL 字符位置。不能扁平化嵌套、按文本逗号切割或替换为 ROW、JSON_ARRAY、字符串字面量。语法嵌套不等于最终值维数：解析保留树，矩形形状约束和表达式求值得到的维度交后续语义验收；数组类型、共同元素类型、空数组定型、转换、NULL 与 OID 身份归工作流 3，参数/结果文本和二进制编码、错误字段和恢复归工作流 7。Q 与扩展 Parse 使用同一 PG 语法契约，不以 Parse 成功代表可 Bind 或可返回数组。

下表为待新增 `ARRAY-SYNTAX` 用例族，按第 1 章登记能力 ID/case ID。无表依赖；参数场景另记录参数类型、值及编码。所有行为均未验证，成功目标 SQLSTATE 为“不适用（成功）”、无错误位置。表中数组值用逻辑元素/形状表达，不承诺 wire 文本格式、列名或 OID；结果类型由工作流 3 细化，协议描述及编码由工作流 7 验收。

| 正反例输入 | PG 目标结构与类型边界 | MySQL 适用性、诊断及证据责任 |
| --- | --- | --- |
| 正例 `SELECT ARRAY[1,2]` | 保留两项整数表达式及顺序，目标逻辑数组元素为 1、2；共同元素类型交工作流 3，不能用两列行值替代。 | PG 写法在 MySQL 为隔离负例，应按自身语法拒绝；MySQL SQLSTATE/位置须采集自身参考端和本地基线，不套用 PG 码。 |
| 正例 `SELECT ARRAY[ARRAY[1,2],ARRAY[3,4]]` 与 `SELECT ARRAY[[1,2],[3,4]]` | 保留两种原始写法及相同嵌套分组；目标为 2×2 矩形、元素按上述顺序。不能因省略内层 ARRAY 而丢层级。 | 两种 PG 写法均为 MySQL 隔离负例；诊断仍待取证。矩形形状和值的运行验证另行实施。 |
| 语法正例/定型反例 `SELECT ARRAY[]`；带类型正例 `SELECT ARRAY[]::integer[]` | 空列表须可表示且不等于 NULL、零列或语法残缺；独立无类型上下文的前者不能得到无类型数组成功结果，后者交工作流 3 定型为空整数数组。裸空数组的参考 SQLSTATE、阶段及精确位置待 PG 18 参考端采集，不猜码；解析层不得擅自默认 text[]。 | 原 PG 输入在 MySQL 为负例；自身空值/JSON 语法按自身契约验收，不能要求得到 PG 空整数数组。 |
| 混合元素边界 `SELECT ARRAY[1,2.5]`、`SELECT ARRAY[1,true]` | 均保留元素树和各自类型信息；前者目标共同数值类型，后者无法按共同类型规则统一，交工作流 3 判定并拒绝。精确类型、转换结果及类型失败 SQLSTATE/位置由该工作流补参考端证据，不能一律当 42601。 | MySQL 对原输入按语法拒绝，不能先按 PG 元素共同类型规则求值；当前未采集诊断。 |
| 形状反例 `SELECT ARRAY[[1,2],[3]]` | 保留不等长内层结构，不伪造缺失元素；矩形约束失败由后续语义阶段拒绝，错误阶段、SQLSTATE 与 P 是否出现待参考端取证，不能因方括号闭合就声称执行成功。 | 原输入不是 MySQL 数组构造正例；其自身 JSON 嵌套不等长规则不参与 PG 矩形验收。 |
| 语法反例 `SELECT ARRAY[1,2` | 未闭合方括号，PG 拒绝目标 42601；解析器定位输入末尾的缺失闭合处，精确 P 值待参考端采集，禁止补括号后执行。 | MySQL 可更早拒绝该 PG 构造；具体错误与位置独立采集。工作流 7 传输诊断，不把改写后字节偏移当原 SQL 字符位置。 |
| 参数/词法保护 `SELECT ARRAY[$2,$1,$2]`；`SELECT 'ARRAY[1,2]' /* ARRAY[] */` | 前者保留重复/乱序身份与元素位置，类型及 Bind 另验收；后者仅为普通字符串，不能制造构造节点。前者有充分类型上下文后才是完整成功用例；后者为双方言自身正例，目标原字符串。 | MySQL 不接受前者 PG 构造及 $n 参数契约，自身参数用 ?；交错连接验证不泄露 PG 解析身份。后者成功时无错误位置，双方均未实测。 |
| MySQL 自身正例 `SELECT JSON_ARRAY(1,2)` | 不作为 PG 数组构造正例或等价翻译；该函数在 PG 下的存在性/行为另行取证。 | 按 MySQL JSON 构造规则目标为 JSON 数组，成功时无错误码/位置；其 JSON 类型及编码按自身契约，不比作 PG integer[]。本轮未实测。 |

官方依据（2026-10-23 查阅）：PostgreSQL **18** §4.2.12 [Array Constructors](https://www.postgresql.org/docs/18/sql-expressions.html#SQL-SYNTAX-ARRAY-CONSTRUCTORS) 定义方括号、嵌套、省略内层 ARRAY、矩形约束及空数组显式定型；§10.5 [共同类型判定](https://www.postgresql.org/docs/18/typeconv-union-case.html) 交接工作流 3。[附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html) 区分语法错误 42601 与功能未支持 0A000；合法但暂未支持的形态目标明确以 0A000 拒绝，不能冒充参考 PG 语法非法，本轮未证明当前门禁。§54.8 [错误字段](https://www.postgresql.org/docs/18/protocol-error-fields.html) 规定 P 指向原查询、从 1 起按字符计数。MySQL **8.4** §11.5 [Expressions](https://dev.mysql.com/doc/refman/8.4/en/expressions.html) 与 §14.17.2 [JSON_ARRAY](https://dev.mysql.com/doc/refman/8.4/en/json-creation-functions.html#function_json-array) 仅定义自身语法对照，不证明本地入口已拒绝 PG 构造。18.6 仍仅为来源目标，无该补丁版本实测结论。

#### 行构造语法边界

本块对应源工作流 2 步骤 2 的行构造分类，只定义 `ROW(...)`、多字段括号行和单表达式括号的解析边界。当前静态证据基于提交 `d3995a985309c6393975d051cb90b98be5176bb9`，不是运行通过证据。已核对 [expressions.rs](../pkg/parser/ast/expressions.rs) 的 `RowExpr::values: Vec<Expr>` 与 `Expr::restore` 的 Row 分支：可保存有序字段树，恢复时统一输出 ROW，载体本身没有显式 ROW 与隐式括号的来源标记或逐项跨度。共享载体是候选，不能据其存在推断 PG 解析已接线或匿名行类型已可执行。未入图的 [parser.y](../pkg/parser/parser.y) 的 `SimpleExpr` 分别将 `(Expression)` 建为 ParenthesesExpr，将 `(ExpressionList, Expression)` 和 `ROW(ExpressionList, Expression)` 建为 RowExpr；后两条要求至少两项，且含 Go 语义动作，不证明 Rust 当前接受范围。

已读 [pg_name.rs](../pkg/server/pg_name.rs) 的 `Resolver::close` / `scope` / `relation`：close 配对括号，scope 对扫描遇到的括号递归，并不建立行表达式树；relation 在关系位置只允许以 SELECT/WITH 开头的括号，否则返回 0A000 的关系分组拒绝。这是局部名称适配限制，不能推广到 SELECT 列表中的行构造，更不能把所有括号当子查询或关系。已读 [parser_test.rs](../pkg/parser/parser_test.rs) 的 `test_expression` 注册与 `run_contract_table_with_options`，其接受性及顶层类型对照没有直接断言本块的字段嵌套、原始括号和错误位置；已读 [pg_name_test.rs](../pkg/server/pg_name_test.rs) 的 `pg_introspection_names_token_scopes`（含子查询/CTE、extract 内 FROM 及字面量/注释）和 `pg_introspection_names_rejection_boundaries`（引号、注释及名称拒绝）。这些不是本块新增专项断言，亦未执行；`pkg/parser/doc.go`、`pkg/parser/ast/doc.go`、`pkg/server/doc.go` 不存在。

目标由共享 AST 扩展或受控 PG 解析入口承载，选择交后续解析方案任务取证，不预设新接口。两种方案都须区分行构造、分组表达式、标量子查询和 FROM 关系括号；保留显式 ROW 的出现性、字段数量与顺序、逐字段表达式树、嵌套层级、括号/逗号及原 SQL 字符跨度，不能按逗号文本切分、扁平化或改写成 SELECT 列表。字段顺序不承诺求值顺序；字符串、注释及引号内的 ROW/括号不制造节点。行类型、字段类型、转换及参数定型交工作流 3，行比较、NULL 及运算符语义交工作流 3、4；工作流 7 负责复合值元数据/编码、错误传输与恢复。Q 与扩展 Parse 共用 PG 契约，保留 `$n` 身份；解析接受不能替代 Bind、Describe 或结果编码证据。

下表是待新增 `ROW-SYNTAX` 用例族，按第 1 章登记能力 ID/case ID；除关系例外无表依赖，所有行为均未验证。PG 成功目标 SQLSTATE 为“不适用（成功）”、无错误位置；行值以逻辑字段描述，不预定 wire 文本、列名或 OID。类型可判定是目标分析结论，不表示本地实现已具备该能力。

| 输入与正反例 | PG 语法、类型与执行目标 | MySQL 适用性及诊断责任 |
| --- | --- | --- |
| 正例 `SELECT ROW(1,2)` | 语法合法；保留显式 ROW 和有序两字段；目标可定为匿名 record，字段为两个整数，精确字段类型/OID 交工作流 3。目标执行返回一列复合值，字段依次为 1、2，不能返回两列。 | ROW 多字段也是 MySQL 行语法；不能标为 PG 专属负例。但该 SELECT 投影的合法语法不保证 MySQL 可作为单列复合值返回；适用上下文、标量限制及 SQLSTATE/位置须 MySQL 参考端取证，不承诺同结果。 |
| 正例 `SELECT (1,2)` | 语法合法的隐式两字段行，与前例同逻辑字段和匿名行类型目标；保留隐式形式及跨度，不当子查询。执行目标仍是一列复合值，尚未验证。 | 多字段括号行也是 MySQL 语法，投影执行限制独立验收，同样不能仅因语法接受声称可执行。 |
| 分组正例/单字段行反例 `SELECT (1)`；对比 `SELECT ROW(1)` | 前者语法合法、类型为标量整数、目标值 1，不能建成单字段行；后者合法且为含整数的单字段匿名 record，目标一列复合值。两者类型可区分，执行和协议仍待证。 | `(1)` 是自身分组正例，目标标量 1；MySQL ROW 形式要求至少两项，`ROW(1)` 为语法负例。具体 MySQL 错误码/位置另取证，不套 PG 码。 |
| 语法反例 `SELECT ROW(1,` | 缺少下一字段及闭合括号，拒绝目标 42601；无合法完整树，不进入类型判定或执行，不能自动补齐。定位输入末尾，精确 P 待 PG 18 参考端采集。 | 同样是不完整输入，应拒绝；MySQL 码和位置独立取证。解析层负责原 SQL 定位，PG 协议层传递错误字段。 |
| 嵌套正例 `SELECT ROW(1,ROW(2,3))` | 语法合法；外层两字段，第二字段仍为两字段行，不能扁平化成三字段。匿名行及内层字段类型交工作流 3，目标执行保持嵌套值；复合结果编码须工作流 7 补证。 | 只作为自身行表达式上下文候选；嵌套可执行范围、类型及诊断须取证，不推导与 PG 复合值等价。 |
| 参数正例 `SELECT ROW($2,$1,$2)`（扩展 Parse，显式整数参数类型，绑定 1、2） | 保留参数编号、重复身份及字段位置，目标逻辑字段 2、1、2；语法完整，依给定参数类型定型，Parse/Bind/结果仍须分别验证。无类型上下文的参数不能直接声称可定型。 | MySQL 使用自身 `?` 和行语法上下文，不能继承 `$n` 或 PG record 类型；具体执行用例待 MySQL 基线取证。 |
| 名称交互正例 `SELECT ROW(t.id,(SELECT 2)) FROM public.t AS t`；对比关系分组 `SELECT * FROM (public.t) AS t` | 假定 public.t 有整数 id；前者行字段含标量子查询，外层关系名照常绑定，类型依赖绑定/子查询结果，执行待证。后者属于关系括号，当前名称适配有独立拒绝边界，不能用该拒绝否定表达式 ROW；关系组支持不由本块扩写。 | MySQL 用自身已建 t 和合法限定名验证，public 不自动视为 PG schema；关系组诊断独立取证，不要求双方言名称改写相同。 |
| 词法保护正例 `SELECT 'ROW(1,2)' /* (1,2) */` | 只产生普通字符串，目标原字符串值；没有行节点、无行类型需求，语法/定型/执行目标明确但未验证。 | 自身正例，目标相同字符串；双方言交错连接的行/分组正反例验证身份不泄露，仍须分别记录类型与诊断。 |

官方依据（2026-10-23 查阅）：PostgreSQL **18** §4.2.13 [Row Constructors](https://www.postgresql.org/docs/18/sql-expressions.html#SQL-SYNTAX-ROW-CONSTRUCTORS) 定义显式 ROW 与多字段省略形式及匿名 record；§4.2 的表达式分类及 §4.2.11 区分分组与标量子查询。同页还定义零字段 ROW 和 `rowvalue.*` 形式，它们应由后续任务独立定义支持集合，不在本块四个核心输入之外擅自宣称支持。MySQL **8.4** §11.5 [Expressions](https://dev.mysql.com/doc/refman/8.4/en/expressions.html) 提供分组、ROW 至少两项及子查询的自身语法依据，投影/嵌套的实际诊断仍缺参考端证据。[PG 附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html) 区分 42601 与合法未支持目标拒绝码 0A000；不能把类型/上下文错误一律作为语法错误，本轮未证明当前门禁。§54.8 [错误字段](https://www.postgresql.org/docs/18/protocol-error-fields.html) 要求 P 若提供须基于原查询、从 1 起按字符定位，精确值及是否提供由参考端补证。18.6 仍仅为来源目标，没有该补丁版本或本地数据库实测。

#### 引号与大小写规则

本块对应源工作流 2 步骤 3 的名称词法契约，区分名称拼写、绑定键和后端存储约束。静态证据基于提交 `3bd789433dee85dcd9b2037ee631bdcf13a29429`：已读 [pg_name.rs](../pkg/server/pg_name.rs) 的 `Token` / `tokens` / `source_tokens` / `quote` / `rewrite` / `Resolver::relation`。tokens 调用非 native 模式；source_tokens 将未引号名称转为小写 name，双引号名称保留大小写、解码成对双引号，并保存 quoted 及原输入字节 start/end。rewrite 为双引号名称生成反引号文本，关系位置再替换成当前数据库限定名；未被编辑覆盖的文本直接复制。因此 token 中小写 name 不等于整个 SQL 已按 PG 规则折叠，尤其不能据此证明表达式列名/别名已正确绑定。native=true 仅用于 stored_view_select 的存储源提取，不是允许 PG 输入反引号的开关。

当前扫描跳过普通单引号字符串、行注释及普通块注释，保留未编辑原文；未闭合引号/空名称返回 42601。PG 输入反引号、美元引号、可执行 MySQL 注释及嵌套块注释有 0A000 拒绝边界；其中美元字符串与嵌套注释是合法 PG 形式，不能把当前拒绝解释为 PG 语法非法。单引号扫描沿用 native 反斜杠转义，不足以证明 standard_conforming_strings、E 字符串或全部 PG 词法保护正确。错误返回只含 SQLSTATE 和消息，rewrite 返回 String，未见本块原 SQL 到转换后文本的持久位置映射。

已读 [expressions.rs](../pkg/parser/ast/expressions.rs) 的 `CiString`、`ColumnName::matches` / `restore` 与 `RestoreCtx::write_name`：original/lowercase 双拼写、按小写匹配及反引号还原不能表达完整 PG 引号敏感绑定，也不保留逐名称 quoted 和原 SQL 跨度。已读 [pg_name_test.rs](../pkg/server/pg_name_test.rs) 的 `pg_introspection_names_token_scopes`（双引号、`"a""b"`、字符串/注释）和 `pg_introspection_names_rejection_boundaries`（空/未闭合引号、`"PUBLIC"` 等拒绝）；[pg_query_test.rs](../pkg/server/pg_query_test.rs) 的 `postgres_create_table_simple_query` 使用双引号混合大小写表名，但按 schema.name.lower / table.name.lower 查找，未断言大小写不同对象共存或误绑定拒绝。这些仅为静态测试意图，本轮未执行。`pkg/server/doc.go`、`pkg/parser/doc.go`、`pkg/parser/ast/doc.go` 均不存在。

目标在 PG 词法阶段区分标识符、关键字、字符串和注释，保存原拼写、是否引号、解码名称、规范绑定键和原 SQL 跨度；未引号按 PG 规则折叠，双引号保真且 `""` 解码为一个名称字符。resolver 对表、列和别名使用该语义身份，不能统一用 lowercase 或依靠反引号转换制造大小写敏感。共享 AST 扩展与受控 PG 入口的选择交后续解析方案块取证，不预设新接口。若后端存储/目录无法区分 foo 与 Foo，应显式登记能力缺口、阻止静默合并/误读；命名映射或存储改造另行细分，本块不指定方案。限定名、CTE 遮蔽及 search_path 在后续块定义；目录可见性归工作流 6，会话设置归工作流 5。

以下为待新增 `NAME-QUOTING` 用例族，所有运行行为均未验证。成功目标 SQLSTATE 为“不适用（成功）”、无错误位置；类型和元数据按第 1 章分别采集，不把拼写一致当运行通过。MySQL 基线为 8.4，明确记录 sql_mode、lower_case_table_names、对象类别及平台；双引号默认字符串模式与 ANSI_QUOTES 分开验收。

| 输入与正反例/前提 | PG 目标及诊断责任 | MySQL 自身目标及诊断责任 |
| --- | --- | --- |
| 正例：预建 public.foo，列 id=1；分别 `SELECT id FROM foo`、`SELECT id FROM Foo`、`SELECT id FROM "foo"` | 三者均绑定 foo，目标值 1；不将 Foo 作为独立对象。名称折叠发生于词法/绑定交接，非字符串全文转小写。 | 未引号 foo/Foo 的表绑定取决于 lower_case_table_names 与平台，不预定一致；双引号表名在默认模式是负例，ANSI_QUOTES 下按自身表名规则处理，不自动获得 PG 大小写敏感。MySQL 错误码与位置须参考端采集。 |
| 保真正例/误绑定反例：同一 schema 预建 foo(id=1) 和 "Foo"(id=2)；`SELECT id FROM "Foo"` | 目标只返回 2；foo/Foo/"foo" 仍返回 1。再单独以仅有 foo 的夹具查询 "Foo"，应为未定义关系 42P01，不能降级绑定 foo。是否给出 P 及精确位置待 PG 18 参考端取证。 | 不要求其可创建这两个表；必须先按自身配置确认可区分性。以单个表的列名 foo/Foo 做自身不区分大小写正例；不将后端原有规则改成 PG 规则。 |
| 转义正例：预建 `public."a""b"`，id=3；`SELECT id FROM public."a""b"` | 解码名称为 a 后接一个双引号再接 b，目标 3；原拼写与解码值分别保存，不把引号当字符串或分隔两名称。 | 反引号包围 a、双引号、b 为自身标识符正例；ANSI_QUOTES 下同一双引号输入也是名称候选，默认模式该 FROM 输入是负例。结果/诊断独立取证，不把 SQL 文本改写结果当执行证据。 |
| 词法反例：`SELECT * FROM "Foo`；空名称 `SELECT * FROM ""` | 均拒绝目标 42601，不补齐引号、不进入对象查找。解析层保留错误起始引号及输入末尾跨度，P 的准确选择/数值待参考端取证，工作流 7 只传递诊断。 | 同一 FROM 输入在默认模式及 ANSI_QUOTES 下均须拒绝；错误码、阶段、位置独立采集，不套 PG 42601 或 P。自身未闭合反引号名称另作负例。 |
| 方言隔离：反引号包围的 foo 作 FROM 表名；另对比 `SELECT "Foo"` | 反引号输入为 PG 语法负例，目标 42601；当前适配返回 0A000 是已知诊断差异。后者是名称引用，在无 FROM 且无该列时目标 42703，不能返回字符串。 | 反引号表名是自身正例；默认模式 SELECT 双引号 Foo 返回字符串 Foo，ANSI_QUOTES 下是列引用并按自身缺列规则拒绝。隔离回归交错 PG/MySQL 连接，禁止 PG 改写或绑定键泄露。 |
| 保护正例：`SELECT 'Foo "a""b"' /* "Foo" */`，另加行注释中同样名称 | 字符串值保持原大小写及双引号，注释不制造名称/关系节点；目标只返回该字符串。不同 PG 字符串模式与合法嵌套注释须另取证，不据此推广保护集合。 | 普通单引号和普通注释是自身正例，目标相同文本；MySQL 特有可执行注释另按自身规则，不被 PG 扫描门禁改写。 |

位置责任为：解析/适配保留原查询和每次替换区间，后续绑定/解析诊断若基于转换后文本，必须先映射回原 SQL；字符计数与 UTF-8 字节偏移分别处理。PG 的 P 若提供须按原输入从 1 起按字符计数，不能直接使用 Token.start/end 或反引号文本偏移。待新增用例需包含 `"a""b"` 长度变化、public 到当前数据库限定名变化，以及错误前有中文的输入；精确 P、MySQL 自身位置和简单 Q/扩展 Parse 一致性均缺运行证据。合法但尚未支持的形态按明确能力门禁拒绝，不把词法错误、绑定缺失与 0A000 混为同类。

官方依据（2026-10-23 查阅）：PostgreSQL **18** §4.1.1 [标识符](https://www.postgresql.org/docs/18/sql-syntax-lexical.html#SQL-SYNTAX-IDENTIFIERS)、§4.1.2 与 §4.1.5 定义名称、字符串、注释边界；[附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html) 提供 42601、42P01、42703 和 0A000 分类；§54.8 [错误字段](https://www.postgresql.org/docs/18/protocol-error-fields.html) 定义 P。MySQL **8.4** §11.2 [对象名称](https://dev.mysql.com/doc/refman/8.4/en/identifiers.html) 和 §11.2.3 [名称大小写](https://dev.mysql.com/doc/refman/8.4/en/identifier-case-sensitivity.html) 仅为自身目标依据。U& 引号、编码、名称长度/截断与非 ASCII 折叠的完整契约另需专项官方取证及参考端验证，不由 Rust to_lowercase 推导。18.6 仅沿用来源目标，没有该补丁版本或本地数据库实测结论。

#### 限定名称解析规则

本块对应源工作流 2 步骤 3 的关系名与列/别名限定责任，承接[引号与大小写规则](#引号与大小写规则)。静态证据基于提交 `c05789991a5462588cd20c8c9dedace2f233014f`：已核对 [pg_name.rs](../pkg/server/pg_name.rs) 的 `Resolver::relation` / `scope`、`rewrite`、`adapt` 与 `validate_relations`。relation 按点号收集关系分段：单段非 CTE 名在 public 不在路径时返回 42P01；public.t 与当前库 db.public.t 可进入改写，其他 schema、其他库及更多段返回 0A000；无当前数据库时返回 3D000。限定 public 不受 has_public 布尔值限制。adapt 先以空数据库探测，再按需读取 SELECT DATABASE() 并重写：首遍允许三段库名只是探测，不能脱离第二遍当前库检查解释为允许跨库。最终将关系跨度替换为后端当前库和表名，不表示 public 已成为完整 PG schema。

validate_relations 对成功解析的共享 AST 遍历 TableName，拒绝非空且与当前库 lowercase 不同的 Schema（0A000）；解析失败时返回 Ok，后续命令门禁仍须拒绝错误，不能把此 guard 当完整语法/权限检查或保证所有节点可见。指定 [ast/dml.rs](../pkg/parser/ast/dml.rs) 的 `TableName::new` / `restore` 仅有 schema/name 两级及 MySQL 引用还原；实际 guard 使用 [ast/lib.rs](../pkg/parser/ast/lib.rs) 的 `TableName`（Schema、Name），均不能单独承载 PG 数据库/schema/关系三层、逐段引号与原跨度。scope 只在关系位置调用 relation，a.id 不因此成为 schema.table；列与别名绑定仍需共享解析和绑定层的专项证据，不能由改写成功推定。

已读 [pg_name_test.rs](../pkg/server/pg_name_test.rs) 的 `pg_introspection_names_token_scopes`：t、public.u、db.public.v 的混合 FROM/JOIN、双引号 a.id 及 DML 关系改写；`pg_introspection_names_rejection_boundaries`：other.public.t、other.t、db.private.t、public.t.extra、空当前库和路径无 public 的拒绝断言。二者只调用 rewrite，不覆盖 adapt 的数据库读取或 AST guard。已读 [pg_query_test.rs](../pkg/server/pg_query_test.rs) 的 `postgres_create_table_simple_query`，public 的混合大小写建表落到 test 库是局部意图，不是跨库拒绝、别名或完整 schema 的证明；本轮均未运行。目标包 `pkg/server/doc.go`、`pkg/parser/doc.go`、`pkg/parser/ast/doc.go` 不存在。

目标按语法上下文区分关系限定与表达式限定，保留有序分段、每段 quoted/解码名称及原 SQL 跨度。关系一段交给当前数据库内的名称搜索，两段为显式 schema/关系，三段为数据库/schema/关系且数据库必须匹配连接数据库；不得删掉 foreign 前缀后落到同名本地表，也不得用后端 database 名直接接受为 PG schema。resolver 在执行前完成隔离、对象绑定及权限交接；即便跨库用户有后端权限也不能放开 PG 跨库访问。非 public schema 的目标绑定依赖工作流 6 的目录模型，尚未开放时以明确能力门禁 0A000 拒绝，不静默回退 public。CTE 遮蔽由下一块定义；search_path 的完整搜索、状态执行分别由后续名称块和工作流 5 定义，本块只固定查找责任，不展开实现。

下列为待新增 `NAME-QUALIFIED` 用例族，均无当前运行证据。PG 夹具为连接数据库 db、public.t(id=1)、路径包含 public、具备必要权限且无同名 CTE；other 数据库中的同名表取 id=2 以检测误读。成功目标 SQLSTATE 不适用（成功）、无错误位置；结果列类型/元数据按第 1 章另行采集。MySQL 8.4 使用独立夹具和自身权限，默认数据库 db 的 t(id=1)、数据库 public 的 t(id=2)，不能把 PG 的 public 自动映射过去。

| 输入/正反例 | 当前静态边界 | PG 目标与诊断责任 | MySQL 自身目标与诊断责任 |
| --- | --- | --- | --- |
| 正例 `SELECT id FROM t` | public 可搜索且当前库非空时改写到后端 db.t；无 public 时 rewrite 返回 42P01。 | 给定夹具返回 1；未限定名由 resolver 搜索，不硬编码总是 public；路径不含可见 t 时目标 42P01，完整路径规则另块定义。 | 绑定默认库 db.t，目标 1；不读取 PG search_path。 |
| 正例 `SELECT id FROM public.t` | 显式 public 可在 public=false 时改写 db.t；数据库为空返回 3D000。 | 绑定当前库显式 public.t，目标 1，不受 public 是否列入路径影响；对象不存在目标 42P01。 | public 是数据库限定，按独立夹具返回 2；无该库/表或无权限时独立采集自身诊断，不套 PG schema 语义。 |
| 正例 `SELECT id FROM db.public.t` | db 匹配当前库时改写 db.t；不是三个后端名称层次。 | db 必须是连接数据库，目标同 public.t 返回 1。数据库身份在绑定前核对，不能依文本删前缀通过。 | 三段 FROM 关系名是自身语法负例；MySQL 的三段列名 db.t.id 是另一上下文。错误码/位置待参考端采集。 |
| 跨库反例 `SELECT id FROM other.public.t`；另对比 `SELECT id FROM other.t` | 在当前库 db 下均拒绝 0A000。 | 前者拒绝，目标 0A000，不读 other 或本地同名表；后者是当前库 schema other 的关系，不是跨库语法：无该关系目标 42P01，合法但未开放的 schema 按能力门禁 0A000，不伪称 PG 禁止该 schema。 | 前者是三段关系名语法负例；other.t 可作为自身跨库正例，预建且授权时读取 other 库 t，结果与权限独立验收，不受 PG 门禁影响。 |
| 列/别名正例 `SELECT a.id FROM public.t AS a`；反例 `SELECT t.id FROM public.t AS a`；缺列 `SELECT a.missing FROM public.t AS a` | relation 只改 FROM 表；token_scopes 有别名保留意图，无列绑定断言。 | 正例返回 1；显式别名后原表名不再可引用，第二条目标 42P01；第三条目标 42703。binder 按查询作用域处理别名与列，不把 a 当 schema 去跨库查表。 | 用 `FROM db.t AS a` 作自身正例返回 1；原名/缺列负例按自身规则拒绝，诊断独立采集，不套 PG SQLSTATE。 |
| 形态反例 `SELECT * FROM public.`；多段负例 `SELECT * FROM db.public.t.extra` | 尾点缺名称返回 42601；多段关系返回 0A000。 | 不丢空段或截断多余段；前者目标 42601，后者精确 PG 诊断须参考端取证，不能由适配层 0A000 推定。 | 分别按自身关系语法拒绝；多段列引用与关系引用分开，精确错误码/位置待参考端取证。 |

错误责任：parser 负责分段合法性及上下文，resolver/binder 负责数据库隔离、可见关系/别名/列与权限，工作流 7 传递原诊断。所有拒绝必须在读写目标关系前完成，不能借转换后 AST 绕过检查。P 若提供须映射回原 SQL、从 1 起按字符计数；限定名长度变化、逐段引号和错误前中文均纳入待新增位置断言，不能使用改写后的字节偏移。上述目标 SQLSTATE 的精确触发、P 是否提供及数值、错误优先级、简单 Q/扩展 Parse 一致性和双方言交错连接隔离均待运行取证；无当前数据库的 3D000 是现适配边界，连接建立/切库契约由工作流 5、7 另证，不伪造 PG 同场景对照。

官方依据（2026-10-23 查阅）：PostgreSQL **18** §5.10.1 [Schemas](https://www.postgresql.org/docs/18/ddl-schemas.html#DDL-SCHEMAS-CREATE) 定义 schema.table 与只允许当前数据库的 database.schema.table；§5.10.3 将未限定关系交给搜索路径。§7.2.1.2 [表和列别名](https://www.postgresql.org/docs/18/queries-table-expressions.html#QUERIES-TABLE-ALIASES) 定义别名替代原关系名；[附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html) 给出 0A000、42601、42P01、42703 分类，具体错误位置仍需参考端补证。§54.8 [错误字段](https://www.postgresql.org/docs/18/protocol-error-fields.html) 定义 P；MySQL **8.4** §11.2.2 [Identifier Qualifiers](https://dev.mysql.com/doc/refman/8.4/en/identifier-qualifiers.html) 区分关系的 database.table 与列的 database.table.column。18.6 仅为来源目标，本轮没有该补丁版本或本地数据库实测。

#### 公共表表达式遮蔽规则

本块对应源工作流 2 步骤 3 的 CTE 名称责任，承接[引号与大小写规则](#引号与大小写规则)及[限定名称解析规则](#限定名称解析规则)。静态取证基于提交 `b5ae432d5a2c6682d13b9fe2b3813bd72c766482`：已读 [pg_name.rs](../pkg/server/pg_name.rs) 的 `Resolver::scope` / `relation`、`rewrite` / `adapt`。scope 克隆继承的名称集合，在扫描每个定义体后才插入该 CTE 名，再向后续定义和主查询传递；括号递归使用克隆集合，内层新增名称不会泄漏出去。relation 只在单段名命中集合时保留引用；显式 public.t 仍改写到当前后端库的表，即使 public 不在路径中。WITH 后紧接 RECURSIVE 一律返回 0A000，包括没有实际自引用的输入；这只是当前入口门禁，不能称为 PG 的递归规则。

已读独立测试 [pg_name_test.rs](../pkg/server/pg_name_test.rs) 的 `pg_introspection_names_token_scopes`：覆盖 `WITH t AS (SELECT 1) SELECT * FROM t` 在 public=false 时不改写、同名 CTE/public.t 区分及内层 CTE 不泄漏到外层；`pg_introspection_names_rejection_boundaries` 覆盖 RECURSIVE 的 0A000。未执行这些测试；现有断言没有证明嵌套同名的绑定对象、前向引用、自引用、重复定义或错误位置。已读 [ast/dml.rs](../pkg/parser/ast/dml.rs) 的基础 `TableName`，CTE 实际共享载体另定位到 [ast/lib.rs](../pkg/parser/ast/lib.rs) 的 `WithClause`（IsRecursive、顺序 CTEs）、`CommonTableExpression`（Name、ColNameList、Query、IsRecursive）及 `WithClauseRef`；这些字段存在不等于 PG 绑定接线完整。已读 [parser_test.rs](../pkg/parser/parser_test.rs) 的 `test_dml_stmt` / `test_set_operator` 注册和 `run_contract_table_with_options`：接受性及顶层节点类型检查不能替代 CTE 作用域断言。`pkg/server/doc.go`、`pkg/parser/doc.go`、`pkg/parser/ast/doc.go` 不存在。

目标由 parser 保留定义顺序、查询块嵌套、递归标记、列名列表及名称的 quoted/规范身份和原 SQL 跨度；resolver/binder 拥有每个查询块的 CTE 名称空间与具体绑定。普通 WITH 的定义体只见先前同级定义及合法外层名称，主查询见本层全部定义；内层同名定义在其可见区域遮蔽外层 CTE，退出内层后恢复外层，不能用无绑定身份的字符串集合证明实现完成。未限定关系先查可见 CTE，再交关系搜索；显式 schema.t 绕过 CTE，按真实关系绑定。非递归定义自身尚未登记时，同名引用仍可绑定合法外层 CTE 或真实表，不能仅凭拼写相同认定递归或无条件拒绝。会话后续语句不保留 CTE 名称，CTE 不成为目录对象；search_path 状态执行及目录可见性分别交工作流 5、6。

PG 的 RECURSIVE 还允许无环前向引用，不等于每个 CTE 都自递归；自引用是否合法由递归项位置等结构约束决定，互递归不属于 PG 已实现能力。本块只登记解析/绑定入口责任，不开放递归执行；能力未开放时保持明确 0A000 门禁，并将参考 PG 的合法结果与 AsterSQL 暂时拒绝分别记录。物化、工作表迭代、优化、SEARCH/CYCLE 与数据修改型 CTE 的执行另行细分，不以本块补建完整 WITH RECURSIVE。

以下为待新增 `NAME-CTE` 用例族，全部运行行为未验证。PG 夹具为当前库 db、可见 public.t(v=9)、不存在关系 a/b/r，普通表搜索路径含 public；MySQL 8.4 独立夹具为默认库 db 的 t(v=9)，显式限定对照使用 db.t，PG 的 public.t 不直接充当 MySQL schema 语义。成功项 SQLSTATE 不适用（成功）、无错误位置；类型、列名及协议元数据按第 1 章另采集。

| 输入/正反例 | PG 目标与当前边界 | MySQL 自身目标 | 诊断责任与证据缺口 |
| --- | --- | --- | --- |
| 基本正例 `WITH t AS (SELECT 1) SELECT * FROM t` | 目标单行 1，遮蔽 public.t 的 9；现有静态测试仅断言 rewrite 保留，public=false 也应命中 CTE。 | 同一 SQL 目标 1，遮蔽默认库真实表。 | binder 绑定 CTE，不能先因搜索路径缺表拒绝；无执行/元数据证据。 |
| 嵌套同名正例 `WITH t AS (SELECT 1 AS v) SELECT t.v, q.v FROM t CROSS JOIN (WITH t AS (SELECT 2 AS v) SELECT v FROM t) AS q` | 目标一行 (1,2)，内层绑定内层 t，外层仍为外层 t。当前集合扫描保留同名引用，不能辨别绑定对象。 | 同一 SQL 目标 (1,2)，分别验证查询块的同名解析。 | binder 记录层级与对象身份；静态改写相同不证明遮蔽正确。 |
| 显式表正例 `WITH t AS (SELECT 1 AS v) SELECT v FROM public.t` | 目标 9；即使移除路径中的 public 仍指显式表，当前 relation 将它改写到 db.t。 | 使用 `WITH t AS (SELECT 1 AS v) SELECT v FROM db.t`，目标 9；db 是数据库限定。 | resolver 绕过 CTE，目录/权限交工作流 6；不存在真实表时 PG 目标 42P01，MySQL 自身诊断另采集。 |
| 顺序正例 `WITH a AS (SELECT 1 AS v), b AS (SELECT v FROM a) SELECT v FROM b`；前向反例 `WITH a AS (SELECT v FROM b), b AS (SELECT 2 AS v) SELECT v FROM a` | 前者目标 1；后者在无真实 b 时拒绝，目标 42P01，不提前注册未来 b。当前扫描后插入次序吻合局部意图，但不会自行查目录；有真实同名 b 时应另验真实表绑定，不能一概拒绝。 | 前者目标 1；后者同样不可引用后定义的 b，无真实 b 时拒绝，精确码/位置待采集。 | binder 区分未来 CTE 与可见真实表；错误位置应关联定义体中的 b，P 是否提供及数值待证。 |
| 递归关键字边界 `WITH RECURSIVE t AS (SELECT 1) SELECT * FROM t`；无环前向 `WITH RECURSIVE a AS (SELECT v FROM b), b AS (SELECT 2 AS v) SELECT v FROM a` | PG 合法，目标分别为 1、2；当前适配器两者均在 RECURSIVE 处拒绝 0A000。 | 第一条目标 1；第二条仍不能前向引用，无真实 b 时拒绝；不能复制 PG 放宽规则。 | parser/binder 区分关键字、依赖与实际递归；当前门禁与参考成功分别验收，MySQL 精确诊断待取证。 |
| 自引用边界 `WITH RECURSIVE r(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM r WHERE n<2) SELECT n FROM r ORDER BY n`；去掉 RECURSIVE 的反例 | 合法形式参考目标两行 1、2，当前入口拒绝 0A000；去掉关键字且无外层/真实 r 时目标 42P01，不自动升级递归。 | 合法形式目标 1、2；去掉关键字且无真实 r 时拒绝，官方示例给出 1146/42S02，仍需此输入实测。 | binder 负责自引用可见性，执行另交工作流 5；不据扫描通过宣称递归可用。 |
| 非法递归反例 `WITH RECURSIVE r(n) AS (SELECT n FROM r UNION ALL SELECT 1) SELECT * FROM r`；同层重复 `WITH t AS (SELECT 1), t AS (SELECT 2) SELECT * FROM t` | 第一条自引用位于非递归项，应拒绝，精确码待参考端取证；第二条应拒绝重复定义，目标 42712。当前分别为 RECURSIVE 门禁与集合去重，后者未显式检查重复。 | 两条分别按自身递归结构与名称唯一性规则拒绝；精确码/位置待采集。 | binder 负责结构及重复名诊断；不能让 HashSet 去重隐藏错误，也不能用 0A000 代替 PG 本来无效输入的真实诊断。 |
| 名称不泄漏反例：先执行基本正例，再单独执行 `SELECT * FROM t`；另将内层 WITH 放入派生表后在外层引用 t | 下一语句/外层均不得使用已结束的 CTE；给定真实表夹具应返回 9，而非 1，删除真实 t 的独立夹具下目标 42P01。现有测试只覆盖内层新增名不泄漏的文本。 | 独立连接/夹具同样绑定真实表 9；无表时按自身诊断拒绝。 | resolver 负责查询块生命周期；跨语句、简单 Q/扩展 Parse 及双方言交错连接均待新增实际断言。 |

诊断按 parser 的形态/跨度、binder 的可见性/定义顺序/重复与递归结构、工作流 7 的错误传递分责。P 如提供，须指向原 SQL，从 1 起按字符计数；定义体引用、第二个重复名及递归项的定位、引号转换和错误前中文均须独立取证，不能用改写后字节偏移。未列精确码的目标需先查官方对应源码或真实参考端，再补矩阵；当前拒绝不是未来功能成功，所有适用断言仍按第 1 章保留“未验证”。

官方依据（2026-10-23 查阅）：PostgreSQL **18** [SELECT 的 WITH/FROM](https://www.postgresql.org/docs/18/sql-select.html#SQL-WITH) 定义普通/递归 WITH 的同级引用顺序、真实表被遮蔽及显式 schema 绕过；§7.8 [WITH Queries](https://www.postgresql.org/docs/18/queries-with.html) 定义单查询作用域及递归项结构。嵌套同名按这些作用域规则制定上述目标，仍需参考端验证具体输入；[18 分支 parse_cte.c](https://github.com/postgres/postgres/blob/REL_18_STABLE/src/backend/parser/parse_cte.c) 是重复名与递归诊断的后续取证入口，分支可变，不作固定补丁实测证据。[附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html) 给出 42P01/42712/0A000 分类，§54.8 [错误字段](https://www.postgresql.org/docs/18/protocol-error-fields.html) 定义 P。MySQL **8.4** §15.2.20 [WITH](https://dev.mysql.com/doc/refman/8.4/en/with.html) 定义查询块搜索、同级仅向前序引用及自递归关键字要求。18.6 仍仅为来源指定目标，没有本轮补丁版本、AsterSQL、PG 或 MySQL 的实测结果。

#### 模式搜索路径解析契约

本块对应源工作流 2 步骤 3 的 schema/search_path 责任，承接[引号与大小写规则](#引号与大小写规则)、[限定名称解析规则](#限定名称解析规则)及[公共表表达式遮蔽规则](#公共表表达式遮蔽规则)。静态证据基于提交 `d0e11420ba7c3e452418375ce494b5769bae811f`：已读 [pg_session.rs](../pkg/server/pg_session.rs) 的 `PgSession::has_public` / `schema` / `public_precedes_catalog` / `execute` 与 `SessionQuery::parse`。当前默认路径仅 public；has_public 只判断成员，schema 只取原始列表首项，public_precedes_catalog 仅在 public 与 pg_catalog 都显式出现且 public 在前时为真。parse 只接受这两个名称、无重复列表及空字符串路径，其他 schema（包括 missing）、重复项返回 0A000；这不是 PostgreSQL 完整路径模型，也没有证明首项是存在且有 USAGE 的有效 schema。

[pg_name.rs](../pkg/server/pg_name.rs) 的 `Resolver::relation` / `adapt` 将未限定非 CTE 关系交给 has_public 门禁；public 缺席返回 42P01，显式 public.t 绕过该门禁并改写到当前后端库，非 public/跨库拒绝沿前块契约。[pg_catalog.rs](../pkg/server/pg_catalog.rs) 的 `CatalogQuery::parse_session_with_types` 在 public_precedes_catalog 为真时，通过当前库快照检查隐式目录关系是否被同名用户表遮蔽，再交给有界目录解析；`current_schema` 字段取自 session.schema()。目录行生成中 pg_namespace 仅映射当前库为 public，并提供 pg_catalog，不因路径排除而删除目录行。上述接线不是任意 schema 的 resolver，也不能由路径成员推导对象存在或用户可见。

独立测试只阅读、未执行：[pg_session_test.rs](../pkg/server/pg_session_test.rs) 的 `pg_introspection_namespace_parser_boundaries` 覆盖有界 SET/RESET/SHOW 接受与缺失 schema、重复名拒绝；[pg_query_test.rs](../pkg/server/pg_query_test.rs) 的 `pg_introspection_namespace_session_state` 有两连接不同路径、空路径 current_schema 为 NULL、隐式 catalog 可查询、RESET 和 Parse 后 SET 的 current_schema 断言；`pg_introspection_names_live_tables` 有路径仅 pg_catalog 时未限定用户表 42P01、显式 public 表仍读取的断言。未据此声称同名 catalog 完整优先级、权限过滤或缓存重绑定已通过。`pkg/server/doc.go` 不存在，本块源码和测试阅读均不替代当前运行证据。

目标由工作流 5 拥有连接私有的路径设置、SHOW、RESET、SET LOCAL 及事务恢复；本块只定义 resolver 消费状态的责任，不指定新类型或方法。交接信息须能区分原始设置与有效有序 schema、连接数据库/角色、名称身份与对象类别，并关联工作流 6 提供的真实对象、schema 存在性及权限可见性；不能用 has_public 或首项字符串取代目录查询。未限定关系先按前块查可见 CTE，再按有效关系搜索路径查首个匹配对象；显式 schema.t 绕过 CTE 和路径成员筛选，仍须执行目录存在性、权限及当前数据库隔离检查，失败不能静默回落 public。

PG 目标规则：省略 pg_catalog 时隐式优先于显式路径；显式列出则遵从所在位置。空显式路径不禁用隐式 catalog，普通 public 表不能因此可见；存在临时 schema 时，关系/类型还须按 PG 的临时 schema 优先级处理，不能将关系顺序直接用于函数/运算符。不存在或无 USAGE 的路径项按官方规则忽略，特殊项 "$user" 按当前用户解析；这些完整规则是工作流 5、6 的后续依赖，当前入口不支持任意 schema、用户路径或临时 schema 的部分须显式登记门禁 0A000，不能误称 PG 要求拒绝。对象创建默认 schema、函数/运算符重载及临时对象实现不在本块展开。current_schema 的目标是首个有效显式 schema（没有则 SQL NULL），不能因隐式 pg_catalog 总可搜索便返回 pg_catalog；SHOW 原值、有效路径及目录可见对象分别验收。

以下为待新增验收入口，所有运行行为均“未验证”。夹具：同一 PG 数据库仅有 public.t(id) 一行 1，未建临时对象，角色具有所需 USAGE/SELECT；目录同名场景另建 public.pg_namespace(nspname) 一行 'user_marker'，参考端 pg_catalog.pg_namespace 含 pg_catalog 行。MySQL 独立默认库 db 中准备 t(id)=1，仅按自身名称规则对照，不使用 PG 设置。

| 场景与正反例 | PG 目标结果与当前边界 | MySQL 适用性与目标 | SQLSTATE、定位及交接 |
| --- | --- | --- | --- |
| public 在路径：`SET search_path TO public` 后 `SELECT id FROM t`；反例改为不存在的 missing_t | 返回 1；隐式 catalog 仍先搜索。missing_t 目标 42P01，不能以路径含 public 作为存在证明。 | `SELECT id FROM t` 在 db 返回 1；PG SET 不适用。缺表按自身诊断取证。 | 成功不适用；缺表由 resolver 诊断原关系名，工作流 6 负责对象存在性。 |
| public 不在路径：`SET search_path TO pg_catalog` 后分别 `SELECT id FROM t`、`SELECT id FROM public.t` | 前者目标 42P01；后者仍返回 1。当前有对应有界测试意图，显式名不受路径成员限制不等于免权限检查。 | t 仍绑定默认库；对应显式写法为 db.t。public.t 在 MySQL 是数据库限定，不能直接当 PG schema 对照。 | resolver 对未找到的 t 定位，显式名按前块权限/目录交接；不得转而读取其他数据库。 |
| catalog 同名与顺序：分别设置 public、pg_catalog, public、public, pg_catalog，执行 `SELECT nspname FROM pg_namespace WHERE nspname = 'pg_catalog'`；再显式查 pg_catalog.pg_namespace 与 public.pg_namespace | 前两种隐式查询返回 pg_catalog 行；第三种绑定用户表返回零行。显式 catalog 查询总返回该行；显式 public 查询在此谓词下零行。顺序控制的是对象绑定，不能靠 UNION 两个对象模拟。 | PG 路径和目录 SQL 不适用；旁路 `SELECT id FROM db.t` 始终返回 1，不受 PG 连接设置影响。 | 各成功项 SQLSTATE/位置不适用；同名夹具、绑定身份、列元数据及隐式/显式一致性由工作流 6 协作验收，当前未验证。 |
| 空路径：`SET search_path TO ''` 后查询 t、public.t、pg_catalog.pg_namespace，以及 `SELECT current_schema()` | t 目标 42P01；public.t 返回 1；显式 catalog 可查，隐式 pg_namespace 也仍绑定 catalog；current_schema 目标 SQL NULL。空路径不等于禁用所有对象。 | PG SET/current_schema/catalog 不适用；旁路查询 db.t 返回 1。 | 缺表归 resolver；空路径设置和内省值归工作流 5，目录对象归工作流 6；其余成功无错误位置。 |
| 两 PG 会话 A=public、B=pg_catalog，交错查询 t/public.t，随后仅 A 改为空路径；MySQL M 同时查 db.t | 初始 A 的 t 返回 1，B 的 t 为 42P01，两者 public.t 都返回 1；A 改路径后仅 A 的未限定查找改变，B 状态保持。简单 Q 与扩展查询分别采证。 | M 始终返回 1；不向 M 注入 PG SET 语句并忽略错误。 | 工作流 5 拥有状态隔离，resolver 消费所属连接的有效状态；不能从 current_schema 的两连接测试推广为全链路绑定隔离。 |
| 能力边界反例：`SET search_path TO missing, public`、`SET search_path TO "$user", public` | PG 参考端按存在性/USAGE 展开或忽略相应项；当前入口返回 0A000，作为有界能力差异登记，不作为 PG 语法错误或未来成功证据。 | PG 路径语句不适用，MySQL 自身默认库逻辑独立。 | 本轮不猜造任意角色/权限组合结果；需工作流 5、6 的夹具和参考端证据后细分开放条件。 |

诊断责任分开：路径设置语法与拒绝归工作流 5，名称无匹配的 42P01 归 resolver，目录/权限诊断归工作流 6，错误传输归工作流 7。0A000 为当前有界能力门禁，不能替代 PG 合法输入的完整目标。P 如提供须指向原 SQL，从 1 起按字符计数；双引号、错误前中文和改写跨度均待独立取证，不从后端改写文本计算。Parse/Bind/Execute 间改变路径时，既有 current_schema 断言不能决定关系绑定时点、重新分析及缓存失效规则；须后续与工作流 5 单独取证再定义，不承诺所有已准备语句固定或立即重绑定。类型、OID、NULL 元数据、精确诊断位置及参考端结果均须按第 1 章补证。

官方依据（2026-10-23 查阅）：PostgreSQL **18** §5.10.3/5.10.5 [Schemas](https://www.postgresql.org/docs/18/ddl-schemas.html) 说明路径和系统目录；§19.11.1 [search_path](https://www.postgresql.org/docs/18/runtime-config-client.html#GUC-SEARCH-PATH) 给出隐式/显式 catalog、有效路径、临时 schema 及用户项规则；§9.27.1 [Session Information Functions](https://www.postgresql.org/docs/18/functions-info.html#FUNCTIONS-INFO-SESSION) 定义 current_schema/current_schemas。[附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html) 给出 42P01/0A000 分类，§54.8 [错误字段](https://www.postgresql.org/docs/18/protocol-error-fields.html) 定义 P。MySQL **8.4** §11.2.2 [Identifier Qualifiers](https://dev.mysql.com/doc/refman/8.4/en/identifier-qualifiers.html) 支持上述默认库/数据库限定对照。18.6 仅为来源指定目标，本轮未运行该补丁版本、AsterSQL、PG 或 MySQL，不将大版本规则当作补丁实测。

#### 解析承载方案决策

本块对应源工作流 2 步骤 4，汇总此前六类语法和名称规则，决策为：**先做最小语法原型，再选择最终接入方式**。目前只确定共享规范执行内核、连接私有方言身份、原 SQL 保真及显式能力门禁；不认定共享 AST 扩展或独立受控 PG parser 已胜出。候选 B 的“独立”仅指词法/语法入口和必要的语法表示，不包含第二套 resolver、优化器、执行器或存储。现有文本适配保留为有界过渡，不作为第三条通用翻译路线。

证据基于提交 `a2d37b4477ba9feb4492d32c780a38b2560f64ff`，本轮仅阅读源码和独立测试。已核对 [yy_parser.rs](../pkg/parser/yy_parser.rs) 的 `Parser::ParseSQL` / `ParseOneStmt` / `yySetOffset`：Scanner/goyacc 决定接受和诊断，入口返回共享 Node，单语句入口检查数量，偏移写入不等于完整字符跨度映射。[parser.y](../pkg/parser/parser.y) 有 Go 语义动作的 `IlikeOrNotOp`、`ReturningClause`、`OnDuplicateKeyUpdate` 及 RowExpr 分支，不能作为 Rust PG 接线证明。[expressions.rs](../pkg/parser/ast/expressions.rs) 的 `PatternLikeOrIlikeExpr` 以 u8 保存 escape，`RowExpr` 只列 values；[dml.rs](../pkg/parser/ast/dml.rs) 是基础类型入口，其 `TableName`/恢复逻辑不能代替 PG 分段引号身份。实际共享 [ast/lib.rs](../pkg/parser/ast/lib.rs) 的 `ExprKind::Cast` 使用 FieldType，`InsertStmt` 有 OnDuplicate 赋值列表和 Returning 投影，却不足以表达 PG 冲突目标及两类谓词；候选扩展仍须逐消费者取证。

[optimize.rs](../pkg/planner/optimize.rs) 的 `Optimize`、`OptimizeAstNode`、`OptimizeAstNodeNoCache` 接收 `resolve::NodeW` 及共享会话/InfoSchema；Optimize 区分 EXECUTE 与普通缓存路径。这是共享规划边界证据，不证明任意新 PG 语法可直接交给它，更不能通过绕开正常入口跳过只读准入、缓存或成功后效应。[pg_extended.rs](../pkg/server/pg_extended.rs) 的 `Extended::process` 对普通 Parse 依次执行名称/SQL 适配、`markers`、结果适配并调用 `prepare_statement`，会话/目录分支另行处理；普通参数 OID 缺省/为零时仍返回 0A000。这里没有已取证的接收 PG AST 的准备接口。`markers` 的重复/乱序参数映射也不是原查询诊断映射。

独立测试已读 [yy_parser_test.rs](../pkg/parser/yy_parser_test.rs) 的 `failure_returns_warnings_and_parser_reuse_clears_them`、`hint_uses_live_scanner_mode_and_position`（警告生命周期、scanner mode/位置），以及 [pg_extended_test.rs](../pkg/server/pg_extended_test.rs) 的 `marker_mapping_preserves_literals_comments_and_index_order`、`canonical_prepare_parameter_syntax_probe`（词法保护、参数顺序及共享准备入口的语法边界断言意图）。它们没有覆盖本块两条路线的端到端保真，且本轮未执行。`pkg/parser/doc.go`、`pkg/parser/ast/doc.go`、`pkg/planner/doc.go`、`pkg/server/doc.go` 不存在；不把模块说明或历史通过记录当作当前行为证据。

| 比较维度 | 候选 A：扩展共享 AST / 现有解析链 | 候选 B：独立受控 PG 解析入口，再交共享内核 |
| --- | --- | --- |
| 接线成本 | 可复用 ParseSQL、Node 与既有访问链；仍需连接方言接入、Rust 规约动作及缺失字段，不能只改 parser.y。 | 需新增有限 PG 词法/语法及保真转换，并取证共享准备入口如何接收节点；当前 prepare_statement 只见 SQL，接口成本未知。 |
| 共享内核复用 | 载体较近，但每个新增字段仍须穿过 visitor、resolver、planner、Describe；字段存在不等于消费者支持。 | 必须转换到共享表示及正常规划入口；无法表达的语义先明确拒绝，不能建 PG 专用执行内核绕过。 |
| MySQL 回归面 | 修改共享词法、优先级、SQL mode 或 AST 恢复可能影响现有入口；ILIKE/RETURNING 已有扩展候选，须先记录真实 MySQL 基线，不能直接删去。 | 独立入口可隔离语法接受面，但共享表示/消费者与缓存仍可能回归；必须验证交错连接和重连，不能据入口独立即宣称隔离。 |
| 源码跨度/错误定位 | yySetOffset 是起点，仍需补原拼写、quoted、逐节点跨度和改写映射；现有 near/line 不能冒充 PG P。 | 可从原 SQL 记录跨度，但转换及内核诊断必须回溯原查询；字节与字符位置、多字节前缀及 EOF 都要验证。 |
| 参数绑定 | 复用现有准备路径需保留 `$n` 身份和出现顺序，不能让每个 `?` 变成独立 PG 参数；OID 推断仍为缺口。 | PG 参数节点可保留编号，但转换/准备元数据须证明重复绑定一致；不能用参数值拼 SQL 或暗中沿用文本替换。 |
| 维护成本 | 一套主要语法链，但双方言分支与字段语义须持续回归；受共享语法限制的成本尚未测量。 | 增加语法与转换层的双向维护、版本跟踪及拒绝集合；必须逐能力登记，不能把外部 parser 的接受范围当成支持集合。 |

六类语法的最小保真清单沿用对应小节，不重定语义：`::` 保留转换链、目标名/修饰及参数；ILIKE 保留操作类别、NOT、ESCAPE 出现性/表达式和优先级；RETURNING 保留三类 DML 的有序投影/别名及 Describe 交接；ON CONFLICT 保留省略/推断/约束目标、两种动作、两类谓词及 excluded 绑定；ARRAY 保留构造类别、维度嵌套与空列表；ROW 保留显式性、字段顺序/嵌套并区分分组和子查询。另须携带标识符分段、原拼写/引号、CTE 定义顺序和查询块作用域，供 resolver 消费；schema/search_path 的状态执行归工作流 5、目录可见性归工作流 6。不得把这些信息提前压成 MySQL SQL 后再声称仍然保真。

后续原型是待新建的独立任务，限定同一组原始 SQL、连接身份、编码、显式参数 OID 与参数编号作为输入，两候选使用相同用例清单。候选接口只描述数据交接，不命名新 API：A 从带连接身份的解析入口产生扩展共享节点；B 从受控 PG 入口产生保真语法表示，再转换为共享节点。两者均输出原 SQL/节点跨度、参数身份、诊断及共享消费者交接结果。先取证节点到 `resolve::NodeW` 和准备元数据的必要局部接线，不能假定现有 prepare_statement 接收 AST；本轮不实现此原型，也不生成接口定义。

下表是原型最小输入集，完整行为用例仍引用前面小节，所有树形、绑定、SQLSTATE 与位置均未实测。成功的语法项 SQLSTATE 为“不适用（成功）”，不代表可执行；涉及数据修改的输入只解析/描述，不发出写入。

| 输入、正反例 | PG 目标及责任 | MySQL 对照及证据边界 |
| --- | --- | --- |
| `SELECT ($2)::int4, ($1)::int4, ($2)::int4`，参数显式 int4；反例 `SELECT 1::` | 正例有三个独立转换节点及编号序列 2/1/2；反例目标 42601，parser 定位原输入末尾，工作流 7 传输 P。精确 P 待参考端采集。 | `::` 是隔离负例；自身正例用三个 CAST(? AS SIGNED)，绑定顺序 2/1/2。MySQL 精确诊断码/位置仍待取证。 |
| `SELECT 'Ab' ILIKE 'a%' ESCAPE '!'`；`SELECT ':: $1 ILIKE' /* $2 */` | 前者保留谓词与显式 ESCAPE；后者只有字符串和注释，不制造参数或转换。匹配值/locale 交工作流 4，成功无错误位置。 | 前者是目标隔离负例，但现有共享扩展基线须先核实；后者为自身字符串正例。不能用 LIKE 替换证明 PG 语义。 |
| `INSERT INTO t(id) VALUES ($1) ON CONFLICT (id) DO NOTHING RETURNING id`；反例尾部只有 RETURNING | 保留冲突目标、动作及投影；缺投影目标 42601，定位 EOF。绑定需后续 t 夹具，写入/返回行交工作流 5；合法未开放路径目标先于副作用拒绝 0A000。 | PG 子句为目标负例；自身 INSERT/ON DUPLICATE KEY UPDATE 用例单独验收，不认为动作等价。本地拒绝码/位置仍缺证。 |
| `SELECT ARRAY[1,2], ROW(1,2), (1)`；反例 `SELECT ROW(1,` | 区分数组、两字段行与标量分组；未闭合输入目标 42601，精确末尾位置待采集。数组/record 类型及编码不由原型补齐。 | ARRAY 构造不作为自身正例；ROW/分组按自身合法上下文检查，不能要求 MySQL 返回 PG 复合值。精确诊断待双方参考取证。 |

以上诊断输入再加入合法多字节字符串/注释前缀、带引号分段名和嵌套 WITH，核对跨度及作用域保真，不扩写执行功能。原型成功标准是：两路线分别给出这些输入的树形/拒绝、原 SQL 字符定位、参数映射及共享消费者最小交接证据，并记录修改面、不能承载字段和维护代价；双方言基线、并发连接身份及缓存隔离须有独立证据。可暂以明确门禁拒绝未开放合法语法，但拒绝本身不能算该语法交接成功。若必须丢信息、通用文本替换、用全局可变方言开关、执行写入获取 Describe 或补建第二套执行内核，该路线淘汰；若只是消费者尚未支持，应记录共同缺口和下一项局部取证，不能凭两路线均未编译/零测试选胜者。证据仍不足时保持未决，明确缺少哪个输入/交接结果，不递归补建子系统；原型完成后单独评审承载决策，再按能力细分实现。

官方依据（2026-10-23 查阅）：PostgreSQL **18** §4.2 [Value Expressions](https://www.postgresql.org/docs/18/sql-expressions.html) 的参数、转换、数组与行分类，以及 [INSERT](https://www.postgresql.org/docs/18/sql-insert.html) 的冲突/返回子句；协议 §54.2.3 [Extended Query](https://www.postgresql.org/docs/18/protocol-flow.html#PROTOCOL-FLOW-EXT-QUERY) 区分 Parse、Bind 与 Execute，§54.8 [错误字段](https://www.postgresql.org/docs/18/protocol-error-fields.html) 规定 P 为原查询从 1 起的字符位置；[附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html) 区分 42601 与 0A000。其他语法及 MySQL 规则沿用前面各块的官方来源和缺证说明。18.6 仅为来源指定目标，本轮没有该补丁版本或真实数据库的运行证据。

#### 章节验收与后续交接

本块累计审阅本章十二个前序文档块，对应来源任务四个步骤及完成条件；只确认文档契约可交接。前序交付可由提交 `a19b8cba62` 至 `00c684fb49` 的十二条文档提交和本章小节核对，不依赖已删除的编号任务文件。方案结论仍是先做最小原型，未选定最终承载方式；所有功能和双方言行为仍为未验证。

下表将来源步骤映射到小节、后续独立实施边界与验收入口。入口中的用例族均待新增或细化，沿对应小节的 PG/MySQL 正反例、夹具、目标结果与诊断责任登记，不代表现有通过测试；本表不重复 SQL，也不展开完整 parser 实施清单。

| 来源需求 | 本章小节及交接内容 | 后续独立实施块 | 验收入口与所需证据 |
| --- | --- | --- | --- |
| 步骤 1：连接私有身份链 | [方言身份传递](#方言身份传递)：listener→session→parser→resolver→planner→expression context | 先取证身份拥有权与关闭/重连，再逐交接点细分 | 待新增 DIALECT-IDENTITY 三类场景；简单/扩展查询、两 PG 连接与 MySQL 交错、缓存隔离均需当前证据，不能只比 SELECT 1 的值。 |
| 步骤 2：`::` | [类型转换语法边界](#类型转换语法边界)：转换链、类型名称与参数来源 | 单独取证转换节点保真，类型可行性交工作流 3 | 待新增该小节的优先级/嵌套、重复参数、缺类型与词法保护断言；区分树形、类型失败及原 SQL 位置。 |
| 步骤 2：ILIKE | [大小写匹配语法边界](#大小写匹配语法边界)：操作类别、否定与 ESCAPE | 单独细分匹配语法，求值交工作流 3、4 | 待新增该小节的谓词/NOT/优先级/ESCAPE 正反例；先采集 MySQL 既有扩展基线，不能将 LIKE 当等价翻译。 |
| 步骤 2：RETURNING | [返回子句语法边界](#返回子句语法边界)：三类 DML 的投影与 Describe 交接 | 单独细分投影保真与描述接线，副作用交工作流 5 | 待新增三类 DML、零行仍有列、空投影/错位拒绝；Describe 不写入，工作流 3、7 分别验证类型和元数据，0A000 门禁不算功能成功。 |
| 步骤 2：ON CONFLICT | [冲突子句语法边界](#冲突子句语法边界)：目标、动作、双谓词与 excluded | 单独细分冲突语法及绑定，执行交工作流 5 | 待新增 CONFLICT-SYNTAX；省略/推断/约束目标及反例逐项检查，不以 MySQL 冲突更新代替 PG 契约。 |
| 步骤 2：数组构造 | [数组构造语法边界](#数组构造语法边界)：构造类别、嵌套与空列表 | 单独细分数组树形，定型/形状与编码交工作流 3、7 | 待新增 ARRAY-SYNTAX；区分语法完整、空数组定型、混合类型及非矩形拒绝，不把数组改成行或 JSON。 |
| 步骤 2：行构造 | [行构造语法边界](#行构造语法边界)：显式/隐式行、标量分组与嵌套 | 单独细分行/分组保真，复合类型交工作流 3 | 待新增 ROW-SYNTAX；ROW(1,2)、(1,2)、(1)、ROW(1, 分别验收树形、定型/拒绝与执行边界，不强求 MySQL 可返回 PG record。 |
| 步骤 3：引号与大小写 | [引号与大小写规则](#引号与大小写规则)：原拼写、quoted 与绑定身份 | 单独细分名称保真与精确绑定，后端可区分性另取证 | 待新增 NAME-QUOTING；foo/Foo/"Foo"、双引号转义及缺对象反例，MySQL 配置分别记录，不能让 lowercase 合并对象。 |
| 步骤 3：限定名 | [限定名称解析规则](#限定名称解析规则)：关系分段、列/别名及当前数据库隔离 | 单独细分关系限定与列作用域，目录协作工作流 6 | 待新增 NAME-QUALIFIED；t/public.t/db.public.t、跨库拒绝及 a.id，不截断前缀后误读本地表。 |
| 步骤 3：CTE 遮蔽 | [公共表表达式遮蔽规则](#公共表表达式遮蔽规则)：定义顺序、查询块与绑定对象 | 单独细分普通 CTE 绑定与递归门禁，不补建递归执行 | 待新增 NAME-CTE；嵌套同名、显式表绕过、前向/自引用/重复名及退出作用域，字符串集合保留不等于绑定通过。 |
| 步骤 3：schema/search_path | [模式搜索路径解析契约](#模式搜索路径解析契约)：有效有序路径与显式名 | 单独细分 resolver 消费路径，状态/目录分别交工作流 5、6 | 待新增该小节的 public 在/不在、catalog 顺序、空路径与两会话隔离；绑定时点及缓存重分析另取参考端证据。 |
| 步骤 4：两条方案比较 | [解析承载方案决策](#解析承载方案决策)：共享 AST 扩展和受控 PG 入口 | 独立最小原型取证后复审，不新增第二套执行内核 | 待新增该小节的同输入候选对照；按六个维度审查树形、参数、原 SQL 跨度与共享消费者交接，缺证保持未决。 |
| 完成条件：双方言示例、错误位置及隔离 | 本表各小节及[方言身份传递](#方言身份传递)的交错连接用例 | 单独细分原 SQL 诊断与双方言回归；扩展加载继续排除 | 待新增中文前缀、引号/限定名改写、EOF 和 Q/扩展 Parse 的位置断言；PG SQLSTATE/P 与 MySQL 自身诊断分别采证，不能把改写后的字节偏移当字符位置。 |

本轮复核源码的基线为 `00c684fb49`：`pg_name.rs::Resolver::scope` / `relation`、`rewrite` / `adapt` / `validate_relations` 仍是有界名称扫描、文本改写及成功解析 AST 的跨库守卫；`PgSession` 只保存局部路径及失败事务状态，`has_public` / `schema` 不能替代有效路径和目录模型；`Extended::process` 的普通 Parse 仍先适配和 markers，再向 `prepare_statement` 传 SQL，未知参数 OID 返回 0A000。未见上述入口提供完整 PG 语法表示、全链身份或原 SQL 持久位置映射，不从这些局部事实推断所有其他入口均不支持。

本轮实际阅读独立测试 `pg_name_test.rs::pg_introspection_names_token_scopes` / `pg_introspection_names_rejection_boundaries`，以及 `pg_query_test.rs::pg_introspection_namespace_session_state` / `pg_introspection_names_live_tables`：前两者断言 rewrite 的文本/拒绝，后两者有两连接局部路径、current_schema 及 Q/扩展查询名称适配的断言意图。它们没有替代本表逐能力的树形、绑定对象、精确错误位置和 PG/MySQL 全链路隔离验收；本轮未执行这些测试。`pkg/server/doc.go` 不存在，模块说明只作导航。

交给工作流 3 的只读输入是本章及第 1 章的矩阵/证据/用例格式：解析结构须保留操作类别、有序表达式、转换目标、数组/行嵌套、参数编号及原 SQL 跨度；名称绑定须提供分段/引号身份、CTE/别名作用域、连接数据库及有效名称环境；类型阶段再确定类型名与重载、共同元素类型、空数组定型、行字段类型、参数推断、转换/NULL 和结果元数据，不由语法接受替代类型判定。本章未证明这些交接接口已存在，也不命名新 API；工作流 3 应逐能力补齐类型目标和未知诊断，不接管 parser 完整实现。

跨工作流责任保持独立：工作流 5 拥有 search_path 设置/事务恢复、DML 副作用与递归执行；工作流 6 拥有 schema/对象/权限可见性及目录身份；工作流 7 拥有参数/结果编码、Describe 协作、SQLSTATE/P 传输与协议恢复。本章只约束交接，函数/运算符求值归工作流 4，客户端共存证据汇入工作流 8。本轮不执行这些工作流，也不创建运行器、数据库环境、代码或原型。

累计文档验收条件是：本章恰有十三个四级块且五个责任索引可达；上述来源四步和完成条件无遗漏；每个语法/名称规则均能定位模块、目标、范围外、验收与单独细分入口；正反例分方言写明前提、目标、错误责任及缺证状态；两方案维持最小原型取证门禁，工作流 5/6/7 未被吞并。本轮完成仅表示这些文档条件满足。全链路身份/缓存隔离、语法树保真、名称绑定、类型/OID、执行与副作用、精确 SQLSTATE/P、真实客户端/双方言差分及性能均未验证；未知错误码、是否提供 P 及其数值先补官方来源或参考端取证，禁止猜造。

本块没有新增外部行为规则；累计目标沿用各小节的具体官方来源。2026-10-23 复核 PostgreSQL **18** §4.2 [Value Expressions](https://www.postgresql.org/docs/18/sql-expressions.html) 的语法/类型交接分类、§5.10 [Schemas](https://www.postgresql.org/docs/18/ddl-schemas.html) 的名称责任和 §54.8 [错误字段](https://www.postgresql.org/docs/18/protocol-error-fields.html) 的原查询字符位置要求。其他规则与 MySQL 8.4 对照保留各块的官方依据及缺证说明；18.6 仅为来源目标，没有该补丁版本实测证据。

### 范围外

[章节验收与后续交接](#章节验收与后续交接)不增加能力设计、代码、原型或完整 parser 实施清单，不执行工作流 3；工作流 5/6/7 的状态、目录和协议责任独立交接。

[解析承载方案决策](#解析承载方案决策)本轮不写原型、API、parser/AST 或执行代码，不增加依赖、平台、数据库环境或性能基准；会话、类型、目录、协议及执行实现继续由相应工作流独立细分。

[方言身份传递](#方言身份传递)本轮不实现方言类型、解析原型、共享 AST、缓存接线或运行测试平台；不扩建 resolver、planner、表达式子系统。六类语法、引号/大小写、限定名、CTE 及 schema/search_path 名称规则由后续文档块定义；会话设置执行归工作流 5，目录实现归工作流 6，错误传输与恢复协作工作流 7。现有文本适配只作为有界入口证据，不承担通用 SQL 翻译方案，也不据其推断完整 PG 支持。

[类型转换语法边界](#类型转换语法边界)不实现 PG parser、转换执行、类型推断或元数据；不扩建类型目录、数组/行子系统。转换语义归工作流 3，协议 OID、错误传输与恢复协作工作流 7。

[大小写匹配语法边界](#大小写匹配语法边界)不实现 parser/AST 接线、匹配算法、locale/collation、NULL、重载或错误响应；求值语义交工作流 3、4，协议诊断交工作流 7，不扩展其他匹配家族。

[返回子句语法边界](#返回子句语法边界)不实现解析器、共享 AST 接线、投影求值、事务或 wire 编码；真实写入/返回行交工作流 5，类型和结果描述协作工作流 3、7。ON CONFLICT 本身及 OLD/NEW、MERGE 等组合不由本块扩写实现。

[冲突子句语法边界](#冲突子句语法边界)不实现 upsert、唯一性/索引推断、锁、并发、更新求值和事务；执行与副作用交工作流 5，名称/类型协作本章后续规则及工作流 3，错误响应交工作流 7。

[数组构造语法边界](#数组构造语法边界)不实现数组类型、下标、运算符、存储或 wire codec；ARRAY(子查询) 是另一个合法构造形式，留待单独定义支持集合，不能当作本块已覆盖。类型、转换及 OID 归工作流 3，参数/结果编码归工作流 7。

[行构造语法边界](#行构造语法边界)不实现 parser/AST、复合类型、行比较、NULL 求值或协议编码，不补建关系分组解析；类型与转换交工作流 3，比较/NULL 交工作流 3、4，元数据和错误响应交工作流 7。

[引号与大小写规则](#引号与大小写规则)不实现词法器、名称绑定、存储大小写映射或错误位置接线，不扩建目录、CTE 或 search_path；后端名称区分能力作为独立缺口交接，不以文本改写代替。

[限定名称解析规则](#限定名称解析规则)不实现 parser/AST、名称绑定、权限、跨库访问或完整 schema 模型；CTE/search_path 交后续块，会话状态和目录实现分别归工作流 5、6，协议诊断归工作流 7。

[公共表表达式遮蔽规则](#公共表表达式遮蔽规则)不实现解析/绑定、递归执行、物化或优化，不扩建目录及 search_path；只固定名称可见性与入口门禁责任，执行协作工作流 5、目录协作工作流 6、诊断协作工作流 7。

[模式搜索路径解析契约](#模式搜索路径解析契约)不实现路径状态、RESET、事务恢复、目录对象或权限可见性；工作流 5、6 分别主责。只约束 resolver 消费顺序、隐式 catalog、空路径和限定名的交接，不承诺任意 schema 已可用。

### 验收

[章节验收与后续交接](#章节验收与后续交接)按本块累计条件检查十三个文档块、来源映射和双方言诊断责任；源码/测试阅读及文档完成均不能升级为功能通过。

[解析承载方案决策](#解析承载方案决策)的文档验收为：六个比较维度均有源码依据及缺证限制，六类语法信息齐全；已选择先做最小原型，输入、候选数据交接、成功/淘汰标准和后续任务边界明确。PG/MySQL 正反例、SQLSTATE/原 SQL 定位责任与未验证状态分开记录；不以候选节点存在或静态测试阅读声称方案已可用。

[方言身份传递](#方言身份传递)的文档通过条件：六段责任链均区分实际入口、状态拥有者/生命周期、目标与缺口；简单/扩展查询身份一致；PG/MySQL 同时连接及两 PG 连接交错场景可单独拆分，负例、错误责任与证据状态明确；局部测试阅读不升级为全链路支持。当前仅文档契约交付；parser 内部接线、共享名称绑定、规划/缓存、表达式上下文、错误位置及真实双方言隔离行为全部未验证，实施后须按第 1 章采集有效证据。

[类型转换语法边界](#类型转换语法边界)的文档验收为：上述输入形态、优先级/嵌套、参数位置、缺类型反例及词法保护均有双方言目标、诊断责任和证据状态；AST 候选与保真缺口明确。现有测试阅读不替代新增树形/位置断言及实际双方言对照，所有行为仍未验证。

[大小写匹配语法边界](#大小写匹配语法边界)的文档验收为：操作符类别、左右操作数、否定、ESCAPE 显式性与保真、优先级及不完整输入均有双方言目标和诊断责任；当前源码、测试意图与运行证据区分清楚。全部运行行为仍未验证，不能以共享语法或同值结果代替 PG 树形与双方言隔离证据。

[返回子句语法边界](#返回子句语法边界)的文档验收为：三种 DML、表达式/星号/别名、空投影与错位反例、零行和词法保护均有双方言目标与诊断责任；解析树、返回列元数据、真实副作用分开验收。现有非空 Returning 的 0A000 门禁不得被当作功能成功，全部运行行为未验证。

[冲突子句语法边界](#冲突子句语法边界)的文档验收为：目标的三种形态、两种动作、两类谓词、excluded 与目标别名、缺动作/缺目标反例均有保真和诊断责任；MySQL 对照、参数/词法保护及执行交接明确。所有运行行为未验证，不能将普通 INSERT 测试或文本改写当作冲突行为通过。

[数组构造语法边界](#数组构造语法边界)的文档验收为：基本构造、两种嵌套写法、空数组定型、混合类型、非矩形与未闭合括号分别有树形、语义交接和诊断责任；PG/MySQL 正反例、参数/词法保护及证据限制齐全。文档完成不表示数组可解析、可绑定或可编码，所有运行行为未验证。

[行构造语法边界](#行构造语法边界)的文档验收为：ROW(1,2)、(1,2)、(1)、ROW(1, 四个核心输入分别区分语法合法、类型可判定与可执行；单字段 ROW 对照、顺序/嵌套、参数、关系括号及词法保护均有双方言责任和证据限制。当前全部运行行为未验证，不以解析或名称扫描成功替代语义兼容。

[引号与大小写规则](#引号与大小写规则)的文档验收为：foo、Foo、"Foo"、"a""b"、未闭合/空引号及字符串/注释均有双方言正反例、前提和诊断责任；词法键与后端存储、原始与转换位置区分明确。现有适配和测试不能证明完整名称解析，运行能力全部未验证。

[限定名称解析规则](#限定名称解析规则)的文档验收为：t、public.t、db.public.t、other.public.t 和 a.id 均区分当前改写/拒绝与 PG/MySQL 目标；别名隐藏、缺列、残缺/多段名有诊断责任及缺证说明；隔离检查不被文本降级绕过。所有运行行为仍未验证，测试阅读不构成通过。

[公共表表达式遮蔽规则](#公共表表达式遮蔽规则)的文档验收为：基本 WITH、嵌套同名、public.t、定义顺序/前向、自引用及递归结构反例均有双方言目标；作用域生命周期、当前 RECURSIVE 拒绝和未验证项明确。当前测试仅为静态证据，所有解析、绑定、结果及诊断行为均未实测。

[模式搜索路径解析契约](#模式搜索路径解析契约)的文档验收为：public 在/不在路径、显式 public.t、catalog 同名及顺序、空路径、两 PG 会话和 MySQL 旁路均有目标及诊断责任；原始/有效路径、当前布尔门禁与完整目标区分清楚。全部运行行为仍未验证，状态/目录阅读不升级为绑定或兼容通过。

### 后续细分

[章节验收与后续交接](#章节验收与后续交接)将本章与第 1 章证据契约交工作流 3；实现先核实连接身份和最小原型，再按单一语法或名称规则重排依赖，逐项补运行证据。

[解析承载方案决策](#解析承载方案决策)后续先单独拆两候选最小原型取证和决策复审，再按语法保真、共享准备/规划接线、原 SQL 诊断、参数身份及双方言隔离分别细分；每项限定输入与消费者边界。共同类型/执行/目录缺口另行交接，禁止借原型一次补齐全部 PG parser。

[方言身份传递](#方言身份传递)后续先做连接身份拥有权及关闭/重连边界的局部取证，再分别拆简单查询与扩展查询的身份承接、共享解析/名称绑定/规划/表达式上下文的必要局部接线及隔离回归；实施前确认实际落点，不将本责任链自动扩张为完整子系统重建。各任务明确单一交接点、所需状态、现有可复用实现和验收断言；DIALECT-IDENTITY 三类场景是待新增验收入口，不是既有测试或已登记通过的能力。

[类型转换语法边界](#类型转换语法边界)后续先拆最小 `::` 解析承载取证，比较现有 Cast 与受控入口能否保留上述字段；再按优先级/嵌套、参数位置与词法保护、原 SQL 错误定位分别细分。每项复用现有 parser 与独立测试布局，只补必要局部接线；将转换语义及参数类型推断交工作流 3，不在此建立完整类型系统。

[大小写匹配语法边界](#大小写匹配语法边界)后续先核对 Rust parser 到现有载体的最小接线及 MySQL 扩展基线，比较共享 AST 与受控 PG 入口的保真成本；再分别拆谓词/NOT/比较组合、ESCAPE 信息与词法保护、原 SQL 诊断及隔离验收。沿现有独立测试布局补专项树形/位置断言，不递归修建执行子系统；匹配算法和转义求值另交工作流 3、4。

[返回子句语法边界](#返回子句语法边界)后续先按三种 DML 核对 Rust 解析动作到共享载体的最小保真，再拆表达式/星号/别名与原 SQL 位置、planner/Describe 列交接、双方言拒绝隔离。功能开放须等待工作流 5 的真实副作用/回滚和工作流 7 的结果描述证据，不递归补建执行或协议子系统。

[冲突子句语法边界](#冲突子句语法边界)后续先比较共享 AST 与受控 PG 入口保真，再分别拆目标/动作与双谓词、excluded 绑定、原 SQL 跨度与参数映射、双方言隔离的独立验收。先补解析树和拒绝证据；真实唯一性、并发及更新条件效果由工作流 5 单独细分，不递归扩建执行子系统。

[数组构造语法边界](#数组构造语法边界)后续先比较共享 AST 与受控入口保留构造类别、嵌套及跨度的最小方案，再拆基本/嵌套/空列表树形、原 SQL 定位、参数身份与双方言隔离验收；共同类型、形状语义及数组 OID 交工作流 3，Bind/Describe/结果编码交工作流 7 单独细分，不递归补建数组子系统。

[行构造语法边界](#行构造语法边界)后续先比较共享 Row 载体与受控 PG 入口保留显式性、嵌套和原 SQL 跨度的最小接线，再分别拆行/分组/子查询树形、名称扫描交互、错误位置及双方言隔离断言。沿独立测试布局取证，不递归扩建表达式或关系子系统；行类型与 NULL/比较按工作流 3、4 单独细分。

[引号与大小写规则](#引号与大小写规则)后续先取证共享名称载体的 quoted/拼写/跨度保真，再分别拆折叠与精确绑定、双引号转义及词法保护、位置映射和双方言隔离；后端同名异大小写的可区分性单独评估，禁止递归补建存储或目录子系统。

[限定名称解析规则](#限定名称解析规则)后续先取证关系分段与列限定的上下文保真及当前数据库身份，再分别拆关系限定/跨库拒绝、列与别名作用域、原 SQL 位置和双方言隔离断言；沿现有独立测试补 adapt/guard 的必要覆盖，不扩建目录或权限子系统。

[公共表表达式遮蔽规则](#公共表表达式遮蔽规则)后续先核对顺序/嵌套/名称身份与跨度的保真，再分别拆普通 CTE 绑定与显式表绕过、前向及重复名诊断、查询块退出/跨语句与双方言隔离断言；递归入口依赖图与门禁另拆，不借名称任务补齐递归执行或物化子系统。

[模式搜索路径解析契约](#模式搜索路径解析契约)后续先核对状态到 resolver 的有效路径交接，再拆 public/显式限定、catalog 同名顺序与空路径、两会话及双方言隔离的独立验收；关系绑定时点与缓存失效先做参考端取证。任意 schema、权限、临时项及用户项分别交工作流 5、6 细分，不递归补建目录或会话子系统。

## 3. 共享类型与转换契约

### 模块

章节验收与后续交接以本章前18块为累计契约，依赖交付为 `f4cde1ed76`（准备描述执行元数据契约）。本轮复核 `pkg/server/pg_types_test.rs::common_type_oids_and_values`、`pkg/server/pg_extended_test.rs::text_parameters_are_typed_and_reject_loss` 以及 `pkg/server/pg_client_integration_test.rs::run_clients`：前两者分别有普通结果OID/值和部分参数成功/拒绝断言，后者启动真实客户端并检查MySQL隔离，不能外推全部类型差分覆盖。经RustCodeGraph读取，均未运行；pkg/server无doc.go。生产入口及各族独立测试沿前序模块段，不新增接口或测试文件。

[规范类型与协议映射](#规范类型与协议映射)复用共享内核；`pkg/types/field_type.rs::FieldType` 是 parser 类型别名，`AggFieldType` 保留 MySQL 混合符号整数提升规则，不以 PG 目标改写 Go 既有语义。`pkg/server/conn.rs::NativeType` 的 code/flags/length/decimal、`QueryResult` 与 `PreparedMetadata` 承接原始类型；`runtime.rs` 的 Prepare 分支及 `result_metadata` 从结果字段提取这些信息，`protocol_result_column` 另产 MySQL 列描述。PG 出口为 `pg_result.rs::pg_type` / `encode_formats` / `value_text` / `value_binary`；入口为 `pg_extended.rs::parameter` / `binary_parameter`。这些是现有符号，不是新建接口。第 2 章交来的转换目标、参数编号、数组结构及名称身份须先定型；本块不替 parser 或 resolver 决定语法与绑定。

本块静态取证基线为 `69ef39f91a781a4147568622943d719c95e3f7cd`，日期 2026-10-23。已阅读上述符号及独立测试 `pkg/server/pg_types_test.rs` 的 `common_type_oids_and_values`、`native_metadata_bridge_preserves_boolean_and_mysql_columns`、`unsigned_binary_and_unsupported_types`、`missing_original_metadata_is_explicitly_unsupported`、`catalog_char_and_acl_wire_types`、`pg_binary_native_results`，以及 `pkg/types/field_type_test.rs::TestFieldType` 的宽度、精度和 flags 断言。前者覆盖意图包括同值异类型、unsigned 最大值、bytea、JSON/零日期/duration 拒绝及缺元数据；目录 char/ACL 断言不能证明通用数组支持。目标包没有 `doc.go`。全部测试未运行，源码阅读不是当前功能通过证据。

[布尔类型契约](#布尔类型契约)沿用上述桥接，补充取证 `pkg/types/datum.rs` 的 Kind 标签和 `Datum::ToBool`（MySQL 风格真值转换，不能直接作为 PG 隐式转换），以及 `pg_extended.rs::parameter` / `binary_parameter` / `Extended::process`、`pg_result.rs::pg_type` / `value_text` / `value_binary`。静态基线为 `9b3d77f784`（2026-10-23），目标包无 `doc.go`；独立测试 `pg_types_test.rs::common_type_oids_and_values` 断言 TRUE/FALSE 的 OID 16 与 t/f，`native_metadata_bridge_preserves_boolean_and_mysql_columns` 断言同值整数与布尔的 flags 区别，`pg_binary_native_results` 含 false→00；`pg_extended_test.rs::text_parameters_are_typed_and_reject_loss` 只含 true 成功和 unknown 拒绝的布尔参数断言；`pkg/types/datum_test.rs::test_to_bool` 是共享真值转换意图，不证明 PG 转换规则。以上测试均未运行。

[整数与无符号边界](#整数与无符号边界)取证基线为 `01ce26d145`。实际读取 `pkg/server/pg_result.rs::pg_type/value_text/value_binary`、`pkg/server/pg_extended.rs::parameter/binary_parameter/Extended::process`、`pkg/types/datum.rs::KindInt64/KindUint64/Datum` 与 `pkg/types/convert.rs::IntegerUnsignedUpperBound/IntegerSignedUpperBound/IntegerSignedLowerBound/ConvertIntToInt/ConvertUintToInt/ConvertIntToUint`。两个包均无 doc.go。独立测试已读 `pkg/server/pg_types_test.rs::unsigned_binary_and_unsupported_types/pg_binary_native_results/common_type_oids_and_values`、`pkg/server/pg_extended_test.rs::text_parameters_are_typed_and_reject_loss` 和 `pkg/types/convert_test.rs::test_convert`；它们是断言意图，均未运行。Datum 区分 i64/u64 承载，但不能单靠值盒确定 int2/int4/int8 声明身份。

[精确数值契约](#精确数值契约)取证基线为 `7fe5fefab8`（本轮只改文档）：[mydecimal.rs](../pkg/types/mydecimal.rs) 的 MyDecimal、fit_unscaled、FromString、Round、ToBin/FromBin，与独立 [mydecimal_test.rs](../pkg/types/mydecimal_test.rs) 的 TestRoundWithHalfEven、TestRoundWithTruncate、TestToBinFromBin；HalfEven 名称实际调用 ModeHalfUp，不能按名字推定银行家舍入。MyDecimal 以 9 个 base-10^9 word 分别容纳整数和小数，容量按两部分各自向上取整占 word，不能简单承诺任意 81 位组合；BigInt 是运算中间量，不表示存储域无限。MAX_DECIMAL_SCALE=30 用于特定编码/运算限制，不是所有方法统一的 PG scale 上限。

cast 另见 [builtin_cast.rs](../pkg/expression/builtin_cast.rs) 的 to_decimal、parse_decimal、produce_decimal：使用 rust_decimal::Decimal，解析失败可经浮点回退，负 scale 被 max(0)，超域产生 warning 并裁剪；常量 MAX_DECIMAL_WIDTH=65 不能证明实际能精确承载 65 位。参数见 [pg_extended.rs](../pkg/server/pg_extended.rs) 的 parameter/binary_parameter；结果见 [pg_result.rs](../pkg/server/pg_result.rs) 的 pg_type、value_text、value_binary、encode_formats。独立 [pg_extended_test.rs](../pkg/server/pg_extended_test.rs) 的 text_parameters_are_typed_and_reject_loss 仅含 12.340 接受及 12bad 拒绝意图；[pg_types_test.rs](../pkg/server/pg_types_test.rs) 有 DECIMAL(8,2)→OID 1700/text 12.34 断言及 U64 文本入口。均未运行，不证明 PG 大精度、typmod、特殊值或 binary 支持；上述三个包均无 doc.go。

[浮点数值契约](#浮点数值契约)静态取证基线为 `cae627a7dc`（2026-10-23，批次 4 精确数值契约已交付）。`pkg/types/datum.rs::Datum` 的 GetFloat32/SetFloat32、GetFloat64/SetFloat64 分别保留 KindFloat32/KindFloat64；SetFloat32FromF64 可将未窄化 f64 写入 float32 标签，标签本身不能证明值已符合 float4 域。`pkg/types/convert.rs::format_float` 使用 ryu 最短格式并展开指数，不是 PG 输出格式的证据。PG 边界仍为 `pg_extended.rs::parameter/binary_parameter` 和 `pg_result.rs::pg_type/value_text/value_binary/encode_formats`：700/701 参数分别解析 f32/f64，再转共享参数小端编码；binary 输入从网络序转小端。结果 Value::Float 共用 f64，text 按 f64 输出，binary 先经文本再解析到目标宽度，尚无完整窄化溢出和 float4 文本定宽证据。

已读独立测试 `pkg/server/pg_types_test.rs::pg_binary_native_results`，浮点部分含 float4 1.5、float8 正无穷及两者 SQL NULL 长度 -1 的 binary 输出断言；`common_type_oids_and_values` 没有浮点场景。另读 `pkg/server/pg_extended_test.rs::text_parameters_are_typed_and_reject_loss` 与 `pkg/types/convert_test.rs::test_convert_to_string`，后者有 1.6_f32/-0.6_f64 的共享输出意图，不能替代 PG 可逆文本验收。两个目标包无 doc.go；所有测试均未运行，未见上述断言覆盖完整浮点边界，不把静态路径或 OID 视为功能通过。

[文本类型契约](#文本类型契约)静态基线为 `e3c4a89ce7`（批次 5 已交付）。已读 [field_type.rs](../pkg/types/field_type.rs) 的 InferParamTypeFromDatum/InferParamTypeFromUnderlyingValue：字符串推断补 MySQL charset/collation，可变长度设 UnspecifiedLength；[datum.rs](../pkg/types/datum.rs) 的 SetString/GetValue 保留 KindString 和 collation，SetBytes 另设 KindBytes。这些信息不能单独区分 PG text/varchar/bpchar 身份或声明长度。目标包 pkg/types、pkg/server 均无 doc.go。

PG 入口 [pg_extended.rs](../pkg/server/pg_extended.rs)::parameter 先校验 UTF-8/NUL，再将 25/1043/1042 同转 tp=253；binary_parameter 对三者复用 parameter。NULL 提前返回 tp=6，typed NULL 连续性缺证。出口 [pg_result.rs](../pkg/server/pg_result.rs)::pg_type 将普通非 binary 字符串映射为 25，charset=63 则为 bytea；value_text 拒绝 NUL，value_binary 的字符串分支发送文本字节，encode_formats 固定 typmod=-1。不能从此宣称已保留 varchar/char 长度、补空格或跨格式身份。

已读独立 [pg_types_test.rs](../pkg/server/pg_types_test.rs)::common_type_oids_and_values 的 hello→25/NULL 断言、catalog_char_and_acl_wire_types 的受控目录 1042 断言，以及 [pg_extended_test.rs](../pkg/server/pg_extended_test.rs)::parse_bind_execute_sync 的 VARCHAR(30) 表入口。目录特例不证明普通 char 支持，上述片段不覆盖本块完整边界；所有测试未运行。

[字节类型契约](#字节类型契约)静态基线为 `c559d9ad7b`（2026-10-23，批次 6 已提交）。通过 RustCodeGraph 读取 [binary_literal.rs](../pkg/types/binary_literal.rs)::BinaryLiteral/String/ToString/ToInt/Compare：Vec<u8> 能保存字节，但 ToString 使用 from_utf8_lossy，ToInt/Compare 去前导零，不能直接作为 bytea 保真转换或比较。独立 [binary_literal_test.rs](../pkg/types/binary_literal_test.rs)::test_binary_literal/test_parse_hex_str_table 是 MySQL BIT/HEX 意图（如 0x1 补零），不能证明 PG 输入语法。pkg/types、pkg/server 均无 doc.go。

现状取证 [pg_extended.rs](../pkg/server/pg_extended.rs)::parameter/binary_parameter：OID 17 text 先验证 UTF-8/NUL，再要求 `\x` 前缀及连续偶数 hex 数字；非 hex 返回 0A000，奇数长度/非法数字返回 22P02，不接受 PG 合法的字节对间空白或 escape 输入。解码后与 binary 原字节入口都转 tp=252；NULL 提前转 tp=6，身份连续性缺证。[pg_result.rs](../pkg/server/pg_result.rs)::pg_type/value_text/value_binary/bytea/encode_formats 只对 string/blob 家族且 ColumnInfo.charset=63 选 OID 17，NativeType 本身没有 charset；结果 text 固定小写 hex，binary 的 Value::Bytes 直接复制。Value::Text 回退不能证明任意 bytes 保真。

已读独立 [pg_types_test.rs](../pkg/server/pg_types_test.rs)::unsigned_binary_and_unsupported_types 的 00 ff 5c→17/文本 `\x00ff5c`、pg_binary_native_results 的 00 ff 原字节结果，以及 [pg_extended_test.rs](../pkg/server/pg_extended_test.rs)::text_parameters_are_typed_and_reject_loss 的 `\x00ff` 成功/`\xabc` is_err。测试全部未运行，不证明完整往返、全字节域、精确错误或 typed NULL 元数据。

[日期类型契约](#日期类型契约)取证基线为 `aa462efc89`（批次 7 已提交）。按仓库 RustCodeGraph 读取 [time.rs](../pkg/types/time.rs)::ParseDate/ParseTimeFromNum：复用 MySQL TypeDate、fsp=0，整数零可构造零日期；[core_time.rs](../pkg/types/core_time.rs)::CoreTime/getYear/setYear 为 u64 压缩字段，年份提取为 u16，写入受掩码约束，不是 PG 有符号日序或扩展年份的保真证据。两个目标包均无 doc.go。

入口 [pg_extended.rs](../pkg/server/pg_extended.rs)::parameter/temporal 对 1082 用 chrono `%Y-%m-%d`，限制年份 1..9999，再转 MySQL tp=10 的年月日小端参数；无效日期当前统一 22007，超引擎年份为 0A000；binary_parameter 没有 1082 分支，非 NULL 返回 0A000，NULL 提前为 tp=6。[pg_result.rs](../pkg/server/pg_result.rs)::pg_type 将 10/14 映射 1082/4；value_text 仅粗查年非零、月 1..12、日 1..31，不能排除 2023-02-29 或 04-31；value_binary 再用 chrono 验证并输出相对 2000-01-01 的 i32 网络序日数，存在两格式校验差异。encode_formats/Extended::process 的 Describe 一致性检查不能替代日期身份、typed NULL 和错误分类取证。

独立 [time_test.rs](../pkg/types/time_test.rs) 的 ParseDate 实际断言（约 374–400 行）含短年份、特殊分隔符和 2011-02-30 拒绝，Go fixture 字符串本身不算执行断言；[pg_extended_test.rs](../pkg/server/pg_extended_test.rs)::text_parameters_are_typed_and_reject_loss 含 2024-02-29 接受、2026-02-29 拒绝但未断言 SQLSTATE；[pg_types_test.rs](../pkg/server/pg_types_test.rs)::common_type_oids_and_values 有 2026-09-30→1082/text，pg_binary_native_results 有 1999-12-31→-1 日及逐类型 NULL 输出断言。以上全部未运行，不能证明 PG 全域、两方向往返或准备元数据连续性。

[日内时间类型契约](#日内时间类型契约)静态基线为 `a8a704187c`（批次 8 日期块已提交）。通过 RustCodeGraph 读取 [time.rs](../pkg/types/time.rs)::ParseDuration/matchDuration/Duration::RoundFrac、MinTime/MaxTime/MaxFsp，以及 [datum.rs](../pkg/types/datum.rs)::KindMysqlDuration/KindMysqlTime：共享 TIME 是可负、可超过一天的纳秒 duration，Fsp 与日内身份不可混同。Duration 的日期转换可依赖当前日期和 location，不能直接当 PG time cast。pkg/types、pkg/server 均无 doc.go。

入口 [pg_extended.rs](../pkg/server/pg_extended.rs)::parameter/temporal 以 chrono `%H:%M:%S%.f` 解析 1083，非整微秒返回 0A000，再转 MySQL tp=11 的非负、零天数结构；binary_parameter 无非 NULL 1083 分支，返回 0A000；NULL 提前转 tp=6。出口 [pg_result.rs](../pkg/server/pg_result.rs)::pg_type/value_text/value_binary/encode_formats 将 code=11 映射 1083/8，typmod 固定 -1；text 只检查小时 <24，binary 另用 chrono 校验分秒与微秒。该差异不能证明两个格式同域，且当前拒绝合法 PG 24:00 边界。

已读独立 [time_test.rs](../pkg/types/time_test.rs)::TestToNumber 的实际 ParseDuration 断言（含负值及小数舍入；go_cases 字符串不作为执行断言）；[pg_types_test.rs](../pkg/server/pg_types_test.rs)::common_type_oids_and_values 的 12:34:56→1083/text、unsigned_binary_and_unsupported_types 的 25:00:00 拒绝、pg_binary_native_results 的 1 微秒→i64 网络序 1 与该类型 NULL 输出；[pg_extended_test.rs](../pkg/server/pg_extended_test.rs)::text_parameters_are_typed_and_reject_loss 明确断言 24:00:00 参数 is_err，不能沿用为 PG 目标。全部测试未运行，不证明完整往返、精度或准备元数据连续性。

[无时区时间戳契约](#无时区时间戳契约)静态取证基线为 `c8f463e422`（2026-10-23，批次 9 已提交）。经 RustCodeGraph 读取 [time.rs](../pkg/types/time.rs) 的 TimeContext、CoreTime 位域、Time::ConvertTimeZone/String：共享值是 MySQL 日历字段、类型标签与 Fsp；ConvertTimeZone 显式按 from/to 转换字段，不能证明 PG 无时区身份。Go 对照 [time_test.go](../pkg/types/time_test.go)::TestConvertTimeZone 有 UTC/Asia/Shanghai 相差八小时及零值保留断言；这不是 PG timestamp 的规则或本轮实测。两个目标包均无 doc.go。

入口 [pg_extended.rs](../pkg/server/pg_extended.rs)::parameter/temporal/binary_parameter/Extended::process：1114 text 经 chrono 固定 `%Y-%m-%d %H:%M:%S%.f`，限年 1–9999、整微秒，转 MySQL tp=12；binary 非 NULL 1114 未开放（0A000），NULL 提前转 tp=6。出口 [pg_result.rs](../pkg/server/pg_result.rs)::pg_type/value_text/value_binary/encode_formats 将 code=7/12 都映射 1114/8，typmod 固定 -1；text 仅粗查年月日，binary 经 chrono 解析后取相对 2000-01-01 的微秒。MySQL TIMESTAMP 上游会话转换是否已执行须另取端到端证据，不从该映射推断等价。

已读独立 [pg_types_test.rs](../pkg/server/pg_types_test.rs)::common_type_oids_and_values 的 DATETIME→1114/text、pg_binary_native_results 的 1999-12-31 23:59:59.999999→网络序 i64 -1 及 NULL 长度 -1；[pg_extended_test.rs](../pkg/server/pg_extended_test.rs)::text_parameters_are_typed_and_reject_loss 含六位小数接受、七位拒绝，后者不能作为 PG 舍入目标。所有测试未运行，不证明两格式往返、特殊值、全范围、会话隔离或 Describe 连续性。

[带时区时间戳契约](#带时区时间戳契约)静态基线为 `ba1af304d2`（2026-10-23，批次 10 无时区时间戳已提交）。经 RustCodeGraph 核对 [time.rs](../pkg/types/time.rs)::TimeContext/CoreTime/ConvertTimeZone：位域承载日历字段和 Fsp，转换要求显式 from/to 并重写字段；不能单独保留 PG 瞬时身份、全域或非有限值。[pg_session.rs](../pkg/server/pg_session.rs)::PgSession 当前字段为 path/failed_transaction，不能据此证明 PG TimeZone 状态已接通。两个包无 doc.go。

已读 [pg_extended.rs](../pkg/server/pg_extended.rs)::parameter/binary_parameter/temporal 和 [pg_result.rs](../pkg/server/pg_result.rs)::pg_type/value_text/value_binary：非 NULL OID 1184 参数两格式均无分支，返回 0A000；NULL 提前返回 tp=6，不证明 typed NULL 保真；结果 code=7/12 均映射 1114，没有独立 1184 身份。独立 [pg_extended_test.rs](../pkg/server/pg_extended_test.rs)::text_parameters_are_typed_and_reject_loss 仅断言 1114 六位成功、七位拒绝；[pg_types_test.rs](../pkg/server/pg_types_test.rs)::common_type_oids_and_values/pg_binary_native_results 的 DATETIME→1114、-1 微秒及 NULL 不证明 timestamptz。Go [time_test.go](../pkg/types/time_test.go)::TestConvertTimeZone 断言 UTC/Shanghai 八小时转换及零值保留，仅为原 MySQL 行为对照。以上测试均未运行。

[时间间隔契约](#时间间隔契约)静态基线为 `7ba687df06`（批次 11 已提交）。经 RustCodeGraph 核对 [time.rs](../pkg/types/time.rs)::Duration/ParseDurationValue/ExtractDurationValue：Duration 只存 i64 纳秒总量与 Fsp；ParseDurationValue 返回年/月/日/纳秒分量，但 ExtractDurationValue 将月按 30 天折入总量并应用 MySQL TIME 范围，不能保真承接 PG interval。[datum.rs](../pkg/types/datum.rs)::KindMysqlDuration 是 MySQL TIME 标签，没有由该标签证明的 PG interval 身份。[builtin_time.rs](../pkg/expression/builtin_time.rs) 使用 chrono Duration 与 MySQL TIME 微秒上限，也不证明日历间隔身份。目标三包均无 doc.go。

[pg_extended.rs](../pkg/server/pg_extended.rs)::parameter/binary_parameter 没有非 NULL OID 1186 分支，当前返回 0A000；NULL 提前转 tp=6 不证明 typed NULL 保真。[pg_result.rs](../pkg/server/pg_result.rs)::pg_type 将 code=11 映射为 1083/8，没有通用 interval 映射；不能把 MySQL TIME 的结果 OID 改成 1186 就声称支持。[time.go](../pkg/types/time.go)::ExtractDurationValue 也有 month*30 折算，这是 MySQL 对照，不是 PG 目标；本任务不修复或简化原 Go/Rust 行为。

已读独立 [time_11_aster_unit_test.rs](../pkg/types/time_11_aster_unit_test.rs) 的实际断言：ParseDurationValue 保留 DAY_MICROSECOND 的 day=12，ExtractDurationValue(MONTH,1) 输出 720:00:00，DAY=-35 拒绝；duration_parse_arithmetic_and_format_match_go 将负两天三小时折为 -51 小时。[pg_types_test.rs](../pkg/server/pg_types_test.rs)::common_type_oids_and_values 和 [pg_extended_test.rs](../pkg/server/pg_extended_test.rs)::parse_bind_execute_sync 提供其他类型与协议入口，不能证明 1186 支持。所有测试均未运行，本块专项用例均待新增。

[通用唯一标识契约](#通用唯一标识契约)静态基线为 `fda2b60876`（批次 12 已提交，2026-10-23）。经 RustCodeGraph 阅读 [datum.rs](../pkg/types/datum.rs)::Datum/Kind 常量，没有独立 UUID 标签；KindString/KindBytes 只证明通用载体。[field_type.rs](../pkg/types/field_type.rs)::InferParamTypeFromDatum/InferParamTypeFromUnderlyingValue/DefaultTypeForValue 按 MySQL 类型推导，String 为 TypeVarString、NULL 为 TypeNull，不从字符串外观保留 UUID 身份。目标包 pkg/types、pkg/server 均无 doc.go。

[pg_extended.rs](../pkg/server/pg_extended.rs)::parameter/binary_parameter 没有非 NULL OID 2950 分支，分别走 0A000；NULL 提前返回 tp=6，不能证明 typed NULL 保真。[pg_result.rs](../pkg/server/pg_result.rs)::pg_type/value_text/value_binary 没有通用 UUID 映射及 binary 编码；字符串/二进制列现有映射为 text/bytea，不构成 UUID 支持。已读独立 [pg_extended_test.rs](../pkg/server/pg_extended_test.rs)::text_parameters_are_typed_and_reject_loss（整数、bool、bytea、时间、decimal 与 NULL）及 [pg_types_test.rs](../pkg/server/pg_types_test.rs)::common_type_oids_and_values/unsigned_binary_and_unsupported_types（常见类型、bytea 与拒绝路径），这些断言未覆盖 UUID，且本轮均未执行。不改变原 Go/MySQL 字符串及 bytes 行为。

[JSON文本契约](#JSON文本契约)静态基线为 `c6807d8e195f20a13b3eb32178fc85c39dbc09b8`（批次13 UUID已提交，2026-10-23）。经仓库 RustCodeGraph 阅读 [json_binary.rs](../pkg/types/json_binary.rs)::ParseBinaryJSONFromString/UnmarshalJSON/from_serde_value/appendBinaryNumber/MarshalJSON/marshalObjTo：输入经 serde 值树与 BTreeMap，再编码 i64/u64/f64；不保留原始空白、重复成员及数字词法，对象按键重建。Number(String) 中间态不证明最终任意精度，超过整数域会走 f64。输出重新插入分隔空格并转义字符串，不是原文出口。最大深度100是现有MySQL限制，不能直接宣称为PG限制。

独立 [json_binary_test.rs](../pkg/types/json_binary_test.rs)::TestBinaryJSONMarshalUnmarshal 仅断言规范文本往返，TestMarshalFloatInContainer 断言浮点格式化，TestParseBinaryFromString 检查空文档/尾随内容；未见这些用例断言 PG 原文、重复键或超numeric大数保留。[builtin_json.rs](../pkg/expression/builtin_json.rs)::json_null/argument_value/json_unquote 表明函数特定的SQL NULL→JSON null及解引号逻辑，不是通用cast依据。[pg_extended.rs](../pkg/server/pg_extended.rs)::parameter/binary_parameter 无非NULL OID114分支，走0A000；[pg_result.rs](../pkg/server/pg_result.rs)::pg_type 无code245映射。[pg_types_test.rs](../pkg/server/pg_types_test.rs)::unsigned_binary_and_unsupported_types 断言JSON及NULL JSON结果拒绝。三个目标包无doc.go；所有测试均未运行，输入与输出证据均为静态阅读。

[JSONB规范值契约](#JSONB规范值契约)静态基线为 `49bc0d486d`（批次14 JSON文本块已提交，2026-10-23）。经 RustCodeGraph 核对 [json_binary.rs](../pkg/types/json_binary.rs)::ParseBinaryJSONFromString/UnmarshalJSON/appendBinaryNumber/appendBinaryObject/from_serde_value：MySQL BinaryJSON 使用 i64/u64/f64、BTreeMap 和自有类型码/长度表，支持 Opaque/Time/Duration 扩展；它不是 PG jsonb 规范值或 wire。数字字符串最终可能转 f64，对象字典序也不能证明 PG 输出键序一致，深度100/键长限制不能直接作为PG目标限制。沿用原 Go/MySQL 语义，不为 PG 改写这些函数。

独立 [json_binary_test.rs](../pkg/types/json_binary_test.rs)::TestBinaryJSONMarshalUnmarshal 已读断言嵌套、数组、null、浮点的 MySQL 规范文本往返，不证明重复键最后值、任意精度或PG binary版本兼容。[builtin_json.rs](../pkg/expression/builtin_json.rs)::json_null/argument_value 是函数特定 NULL 转换，不是通用 jsonb cast。[pg_extended.rs](../pkg/server/pg_extended.rs)::parameter/binary_parameter 无非NULL OID3802分支；[pg_result.rs](../pkg/server/pg_result.rs)::pg_type 未提供code245映射，独立 [pg_types_test.rs](../pkg/server/pg_types_test.rs)::unsigned_binary_and_unsupported_types 明确拒绝JSON及NULL JSON结果。三个目标包无doc.go；本轮仅静态阅读，未运行测试，以上均不能标作PG jsonb已支持。

[基础数组契约](#基础数组契约)静态基线为 `021e3ceeab`（批次15已提交，2026-10-23）。经 RustCodeGraph 阅读 [datum.rs](../pkg/types/datum.rs)::Datum 及 Kind 常量：现有标量盒、KindInterface、MySQL JSON 和 float32 vector 不提供通用 PG 数组的元素身份、形状与下界契约，不能以任意载荷存在推导支持。[pg_result.rs](../pkg/server/pg_result.rs)::CatalogColumnType/wire_type/pg_type/value_text/value_binary/binary_array/encode_formats 有目录专用数组 OID；binary_array 从目录文本生成一维载荷，普通数组下界固定1，vector下界0，拒绝嵌套，不能承接任意下界或多维 SQL 值。value_text 对文本直接输出，不构成通用数组校验器。

[pg_extended.rs](../pkg/server/pg_extended.rs)::Extended::process 的普通 Parse 白名单不含数组OID；parameter/binary_parameter 没有非NULL通用数组解码，NULL参数先转code6也不证明typed NULL身份保真。已读独立 [pg_types_test.rs](../pkg/server/pg_types_test.rs)::pg_binary_native_results 的目录空int2数组（ndim=0、元素OID21）及int2vector（下界0）字节断言、catalog_char_and_acl_wire_types 的ACL NULL/OID断言；后者不是ACL非NULL二进制或普通数组支持证据。[pg_oid_test.rs](../pkg/server/pg_oid_test.rs)::pg_introspection_oid_ranges_and_local_indexes 只断言目录对象OID区间/唯一性，未覆盖元素/数组OID或SQL数组往返。目标包无doc.go；这些测试均未运行，本轮仅静态取证。

[显式隐式转换契约](#显式隐式转换契约)静态基线为 `1ecb906fd0`（批次16基础数组已交付，2026-10-23）。经 RustCodeGraph 阅读 [builtin_cast.rs](../pkg/expression/builtin_cast.rs)::BuildCastFunctionWithCheck/cast_value：目标元数据在构建时固定，输入 NULL 在求值时直接返回 NULL；数组检查为 MySQL JSON 到 typed array 的局部路径，不是 PG 数组转换。其 FieldKind、CastContext 和 in_union 不能代替 PG 类型身份及三种转换上下文。[datum.rs](../pkg/types/datum.rs)::ConvertTo 按 MySQL Type 分派且 NULL 直接返回默认 Datum；[convert.rs](../pkg/types/convert.rs)::handled_truncate/ConvertIntToInt/ConvertFloatToInt 使用 Context 截断策略及带裁剪值的错误，不能直接作 PG 错误后仍返回结果的依据。[pg_extended.rs](../pkg/server/pg_extended.rs)::Extended::process 在普通 Parse 将未指定 OID 补0后明确以0A000拒绝；Bind依据固定OID选择parameter/binary_parameter，格式码不参与SQL转换资格。

已读独立 [builtin_cast_test.rs](../pkg/expression/builtin_cast_test.rs) 的挂载入口及 [builtin_cast_4_aster_unit_test.rs](../pkg/expression/builtin_cast_4_aster_unit_test.rs)::cast_numeric_matrix_matches_go_rounding_and_unsigned_rules、decimal_cast_rounds_and_clamps_to_declared_precision、json_conversion_preserves_parse_flag_and_typed_array_semantics：分别断言负数转unsigned告警、科学计数法前缀截断、decimal溢出钳制和JSON数组转换。[convert_test.rs](../pkg/types/convert_test.rs)::test_str_to_num 明确断言忽略截断时“11xx”→11、告警模式空串→0；这些是需保留的Go/MySQL意图，不是PG证据。[pg_extended_test.rs](../pkg/server/pg_extended_test.rs)::parse_bind_execute_sync 已读目录专用Parse/Bind/Describe局部断言，不能据此推导通用参数推断或转换矩阵覆盖。pkg/expression、pkg/types、pkg/server均无doc.go；所有测试只读未运行，PG转换资格、共同类型和SQLSTATE均未验证。

[准备描述执行元数据契约](#准备描述执行元数据契约)静态基线为 `d42b55c369`（批次17已提交）。通过 RustCodeGraph 已读 [conn.rs](../pkg/server/conn.rs)::PreparedMetadata/QueryResult、[pg_extended.rs](../pkg/server/pg_extended.rs)::Statement/Portal/Extended::process/description/description_formats，以及 [pg_result.rs](../pkg/server/pg_result.rs)::encode_formats；pkg/server/doc.go 不存在。PreparedMetadata 只有参数数量而无推定参数类型；PG Statement 另存 oids/mapping。普通 Parse 对 OID 0 或未提供的类型返回0A000；目录专用推断不能证明通用参数表达式已支持。Bind 保存独立列、native_types、description及formats；Describe 不调用执行闭包；Execute 编码后比对准备描述，不一致返回0A000，并移除内部生成的RowDescription后发结果行。无参数且执行缺native_types时有按列数回填准备元数据的限定分支，不代表参数化投影可同样回填。encode_formats核对类型数量及行宽，但统一typmod=-1、来源表/属性为0，仍缺完整规范身份及修饰符保真。

已读独立 [pg_extended_test.rs](../pkg/server/pg_extended_test.rs)::parse_bind_execute_sync 中目录零行两种Describe、显式23参数、多值/NULL重绑定、重复乱序参数和缺OID拒绝断言；[pg_types_test.rs](../pkg/server/pg_types_test.rs)::missing_original_metadata_is_explicitly_unsupported、unsigned_binary_and_unsupported_types、pg_binary_native_results 分别提供缺原类型、数量冲突及局部binary/NULL断言意图。测试均未运行；现有片段未证明通用参数化表达式、全部typmod、DDL后类型变化或无查询探测的全链验收。

### 目标行为

#### 规范类型与协议映射

本块建立方向契约，不把 MySQL 类型名、PG OID 和规范引擎类型视为同一身份。这里“规范类型”指执行与准备阶段保留的类型身份、修饰符和值域约束，并不声称现有 `NativeType` 已完整表达所有 PG 类型。值相同仍可属于 bool、int2、int4、int8 或 numeric；类型描述必须先于行内容确定。双向映射允许提升或拒绝，不承诺可逆。

方向与共同约束如下；下表 NULL、错误和元数据规则适用于后续每个类型族。

| 方向 | 类型身份、值域与精度 | NULL 与错误 | wire 元数据、现状与责任 |
| --- | --- | --- | --- |
| PG 参数 → 规范值 | 先按 Parse 声明 OID、SQL 目标和上下文推断确定参数身份，再按 Bind 格式解码；OID 0 表示尚未指定，不能当 text 或零值。保留参数序号与重复引用的同一身份，检查目标值域/精度。 | 参数长度 -1 是 SQL NULL，不解码值但仍校验类型；空载荷不是 NULL。类型无法确定须拒绝，非法文本/二进制与溢出分开。 | ParameterDescription 返回确定的 OID，Bind 格式 0/1 只决定编码，不决定类型；没有参数 typmod 字段，SQL 目标约束仍须保留。现有 parameter 将若干 OID 转成 BinaryParam，NULL 仅留下 tp=6；binary_parameter 支持集合更小。类型保真由本章主责，消息与恢复交工作流 7。 |
| MySQL 参数 → 规范值 | 按自身参数类型及 unsigned 等标志解码；这些输入不是 PG OID。保留原 MySQL 域，不从 PG 反向猜测 unsigned 或 bool。 | SQL NULL 保留空值及独立类型信息；错误遵循 MySQL 原契约，不能套 PG SQLSTATE。 | conn/conn_stmt 的既有 MySQL 参数边界继续拥有 wire 解释；本块只约束共享桥接不得丢失信息，未验证全链参数保真。 |
| 解析/推导 → 规范类型 | 消费第 2 章保留的类型名、修饰符、数组结构及上下文；共同类型和 cast 决策先按方言确定，再复用 FieldType 能保真的部分。 | 有类型 NULL 沿用声明；无类型字面量/参数依 PG 上下文解析，不能一律映射 code=6 的 text 回退。缺身份拒绝，不读首行补全。 | Prepare 与执行输出应携带同一类型契约；现有 PreparedMetadata 只有结果 NativeType 与 parameter_count，未证明参数身份和所有 PG 修饰符保真。类型推导属本章，运算重载协作工作流 4。 |
| 规范类型/值 → PG 结果 | 先生成每列 OID、typlen、typmod，再编码每个值；同列不因数值大小、首行/后续行、NULL 或格式而改变类型。 | SQL NULL 的 DataRow 长度 -1、无载荷，仍有列类型；未知类型即使无行或全 NULL 也拒绝。值域不符不得截断、转 text 或改 OID 掩盖错误。 | pg_type 已按元数据选 OID；encode_formats 校验列/native 数量，当前 typmod 恒 -1，来源表/属性号为 0。本章给类型约束，工作流 7 负责 Describe/Execute 与错误传输及恢复。 |
| 规范类型/值 → MySQL 结果 | 保持 MySQL code、flags、charset、length、decimals 的原契约；PG 类型无保真对应时须有显式转换或拒绝。 | NULL 不改变列定义；不因 PG 映射扩大或缩减 MySQL 原域。 | ColumnInfo 与现有 MySQL 编码继续拥有 wire 描述；PG OID/typlen 不写入 MySQL code/length。PG 专属类型的 MySQL 暴露策略未取证，不能默认为字符串。 |

下表是**目标方向映射**，括号内为当前 PG 出口的局部事实；MySQL 一列为共享已有类型的承载约束，不宣称 PG 专属类型已有 MySQL 等价物。U 表示 UnsignedFlag，B 表示 IsBooleanFlag；字符集二进制身份须单独保留，不能用值是否可打印决定。每行 PG 三元组是 OID/typlen/typmod；typlen 是 PG 类型内部长度标识，既非实际 wire 载荷长度，也非 MySQL 显示宽度。无修饰符用 -1；有修饰符必须来自定型结果。

| 规范身份/保真要求 | MySQL wire 方向（输入/输出） | PG 方向（输入身份 → 结果三元组） | 损失、拒绝与当前缺口 |
| --- | --- | --- | --- |
| boolean，独立于整数 0/1 | 保留内核 B；既有 MySQL 列可与整数相同，不强造 wire bool 身份 | bool 16 → 16/1/-1 | 仅凭 TINYINT 或值 1 不能判 bool；现有 pg_type 优先 B，参数 bool 到 tp=1 后的身份连续性未证明。 |
| 有符号整数，精确位宽 | TINY=1、SHORT=2、LONG=3、INT24=9、LONGLONG=8，U 清除；不要把显示宽度当位数 | int2 21、int4 23、int8 20 → 分别 21/2/-1、23/4/-1、20/8/-1 | PG 域为有符号 16/32/64 位。现有 tiny→int2、short→int2、long/int24→int4、longlong→int8；PG 声明 int2 输入不能因值小变 tiny 身份。越界拒绝。 |
| 无符号整数，完整原域 | 同上 code + U，值不能先窄化到 i64 | 无 PG unsigned 输入类型；共享结果 U8/U16/U24/U32/U64 → int2/int4/int4/int8/numeric | 对应结果 21/2/-1、23/4/-1、23/4/-1、20/8/-1、1700/-1/-1；最大 u64 必须精确保留 18446744073709551615。当前有 tiny/short/long/longlong 提升分支；不从结果 numeric 反推出 unsigned，反向须有声明及范围校验。 |
| exact numeric，精度/有符号 scale | DECIMAL=0 或 NEWDECIMAL=246，length/decimal 只在语义相容时复用 | numeric 1700 → 1700/-1/定型 typmod；无约束为 -1 | 不经 f64；不能把 MySQL decimal 上限当 PG 上限。当前文本参数用 MyDecimal 校验，binary 参数未列入支持分支；结果 typmod 丢失。负 scale、特殊值与超出共享表示的域须独立取证，不能靠映射宣称覆盖。 |
| float4 / float8，各自精度 | FLOAT=4 / DOUBLE=5，保留位宽 | 700 → 700/4/-1；701 → 701/8/-1 | 不能以当前值可表示为由降精度；NaN、无穷、负零及溢出按族契约取证。现有参数/结果有分支，不代表特殊值行为已验收。 |
| text / varchar(n) / bpchar(n)，保留字符长度与身份 | VARCHAR=15、VAR_STRING=253、STRING=254 及文本 blob 家族，沿自身 charset/flags；PG char 不能等同内部目录 char | 25 → 25/-1/-1；1043、1042 → 各自 OID/-1/(n+4)，无约束 -1 | PG 长度为字符约束，不取 MySQL wire 字节宽度直接代入。当前普通文本出口折为 text；入口 25/1043/1042 同转 tp=253，声明身份/补空格语义未证明。非法编码和不允许的 NUL 拒绝。 |
| bytea，原始字节 | 二进制 charset 与 blob/string 家族；不能与同载荷 text 合并 | 17 → 17/-1/-1 | 可含零字节；文本转义与 binary 载荷分开验收。现有参数文本只收 hex，binary 可收原字节；结果按 charset=63 选 bytea，此信息不在 NativeType 内，桥接仍依赖 ColumnInfo。 |
| date，日历日期 | DATE=10 / NEWDATE=14，不能用零日期冒充 PG 日期 | 1082 → 1082/4/-1 | 共享范围之外及 MySQL 零日期须明确拒绝；现有结果有映射，参数 temporal 限制引擎日期域，不证明 PG 完整日期域或 infinity。 |
| time，日内时间，秒小数精度 | TIME=11 的 duration 域须与日内时间区分，非无条件双向 | 1083 → 1083/8/p；未声明 p 为 -1 | p 为 0–6。负 duration、25:00:00 不能无损映射日内时间，拒绝或走显式转换；24:00 边界独立取证。当前 code 11 选 time，不证明域已相同。 |
| timestamp，无时区墙钟时间 | DATETIME=12 / TIMESTAMP=7 不能仅凭名称判为 PG 瞬间 | 1114 → 1114/8/p；未声明 -1 | 当前 7/12 都映射 1114；精度、零日期及范围须校验，不能添造时区。MySQL TIMESTAMP 的会话处理不能证明 PG timestamp 等价。 |
| timestamptz，瞬间身份 | 现有 MySQL code 无独立保真对应；须明确转换与会话前提，禁止自动借 7 | 1184 → 1184/8/p；未声明 -1 | 不要求保留原输入时区拼写，但必须保留瞬间，显示依会话时区；现有 NativeType 四字段及 pg_type 没有此身份分支。缺信息拒绝，不冒充 1114。 |
| interval，月/日/时间分量 | 无直接 MySQL duration 等价；须显式转换或拒绝 | 1186 → 1186/16/字段掩码及 p 的类型修饰符；无约束 -1 | 月不折固定秒数；不能仅靠 TIME code 保真。当前通用出口未见分支；字段掩码/typmod 精确规则交 interval 块取官方源码证据，开放受限类型前必须补齐。 |
| UUID，128 位身份 | 无专属既有 wire code；binary/text 仅可作显式转换的结果 | 2950 → 2950/16/-1 | 不从字符串形状或 16 字节反推 UUID；缺规范身份或非法输入拒绝。当前通用 pg_type 未见 UUID 分支。 |
| JSON / JSONB，彼此独立身份 | JSON=245 不能证明同时保留两者语义；文本转换必须显式 | 114 → 114/-1/-1；3802 → 3802/-1/-1 | JSON 原文保真与 JSONB 规范化不可互换，JSON null 不是 SQL NULL；现有 code 245 出口明确拒绝，NULL 也不能绕过。参数/结果 codec 与范围交各族文档，不能用 text OID 假成功。 |
| 基础数组，元素类型、维度、下界及元素 NULL | 无 MySQL 通用数组 wire 等价，不能把字符串花括号视为规范数组 | 由元素身份确定独立数组 OID（int4[] 示例 1007），typlen=-1；typmod 承接元素修饰，不代表维度 | 数组 NULL、空数组、含 NULL 元素分开；必须保留形状/下界，不从首元素猜类型。当前 CatalogColumnType 有目录专用数组/vector 与编码，不证明任意表达式/参数数组支持。其他族数组 OID 在各族开放前核对官方目录。 |

numeric typmod 目标按官方 `make_numeric_typmod` 的精度高 16 位、scale 低 11 位编码并加 4，scale 的符号扩展按 `numeric_typmod_scale`，不能直接套 MySQL decimal 字段。字符 typmod 的 n+4 来自 `anychar_typmodin`。时间族与 interval 的精确修饰符、特殊域及各族 binary 布局在相应文档块取证；本块不发明格式或将其标为现有实现。未知引擎 code、缺必要规范字段和仅有目录专用身份，目标为显式不支持路径；不静默降级 text，不通过读取首行“恢复”身份。

方案比较：只扩展 pg_type/OID 表对整数提升等既有域可复用，但不能恢复已折叠的 bool 参数、varchar/bpchar、timestamptz、JSON/JSONB、数组身份与 typmod；已有 NativeType boolean 测试也表明相同 MySQL 列和值不足以判型。因此选择**复用 NativeType/FieldType 桥接并按必要保真缺口补充规范类型信息**，再由各 wire 适配。这里不命名新 API 或重建完整类型系统；后续先做独立静态/最小往返取证，确定字段、参数声明生命周期和共享消费者，缺证项不开放。方案不改变 MySQL 原有聚合、cast 和错误语义。

官方依据（2026-10-23 查阅）：PostgreSQL **18** §54.7 [Message Formats](https://www.postgresql.org/docs/18/protocol-message-formats.html) 的 Parse、Bind、ParameterDescription、RowDescription、DataRow 支持方向、格式与 NULL 约束；§52.64 [pg_type](https://www.postgresql.org/docs/18/catalog-pg-type.html) 支持 typlen/类型身份含义。OID/typlen 数值核对官方 **REL_18_0** [pg_type.dat](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_type.dat)；typmod 核对同 tag 的 [numeric.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/numeric.c) 与 [varchar.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/varchar.c)。§8.1 [Numeric Types](https://www.postgresql.org/docs/18/datatype-numeric.html)、§8.5 [Date/Time Types](https://www.postgresql.org/docs/18/datatype-datetime.html)、§8.14 [JSON Types](https://www.postgresql.org/docs/18/datatype-json.html) 分别支持位宽/精度、时间域区别、JSON 身份区别；[第 8 章](https://www.postgresql.org/docs/18/datatype.html)作为其他族取证索引，非完整 codec 依据。18.6 仍是来源目标；未运行该补丁参考端，REL_18_0 源码不能代替其运行证据。

#### 布尔类型契约

规范 bool 的非 NULL 值域只有 true/false，SQL NULL 表示未知且保留 bool 身份；不存在精度、scale 或长度修饰符（不适用）。布尔列、TRUE/FALSE 字面量与已定型参数都必须保留此身份，不能由整数 0/1、TINYINT 名称或 MySQL Boolean flag 单独推定 PostgreSQL 列语义。现有 Datum 无独立 bool Kind，出口用 IsBooleanFlag 选 OID 16；这只是承载手段，尚未证明声明列和参数的完整身份生命周期。

| 方向 | 目标身份、值与 NULL | 输入输出、错误及 wire | 现状与差距 |
| --- | --- | --- | --- |
| 字面量/列/参数推导 → 规范 bool | TRUE/FALSE 直接定型 bool；NULL::boolean、布尔目标列中的 NULL 保留类型；未知字符串仅在 bool 上下文按 bool 输入解析。Parse OID 16 或 SQL 布尔目标确定参数，OID 0 按上下文推断，无法推断为 42P18。 | ParameterDescription 为 16；结果 OID/typlen/typmod 为 16/1/-1。零行、首行 NULL、后续 NULL 均不改变描述。裸 NULL 不能无条件定型 bool。 | Extended::process 当前对缺参数推导拒绝；parameter 非 NULL bool 转 tp=1，NULL 转 tp=6，不能据此认定仍携带 bool。声明列持久化、Prepare/Execute 保真未验证；第 2 章提供语法/目标身份，本章定型。 |
| PG text 参数/显式文本输入 → bool | 合法表示包括 true/false、yes/no、on/off、1/0，忽略大小写与两端空白；允许唯一前缀如 t、tr、n、of，o 不唯一应拒绝。SQL NULL 用长度 -1，空串不是 NULL。 | 非法文本如空串、o、2、truth、unknown 为 22P02；非法编码目标 22021。以 OID 16 解释，不能先走数值真值转换。 | parameter 当前仅接受大小写不敏感的 t/true/1、f/false/0，未 trim；其他合法拼写缺失。现有测试不覆盖完整词集或有类型 NULL。 |
| PG binary 参数 → bool | 非 NULL 载荷恰好一字节，00 为 false，01–ff 均为 true；这是已声明 OID 16 的接收规则，不授权整数隐式转 bool。NULL 不消费载荷并保留类型。 | boolrecv 接收后规范化；空载荷目标 08P01，多字节剩余数据目标 22P03；长度 -1 为 NULL，其他负长度和不完整 Bind 归工作流 7。单字节没有非法“值”；把 02/ff 当非法值会偏离 PG。 | binary_parameter 当前只收 00/01，其他值/长度均 22P03；目录专用 catalog_binary_parameter 也有限制，不能推为通用支持。上述差距只记录，未修代码。 |
| 规范 bool → PG text/binary 结果 | 非 NULL text 为 t/f，binary 为 00/01；NULL DataRow 长度 -1、无载荷。先校验身份和值域，非法引擎值如带 bool 身份的整数 2 应拒绝，不能掩盖成 true。 | Describe 与执行结果同为 16/1/-1，格式码依 Bind，格式不改 OID。缺类型/非法内部值走明确错误路径（内部不变量目标 XX000），不静默降级 text；SQLSTATE 传输由工作流 7 承接。 | pg_type 使用 B，value_text 只收 1/true/0/false，value_binary 复用文本校验后发单字节；出口 io::Error 不能证明 XX000 已正确传输。完整 bool 列、NULL 与 Describe 一致性未验证。 |
| MySQL 输入/输出 ↔ 共享承载 | 保留原 code/flags、数值域与真值规则；MySQL Boolean flag、逻辑 bool 与整数三者分别记录。PG 布尔类型信息不能把 MySQL TINYINT 全域缩成 bool。 | 沿自身 wire 契约，无 PG OID/SQLSTATE（不适用）；共享 NULL 不改列定义。 | Datum::ToBool 可处理非零数等，保留其 Go/MySQL 契约；不能把它直接接为 PG 隐式转换。现有同列同值测试只证明桥接断言意图，双方言运行回归未验证。 |

转换边界先按方言与上下文决策再复用共享实现：PG 不允许整数直接充当 WHERE 条件（例如 WHERE 1 目标 42804），整数赋给 boolean 列也不能自动走真值转换（类型不匹配目标 42804）。显式 int4→bool 则合法，0→false、非零（含 -1、2）→true；bool→int4 为 false→0、true→1，结果 OID 23。不得把该显式规则泛化为任意整数类型、隐式重载或赋值转换。显式 text→bool 按输入规则，bool→text 的 SQL cast 输出 true/false（OID 25），区别于 bool wire text 的 t/f；有类型 NULL 的这些转换返回目标类型 NULL。其他来源类型的 cast 由后续转换块逐项取证，不套 MySQL 宽松转换；运算符重载及三值逻辑运算归工作流 4。

官方依据（2026-10-23）：PostgreSQL **18** §8.6 [Boolean Type](https://www.postgresql.org/docs/18/datatype-boolean.html) 支持词集、唯一前缀、空白/大小写、字面量与 NULL 推导及 t/f 输出。精确 codec 核对官方 **REL_18_0** [bool.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/bool.c) 的 boolin/boolrecv/boolsend/booltext；长度错误分别核对 [pqformat.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/libpq/pqformat.c) 的 pq_getmsgbyte 与 [postgres.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/tcop/postgres.c) 的 exec_bind_message 剩余载荷检查。显式转换资格核对同 tag [pg_cast.dat](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_cast.dat) 的 int4/bool castcontext=e，值规则核对 [int.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/int.c) 的 int4_bool/bool_int4；无隐式/赋值转换的拒绝目标是据此推导，尚未运行参考端。wire NULL 与描述字段沿用前块 §54.7 依据；18.6 补丁参考端仍未验证。

#### 整数与无符号边界

本块固定精确整数域；整数没有 scale，PG int2/int4/int8 无长度或精度 typmod（-1），十进制显示位数不是声明精度。共享承载必须同时保留声明宽度和 signed/unsigned，不能由当前值是否为正推断。PG 三种常规 SQL 整数均为有符号；目录 oid 的 unsigned 32 位编码不构成通用 unsigned 整数类型。以下为 MySQL 原整数域及 PG 出口目标（不含 Boolean flag 特例）：

| MySQL 身份/code | signed 值域 | unsigned 值域 | 按声明域选择 PG OID/typlen/typmod |
| --- | --- | --- | --- |
| TINYINT/1 | -128…127 | 0…255 | 两者 int2：21/2/-1 |
| SMALLINT/2 | -32768…32767 | 0…65535 | signed int2：21/2/-1；unsigned int4：23/4/-1 |
| MEDIUMINT/9 | -8388608…8388607 | 0…16777215 | 两者 int4：23/4/-1 |
| INT/3 | -2147483648…2147483647 | 0…4294967295 | signed int4：23/4/-1；unsigned int8：20/8/-1 |
| BIGINT/8 | -9223372036854775808…9223372036854775807 | 0…18446744073709551615 | signed int8：20/8/-1；unsigned numeric：1700/-1/-1 |

此表扩宽与 pg_type 当前分支一致，u64 目标选无约束 numeric，不擅自改为 numeric(20,0)。值域保真不等于来源类型可逆：PG int4 不能反推原 MySQL U16，PG numeric 不能反推 U64。反向必须有目标 FieldType/unsigned 身份；缺声明拒绝或保留原规范身份，不从 OID 猜源类型。MySQL 自身输出保留原 code/UnsignedFlag，不因 PG 扩宽改写 MySQL 列定义。

| 方向 | 身份、精确值与 NULL | 错误、wire 及现状差距 |
| --- | --- | --- |
| PG 声明/上下文 → 规范整数及参数描述 | smallint/int2、integer/int/int4、bigint/int8 分别严格为上述 16/32/64 位 signed 全域。参数显式 OID 21/23/20 或上下文定型；OID 0 无法推断目标 42P18。NULL 保留声明宽度而非归 text。 | ParameterDescription 保持输入 OID；结果分别 21/2/-1、23/4/-1、20/8/-1。当前 process 对缺推导报 0A000；parameter(NULL) 为 tp=6，类型生命周期未验证。第 2 章交接类型语法，本章定型。 |
| PG text 参数/显式文本转换 → 规范整数 | 按已定型 signed 域精确解析，覆盖正负号、首尾空白及 PG 整数输入语法；NULL 长度 -1，空串不是 NULL。整数字符串不得经 f64 中转或接受截断数字前缀。 | 合法整数越界为 22003，非法如空串、1x、1.5 为 22P02，非法字符编码 22021。现有 parameter 用 Rust parse i16/i32/i64，将范围错误也映射 22P02，未 trim，不能证明完整 PG 输入语法。PG 18 的进制前缀及下划线规则以 numutils.c 为准，后续补输入用例，不能只凭 Rust parse 推定等价。 |
| PG binary 参数 → 规范整数 | OID 21/23/20 分别恰好 2/4/8 字节网络序、有符号二补码；每个合法长度的位模式都在该整数域内。长度 -1 为 typed NULL，无载荷。 | 接收器读不足目标 08P01；多余载荷目标 22P03，外层 Bind 帧损坏归工作流 7。当前 binary_parameter 将错误长度统一报 22P03，再转共享 little-endian 参数；不存在合法长度的“数值越界位模式”。 |
| 规范整数/MySQL unsigned → PG text/binary 结果 | 先按声明域定型，再按上表整体扩宽；U64 即使值为 0 也保持 numeric。text 输出精确十进制整数；binary int2/int4/int8 按 2/4/8 字节网络序。NULL 为 DataRow 长度 -1。 | value_text 当前直接格式化整数，未按 OID 再检查整数范围；binary 用 parse 检查范围。u64::MAX 的 text/OID 已有断言意图，numeric binary 当前无分支，不能把十进制 ASCII 填进 numeric binary。开放该格式前依赖 numeric 块补齐 codec；未实现路径明确拒绝目标 0A000，禁止回退 float8 或伪装 int8。 |
| PG/共享整数 → 显式窄化、目标 unsigned | 整数之间只在目标域内精确转换；负值转共享 unsigned 目标拒绝，超过 unsigned 上界同样拒绝；这是桥接契约，PG 没有可写作 ::unsigned 的常规整数类型。numeric→U64 仅在有限、无小数且域内的明确桥接路径接收；一般 numeric→PG integer 舍入规则留 numeric/cast 块，不把此桥接规则当 PG 通用 cast。typed NULL 转为目标类型 NULL。 | PG 路径域外目标 22003，不 wrap、饱和或返回截断成功。MySQL ConvertIntToUint 的 AllowNegativeToUnsigned、携带裁剪值的错误和 Context 策略保持原 Go/MySQL 行为，不直接当 PG 转换。共享错误到 SQLSTATE 尚未验证。 |

算术和转换结果的身份必须在 Prepare/Describe 时由已选择的 PG 重载/目标类型确定，执行时不能因某一行溢出而临时提升 OID；例如 int4+int4 的 int4 溢出应 22003，而预先显式转 int8 后计算可得到 int8，不能直接套共享 MySQL 的 BIGINT 推导。`2147483647::int4 + 1::int4`、bigint 最大值加一及各 signed 最小值取负均应有溢出验收；NULL 结果保留同一结果类型。unsigned 来源的预定扩宽是上表的边界策略，不授权改变 PG 算术规则。运算符/聚合完整重载归工作流 4，本块只规定结果类型交接与溢出不改型；显式、赋值、隐式 cast 的资格按后续转换块确定，wire 扩宽不能自动成为 SQL 隐式 cast。

官方依据（2026-10-23）：PostgreSQL **18** §8.1.1 [Integer Types](https://www.postgresql.org/docs/18/datatype-numeric.html#DATATYPE-INT) 定义三种 signed 域及越界错误；§54.7 [Message Formats](https://www.postgresql.org/docs/18/protocol-message-formats.html) 定义 NULL、参数 OID 与列描述。精确输入和 codec 核对官方 **REL_18_0** [int.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/int.c) 的 int2in/int4in、recv/send、int4pl、int2um/int4um，以及 [int8.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/int8.c) 的 int8in/recv/send、int8pl/int8um 和整型窄化；[numutils.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/numutils.c) 的 pg_strtoint16_safe/32_safe/64_safe 区分语法和范围错误；[pqformat.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/libpq/pqformat.c) 与 [postgres.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/tcop/postgres.c) 的读取/剩余载荷检查支持二进制长度错误目标。错误名称见 PostgreSQL 18 [附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html)。OID 沿前块 pg_type.dat 依据；MySQL 域来自本仓库上下界函数及 test_convert，不以 MySQL 类型名推导 PG 等价。18.6 仍仅为来源目标，未运行该补丁参考端。

#### 精确数值契约

numeric 与 decimal 是 PG 同一精确数值类型（OID 1700、typlen -1），区别于 float4/float8。目标须保存有限精确值、显示 scale、声明 precision/有符号 scale 及特殊值身份；只扩充 OID 或透传字符串不足以证明支持。现有 MyDecimal 与 cast 的 Decimal 容量/错误策略不同，须先验证可保真子域，再决定规范表示补充；不得将 PG 域缩成 MySQL DECIMAL 或用 f64 中转。尚不能承载的合法 PG 域应明确报项目不支持 0A000，记录兼容缺口；PG 自身域外才是 22003，不能混淆。

| 方向 | 值、类型、NULL 与精度目标 | 错误、wire 元数据及当前差距 |
| --- | --- | --- |
| PG 声明/推导 → 规范身份 | numeric(p,s) 的 p=1…1000，s=-1000…1000，可 s>p；numeric(p) 为 s=0；裸 numeric 不强制 scale，有限域上限为小数点前 131072 位、后 16383 位。NULL 保留目标 numeric 身份。 | 无约束 typmod=-1；有约束为 `((p<<16) | (s & 0x7ff))+4`，解码 s 须 11 位符号扩展。非法声明目标 22023。ParameterDescription 仅含 OID，不传 p/s；上下文及结果描述保存修饰符。现有结果一律 -1，不能伪称 constrained numeric。声明语法交工作流 2。 |
| text 参数/文本显式转换 → numeric | OID 1700 下完整解析十进制与指数输入，允许规范所容许的符号/空白；空串、12bad 非法，不能接受数字前缀或回退零。无约束 12.340 保留显示 scale；有约束先舍入再检查整数域。NULL 长度 -1，无载荷；空载荷不是 NULL。 | 非法语法 22P02，非法字符编码 22021，数值范围/舍入后超域 22003。parameter 当前调用 FromString 后保留原字节，所有解析错误映为 22P02；不是完整 PG 输入/范围分类证据。 |
| binary 参数 → numeric | 网络序 16 位字段依次为 ndigits（unsigned 计数）、weight（signed）、sign、dscale，后随 ndigits 个 base-10000 digit（0…9999）；有限值为符号乘各 digit×10000^(weight-i)。长度应为 8+2×ndigits；NULL 在外层长度 -1 表示。 | sign 为 0000/4000/C000/D000/F000（正/负/NaN/+∞/-∞），dscale 为 0…16383；它是显示 scale，不是带符号的声明 scale。接收有限数先截去 dscale 隐藏位，再应用 typmod 舍入/范围。非法 sign/digit/scale 22P03；读取不足 08P01，额外载荷 22P03（沿前块消息边界）。当前非 NULL 1700 无 binary 分支，报 0A000；NULL 能走公共分支不证明身份连续。 |
| 规范 numeric/MySQL DECIMAL/U64 → PG text/binary 结果 | Decimal 原身份保留，U64 沿前块输出无约束 numeric；text 精确输出十进制及应保留的小数零，binary 转换 base-10^9 到 base-10000，不能发送 MyDecimal::ToBin 格式或 ASCII。两格式解码后的值与显示 scale 一致，不要求载荷相同。NULL 为 DataRow 长度 -1。 | OID 1700/typlen -1，typmod 随声明或表达式定型，text/binary 不改变类型；零行、全 NULL 同样 Describe。value_text 当前透传值文本，未校验 numeric 域；value_binary 无 1700；encode_formats 固定 typmod -1。未开放 codec 明确拒绝目标 0A000，不降为 float8/int8，io::Error 到 SQLSTATE 尚未验证。 |
| numeric → 目标精度/整数/共享 DECIMAL | precision/scale coercion 的半值远离零；负 scale 在小数点左舍入，s>p 限制小数点后前导零。numeric→PG signed integer 先按整数舍入再检查对应域；整数→numeric 精确保值。typed NULL 转为目标类型 NULL。 | 正负超域均 22003，不裁剪、wrap 或仅警告成功；如 99.95/-99.95→numeric(3,1) 均因进位超域失败。截断只用于显式截断语义或 binary 接收 dscale 规则，不能代替 cast 舍入。共享 unsigned 的有限无小数桥接仍沿前块；MySQL warning/裁剪原行为独立保留。显式/赋值/隐式资格交本章转换块，运算重载交工作流 4。 |

特殊值单列：目标规范身份须区分 NaN、Infinity、-Infinity 与 SQL NULL；NaN 可进入 constrained numeric，无穷只可进入无约束 numeric，进入有限精度声明目标 22003。输出 text 使用 NaN/Infinity/-Infinity，binary 使用上述专属 sign。MyDecimal 的符号与有限 word 布局没有特殊值标记；Value::Float 的特殊值输出分支或 Value::Text 透传不能证明 numeric 特殊值已可表示、转换或求值。支持前明确 0A000，禁止把它当普通字符串成功；特殊值比较/算术由工作流 4 承接。

官方依据（2026-10-23）：PostgreSQL **18** §8.1.2 [Arbitrary Precision Numbers](https://www.postgresql.org/docs/18/datatype-numeric.html#DATATYPE-NUMERIC-DECIMAL) 支持精度、scale、舍入和特殊值目标。精确布局、typmod、错误及转换以官方 **REL_18_0** [numeric.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/numeric.c) 的 make_numeric_typmod/numeric_typmod_scale、numerictypmodin、numeric_in/out、numeric_recv/send、apply_typmod/apply_typmod_special、numeric_int4 为据；NULL/描述与外层载荷沿前块 §54.7、pqformat.c/postgres.c 来源。以上是规范和源码阅读证据，非参考端实测；来源指定的 18.6 补丁仍未验证。

#### 浮点数值契约

real/float4 与 double precision/float8 是两个规范身份，目标分别为 IEEE 754 binary32/binary64（有效二进制精度 24/53 位），PG OID/typlen/typmod 分别 700/4/-1、701/8/-1。有限最大绝对值为 (2−2^-23)×2^127、(2−2^-52)×2^1023；最小正规正数为 2^-126、2^-1022，最小次正规正数为 2^-149、2^-1074。这些是二进制域边界，不以文档概述的十进制量级代替；近似类型不能承诺十进制精确计算。[PostgreSQL 18 §8.1.3](https://www.postgresql.org/docs/18/datatype-numeric.html#DATATYPE-FLOAT) 为位宽、精度、特殊值和溢出目标依据。

推导消费工作流 2 的类型节点：real/float4 固定为 float4，double precision/float8 为 float8；float(p) 的 p 为二进制精度，1…24 选择 real、25…53 选择 double precision，无 p 选择 double precision。p=0 或 p=54 由声明解析拒绝 22023（REL_18_0 [gram.y::opt_float](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/parser/gram.y)），不把 MySQL FLOAT(M,D) 当 PG 声明。裸 `1.5`、`1e0` 按 PG 数值常量规则初始为 numeric，不能因带小数点或指数直接推导 float8；显式转换和上下文才能改变类型（[18 §4.1.2.6](https://www.postgresql.org/docs/18/sql-syntax-lexical.html#SQL-SYNTAX-CONSTANTS-NUMERIC)）。OID 0 参数按上下文定型，无法确定时目标 42P18；ParameterDescription 确定 OID，格式码不决定宽度。

| 方向 | 值域、精度、NULL 与错误目标 | wire 身份、现状及差距 |
| --- | --- | --- |
| PG text 参数 → 规范浮点 | 按目标宽度直接解析十进制/指数，允许首尾空白及大小写不敏感 NaN、±Infinity/±Inf；拒绝空串、尾随垃圾为 22P02，编码错误 22021。有限文本溢出为无穷或非零下溢为零时 22003；可表示次正规数成功。float4 不先解析 float8 再窄化，以免二次舍入。 | OID 700/701 决定 f32/f64；长度 -1 为 typed SQL NULL，无载荷，空串不是 NULL。当前 Rust parse 的词法、超域分类与 PG 尚不等价，解析失败统一 22P02；NULL 参数仅 tp=6，身份连续性未验证。 |
| PG binary 参数 → 规范浮点 | 载荷恰为 4/8 字节的 IEEE 位型，含次正规、±0、NaN、±∞；接收本身无十进制舍入，不能将无穷当有限超域拒绝。过短目标 08P01、额外字节 22P03，消息截断仍归工作流 7。 | 网络大端；外层 -1 为 NULL，0 长度非法。当前 binary_parameter 已有网络序转换，但短/长均映为 22P03；保留位宽、零符号与特殊值的全链证据未验证。 |
| 规范浮点 → PG text 结果 | 先以规范身份校验并取得目标宽度的值；默认 extra_float_digits=1 时按该宽度输出可回读同值的最短十进制，不打印 f32 扩宽后的 f64 噪声。特殊值文本 NaN/Infinity/-Infinity，零符号须可观察；SQL NULL 长度 -1。 | 两格式均固定 700/4/-1 或 701/8/-1；当前 pg_type 有 4→700、5→701，value_text 共用 f64 to_string，不能证明 float4 或 PG 精确拼写。会话非默认 extra_float_digits 的状态/设置归工作流 5，不能将其有损文本仍标为无损往返。 |
| 规范浮点 → PG binary 结果 | 按身份输出 4/8 字节网络序，有限值和 ±0 保持目标宽度位型；±∞保持符号，NaN 按 NaN 类别验收，不承诺 NaN payload/signaling 位跨文本或运算保存。NULL 无载荷。 | 当前 value_binary 经文本重解析，不能据 1.5 测试推断所有位型可逆；内部未窄化 float4 不得在出口静默变无穷/零，已开放域错误 22003，缺保真表示/codec 时明确 0A000，工作流 7 承接 SQLSTATE 传输。 |
| float4 ↔ float8 / 数值 cast | float4→float8 有限值精确扩宽；float8→float4 按目标宽度舍入，有限溢出、非零变零拒绝 22003，次正规非零成功；原有无穷/NaN 合法传递。typed NULL 保留目标身份。浮点→整数先按参考平台 rint 舍入再检查域，NaN/无穷拒绝 22003；numeric 半值远离零规则不能套用。 | 目标 OID 由 cast 预先确定，不能按值动态提升。显式/赋值/隐式资格交本章转换块核定，算术/重载/round 归工作流 4；不把本表当完整隐式转换矩阵。MySQL 参数和结果维持自身 FLOAT/DOUBLE code、flags 与告警契约，不套 PG 错误；共享 f64 容器不是双方言等价证据。 |

特殊值须区分 SQL NULL：NaN 是非 NULL 的浮点值，PG 比较语义中 NaN 等于 NaN 且大于其他值，±0 数值相等但 codec 验收分别检查符号；无穷、NaN、零的比较与算术由工作流 4 拥有。本块不要求文本往返保存 NaN payload；binary 直通是否保存其位型须另采证，不能将全部 NaN 位串判为非法。舍入验收记录参考端平台与模式，常见最近偶数规则须有具体半值断言，不能用统一 epsilon 淹没 float4/float8 差异。

官方源码依据为 **REL_18_0** [float.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/float.c) 的 float4in_internal/float8in_internal、float4out/float8out、float4recv/send、float8recv/send、ftod/dtof、dtoi4：分别核对直接目标宽度解析、输入/范围错误、输出及窄化和整数转换；二次舍入反例取其注释 `7.038531e-26`。网络位型与不足读取见同 tag [pqformat.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/libpq/pqformat.c) 的 pq_sendfloat4/8、pq_getmsgfloat4/8、pq_copymsgbytes；额外载荷错误沿前块消息边界来源。输出配置以 PostgreSQL **18** [§19.11.2 extra_float_digits](https://www.postgresql.org/docs/18/runtime-config-client.html#GUC-EXTRA-FLOAT-DIGITS) 为据。查阅日期 2026-10-23；以上均为规范阅读，18.6 补丁和实际参考端行为仍未验证。

#### 文本类型契约

本块定义通用 text、varchar、SQL char（内部 bpchar），与目录单字节 `"char"` 区分。长度按字符计数，wire 长度按字节计数；UTF-8 下不按字素簇或视觉宽度截断，不自动 Unicode 规范化。数值精度/scale 不适用，约束是字符数 n。无长度 varchar 保持无约束身份，SQL char 省略长度按 1；无长度 bpchar 是独立的无约束、尾空格不显著形式，不能误套 char(1)。声明长度 n 的 PG 18 合法域为 1..10485760；不合法声明目标 22023，由工作流 2 保留类型语法并交本章校验。

| 方向 | 值、NULL 与错误目标 | 身份、元数据及现状差距 |
| --- | --- | --- |
| PG text 参数 → 规范文本 | 按已确定 OID 完整解码；空载荷为空串，长度 -1 为 SQL NULL，无载荷；两者不得合并。UTF-8 基线拒绝 NUL、非法序列为 22021，不替换字符或截断至 NUL。 | 25/1043/1042 各自保留，OID 0 仍交定型，不猜 text。ParameterDescription 只有 OID，无 typmod 字段；SQL 目标的长度约束不能因此丢失。当前输入校验存在，身份/长度接线未证明。 |
| PG binary 参数 → 规范文本 | 载荷是协商编码的文本字节，不带内部 varlena 头、不以 NUL 终止；空载荷合法，NULL 仍为 -1。与 text 参数同样验证编码/NUL，binary 不绕过长度约束。 | 格式 1 不改变类型，现有路径复用 parameter；并非任意 bytes/bytea。UTF-8 之外的协商和转码归工作流 5、7，未开放编码明确 0A000，不能假称已转码。 |
| 规范文本 → PG text/binary 结果 | 两格式都输出已定型值的文本字节；char(n) 的补空格保留，varchar/text 的尾空格保留；NULL 长度 -1，非 NULL 空串为 0。非法内部编码/NUL 在发布结果前拒绝，目标 22021；缺身份/codec 的能力门禁为 0A000。 | text 为 25/-1/-1；varchar(n)、char(n) 为 1043、1042，typlen 均 -1，typmod=n+4；无约束 typmod=-1。Prepare/Describe/Execute、零行及全 NULL 均一致，不按首行猜。当前普通 text fallback、固定 -1 和 io::Error 尚不能证明身份与 SQLSTATE 传输。 |
| 赋值/长度 coercion 与显式 cast | 向 varchar(n)/char(n) 赋值超长且超出部分含任何不是 U+0020 的字符时 22001；仅多出的 U+0020 可裁去。显式 cast 可截到 n 字符且不切断多字节编码。char(n) 短值补 U+0020，varchar 不补；char 转 text/varchar 去除尾部补空格，text/varchar 不通用 trim。typed NULL 保留目标身份。 | 目标 OID/typmod 由声明或表达式定型决定。隐式/赋值/显式资格交本章转换块，不由 codec 擅自截断；尚无 PG 长度 coercion 完整证据。通用字符值不承诺数值转换、排序或匹配算法。 |
| MySQL 参数/结果 ↔ 共享值 | 保留其自身 charset、collation、声明长度、严格/非严格 sql_mode 与 CHAR 尾空格规则；字符长度和协议字节宽度分开。NUL、非法序列、截断的错误/告警按 MySQL 参考端单独验收，不强套 PG 22021/22001。 | 保留 VARCHAR/VAR_STRING/STRING code、flags、charset 与长度元数据，不将 PG OID 或 n+4 写回 MySQL。共享 KindString、名称相似或 charset=63 均不是双方言语义等价证据；不在本块重写既有 MySQL 路径。 |

规范依据（查阅于 2026-10-23）：[PostgreSQL 18 §8.3 Character Types](https://www.postgresql.org/docs/18/datatype-character.html) 支持字符长度、补空格、赋值与 cast 差别、尾空格及 NUL 禁令；[REL_18_0 varchar.c](https://github.com/postgres/postgres/blob/REL_18_0/src/backend/utils/adt/varchar.c) 的 anychar_typmodin、bpchar_input、bpcharrecv/bpcharsend、varchar/varcharrecv/varcharsend 支持 typmod 与文本载荷边界；[REL_18_0 varlena.c](https://github.com/postgres/postgres/blob/REL_18_0/src/backend/utils/adt/varlena.c) 的 textrecv/textsend 支持无内部头的文本字节编码。[PG 18 §54.7 Bind/DataRow/RowDescription/ParameterDescription](https://www.postgresql.org/docs/18/protocol-message-formats.html) 支持 -1 NULL、字节长度及元数据字段；[PG 18 Appendix A](https://www.postgresql.org/docs/18/errcodes-appendix.html) 支持 SQLSTATE 分类。固定源码标签是 18.0 取证，不等于已在目标 18.6 运行或证明补丁版全部一致。

#### 字节类型契约

规范 bytea 为有序原始字节序列，每字节 0–255，允许 NUL、非 UTF-8、前导/尾随零；长度按字节计，字符精度、scale、collation 不适用。复用引擎 bytes 与 binary charset 的 string/blob 家族时须保留 bytea 身份，不因值可打印就改为 text，不从 BLOB/BIT 名称推导 PG 等价。PG 参数 OID 17，结果 OID/typlen/typmod=17/-1/-1；MySQL 独立验收 type code、charset/flags、长度，保留 Go 原行为。资源上限须实际取证，不能从变长类型或 i32 帧长度承诺已支持近 1GB；超限明确拒绝，禁止截断。

规范依据：[PostgreSQL 18 §8.4、8.4.1、8.4.2](https://www.postgresql.org/docs/18/datatype-binary.html) 支持字节域、hex/escape 语法与 bytea_output；[官方 REL_18_0 varlena.c](https://github.com/postgres/postgres/blob/REL_18_0/src/backend/utils/adt/varlena.c) 的 byteain/byteaout 支持 escape 错误及输出，bytearecv/byteasend 支持 raw binary。核查日期 2026-10-23；源码固定为 18.0，文档为 18 主版本，不构成来源指定 18.6 的发布或运行验证。

| 方向 | 目标值、NULL 与错误 | 当前差距与元数据 |
| --- | --- | --- |
| text 参数→bytes | Bind 实际字符的 hex 以 `\x` 开头，数字大小写皆可，字节对间可有空白，不能拆开一对；`\x` 为零字节。escape 接受普通字符、`\\` 为 5c、三位八进制 `\000`/`\377` 为 00/ff；空串也为零字节。非法数字/奇数半字节/非法 escape 拒绝 22P02；UTF-8 文本载荷非法编码/NUL 拒绝 22021，但解码后的 00/ff 合法。 | 当前 hex-only/无空白为兼容缺口，不能把合法输入被拒绝当作目标通过。NULL 长度 -1 不解析，仍保留 OID 17。SQL 字面量先经过工作流 2 的引号/反斜线解析，不能把 SQL 拼写直接当 Bind 字符。 |
| binary 参数→bytes | 逐字节保留，无 hex/escape 解析、字符转码、字节序转换、varlena 头或终止 NUL；长度 0 合法，NULL 为 -1。任何字节组合本身都不是非法 binary bytea。 | binary_parameter 可直接复制，内核往返缺证；不能套固定宽度错误或以 22P03 拒绝任意字节。帧截断/长度不符由工作流 7 拒绝 08P01，资源限制另记。 |
| bytes→text 结果 | 默认 bytea_output=hex：`\x` 加每字节两位小写数字，空序列输出两个字符 `\x`。escape 模式将反斜线输出两次、非打印字节输出三位八进制、打印 ASCII 原样；回读保留同一字节序列。 | 当前只输出 hex，escape 配置/输出缺证，会话状态归工作流 5。DataRow 长度按输出字节计（hex 为 2+2n），不含 C 结束符；SQL NULL 只写 -1，无载荷。 |
| bytes→binary 结果 | n 个原始字节原样发送，含 NUL/ff；空序列长度 0，NULL 长度 -1。格式选择和 bytea_output 不改变类型或值。 | Value::Bytes 可复用；禁止经 BinaryLiteral::ToString、from_utf8_lossy、替换字符、去前导零或数值转换。Describe/Execute、零行、全 NULL 均保持 17/-1/-1；局部结果断言不证明全链。 |

转换边界：bytea 同型转换保留全部字节与 typed NULL；bytea 的文本输出表示和字符解码是不同契约，不能隐式使用有损 UTF-8 或把 text 直接改标 bytea。text↔bytea 的显式/赋值/隐式资格交本章转换块核对 PG cast；未开放路径明确 0A000，不借 MySQL HEX/整数转换猜测兼容。字节比较与运算交工作流 4，不能复用去前导零的 BinaryLiteral::Compare 推断等价。

#### 日期类型契约

日期是独立 date 身份，无时刻、时区或小数秒精度；目标采用前推 Gregorian 日历。官方 [PostgreSQL 18 §8.5、§8.5.1.1、§8.5.1.4、§8.5.2](https://www.postgresql.org/docs/18/datatype-datetime.html) 支持日历、输入/输出及特殊词目标：基线固定 UTF-8、DateStyle=ISO,YMD，普通输出 YYYY-MM-DD，BC 保留 BC 后缀；MDY/DMY 的 `1/8/1999` 分别为 1 月 8 日/8 月 1 日，不能套 MySQL 短年/分隔符规则。`epoch` 为 1970-01-01；now/today/yesterday/tomorrow 在读取时解析为普通日期，依赖事务时间和会话时区，不能误作每次执行重新计算的动态 date。±infinity 是独立非有限值，非 NULL、非最大/最小有限日；allballs 仅 time，不能作为 date 成功输入。

精确有限域按官方 18 分支 [timestamp.h](https://github.com/postgres/postgres/blob/REL_18_STABLE/src/include/datatype/timestamp.h)::IS_VALID_DATE/POSTGRES_EPOCH_JDATE：日序 -2451545..2145031948（含端点），对应 Julian day 0 至 5874897-12-31；下端按前推 Gregorian 为 4714-11-24 BC，区别于手册表中概略年份 4713 BC。BC/AD 无外部年份 0，内部天文年不能直接以 0000 文本输出。[date.h](https://github.com/postgres/postgres/blob/REL_18_STABLE/src/include/utils/date.h)::DateADT/DATEVAL_NOBEGIN/DATEVAL_NOEND 定义 i32 及 -2147483648/+2147483647 两个特殊哨兵；其余超有限域整数必须拒绝，不能全部当日期。

二进制按官方 18 分支 [date.c](https://github.com/postgres/postgres/blob/REL_18_STABLE/src/backend/utils/adt/date.c)::date_in/date_out/date_recv/date_send：四字节有符号网络序、单位日、基准 2000-01-01，零即该日，-1 为 1999-12-31，+1 为 2000-01-02；不是 Unix 秒、微秒或 MySQL 年月日结构。两特殊哨兵分别对应文本 -infinity/infinity。该链接为读取于 2026-10-23 的可变 REL_18_STABLE，提供 18 分支格式证据，未锁定 18.6 构建或运行参考端；实施时须记录实际参考提交。

| 方向 | 类型、值域、NULL 与 wire 目标 | 错误和现状差距 |
| --- | --- | --- |
| text 参数→规范 date | 已定型 OID 1082 按 PG 日历/DateStyle 解码；日期身份跨桥接保留，不能只变字符串或 MySQL 宽松日期。NULL 长度 -1，无载荷且仍为 date；空串是非法非 NULL。 | 非日期词/空串 22007；非法月日、零日期和有限范围越界 22008；非法编码/NUL 22021。当前 chrono 单格式及 1..9999 不能证明完整 PG 输入。 |
| binary 参数→规范 date | 四字节解日序及哨兵，域内与对应 text 相同值、日精度；NULL 同上，不进入长度检查。 | 当前非 NULL 完全未开放。不足四字节目标 08P01，额外字节 22P03，合法四字节但越日期域 22008；帧截断另为 08P01。不能把空载荷当零日。 |
| 规范 date→text 结果 | 列 OID/typlen/typmod=1082/4/-1；输出随已固定 DateStyle，有限日期/BC/特殊值可回读。SQL NULL 长度 -1，不输出空串或特殊词。 | 当前粗校验漏非法月日，不能原样透传。MySQL 0000-00-00、2024-00-01、2024-01-00 及非法实际日显式拒绝；内部值不合法目标 22008，不静默归一化。 |
| 规范 date→binary 结果 | 同一列身份 1082/4/-1，四字节日序或特殊哨兵；与文本结果表达同一日期，NULL 仅 -1 长度。 | chrono 和现有 MySQL 表示不能证明 BC、超 9999 或非有限值保真；禁止截断、wrap、借用零日期或降为 text OID。 |

错误分类依据官方 18 分支 [datetime.c](https://github.com/postgres/postgres/blob/REL_18_STABLE/src/backend/utils/adt/datetime.c)::ValidateDate/DateTimeParseError 与 [附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html)；协议长度分类沿本章共同契约，消息边界由工作流 7 负责。当前结果编码使用 io::Error，扩展执行可统一包装 0A000，尚不能证明精确 SQLSTATE。PG 合法扩展年份/非有限值若共享表示暂不能承载，过渡拒绝须明确 0A000 并登记兼容缺口；这不等于完成 PG 全域目标，也不能错误报告用户输入超 PG 范围。

类型推导以声明 DATE、typed literal/cast 或参数上下文定型为 date；普通字符串外观不能决定 OID，未知参数交本章映射/转换块。无 date(p) 小数精度，typmod=-1，不把 MySQL fsp/显示宽度充作 PG typmod。显式 text↔date 按日期输入/输出契约；date↔timestamp/timestamptz 的资格与时间/时区转换交对应类型及转换块，不能按天乘微秒自动推导隐式转换。MySQL 既有宽松解析、零日期及 sql_mode 告警独立保留；从 MySQL 结果桥接 PG 时逐值检查合法性，拒绝不改变原 MySQL 行为。

#### 日内时间类型契约

time 单独使用即 time without time zone，保留日内身份，不携带日期或偏移。目标有限域为含端点的 0..86400000000 微秒，末端独立表示 24:00:00，不能 wrap 成零点。官方依据（2026-10-23 查阅）为 [PostgreSQL 18 §8.5 表 8.9、§8.5.1.2、§8.5.1.4 与 §8.5.2](https://www.postgresql.org/docs/18/datatype-datetime.html)：支持日内值、输入形式与输出；普通 time 输入中合法时区说明被忽略，不能因此定型为 timetz，也不能按 TimeZone 移动已存墙钟时间。ISO 基线接受 04:05、040506、04:05 PM 等官方形式，输出 HH:MM:SS 加必要小数位；allballs 为零点。命名时区涉及日期的解析要求由会话工作流协作，不以删除字符串后缀替代校验。

精度为秒后 0–6 位，未声明 typmod=-1，声明 time(p) 的 typmod 直接为 p，不加 4。先按 PG 输入规则归为微秒，再按目标 p 舍入，不能简单截断或把输出补零宽度当精度。官方 18 分支 [date.c](https://github.com/postgres/postgres/blob/REL_18_STABLE/src/backend/utils/adt/date.c)::anytime_typmod_check/AdjustTimeForTypmod/time_recv/time_send 为精确依据：负精度拒绝 22023，大于 6 告警 22023 并按 6；非负微秒以半单位加偏移的整数方式舍入，日末可进位到 24:00:00。binary 为八字节有符号网络序日内微秒，不是 Unix 时间或 MySQL duration 结构，接收范围在舍入前校验。该链接为可变 REL_18_STABLE，未锁定 18.6 参考构建；后续实施须记录实际提交及对照版本，不把本轮阅读当实测。

| 方向 | 类型、值域/精度、NULL 与 wire 目标 | 错误和现状差距 |
| --- | --- | --- |
| text 参数→规范 time | 已定型 OID 1083，按 PG 日内输入解析并应用 SQL 目标 p；ParameterDescription 无 typmod 字段，不丢 SQL 约束。NULL 长度 -1、无载荷仍保留 time 身份；空串不是 NULL。 | 非法文本/空串 22007，分秒/小时越界 22008，非法 UTF-8/NUL 22021；当前单一 chrono 格式及非整微秒拒绝不等于 PG 输入舍入。 |
| binary 参数→规范 time | 恰八字节 i64 微秒，0 与 86400000000 均合法，按目标 p 舍入，与 text 表达同一规范值。NULL 同上，不走长度检查。 | 负数、86400000001 及 i64 两端拒绝 22008；不足八字节 08P01，额外字节 22P03，帧声明不符 08P01。当前非 NULL 未开放。 |
| 规范 time→text 结果 | 1083/8/p 或无声明 -1；合法值规范输出、可回读，24:00:00 保留；NULL 为长度 -1。先验证完整时分秒和微秒域。 | 不能仅验证小时后透传非法分钟/秒；非法内部日内值目标 22008，缺表示保真能力明确 0A000 并记兼容缺口。现有 io::Error 尚不证明准确 SQLSTATE。 |
| 规范 time→binary 结果 | 同一列身份及 typmod，八字节网络序微秒；NULL 仅 -1 长度，不能编码为数值零。Describe 与 Execute 不随首行值/格式改型。 | 当前 binary 经文本再解析，24:00 与精度保真缺口须补证；不能静默降为 text OID 或按天取模。 |

文本范围与错误依据另见官方 18 分支 [datetime.c](https://github.com/postgres/postgres/blob/REL_18_STABLE/src/backend/utils/adt/datetime.c)::DecodeTimeOnly/time_overflows/DateTimeParseError 和 [附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html)；消息长度及恢复沿本章共同规则交工作流 7。显式 text↔time 依输入/输出契约；time(p) coercion 保留类型，时间戳取 time、time 与 interval 的转换资格交对应块和转换块，不能因同为 i64 自动允许隐式转换。

MySQL 参数/结果仍按自身 TIME code=11、符号、Fsp、天数及 sql_mode 解释，不套 PG SQLSTATE，也不改 Go 原行为。共享 duration→PG time 仅在身份和精度可保真、值在日内域时显式桥接；负值 -00:00:01、25:00:00、838:59:59 均明确 0A000 拒绝，不取绝对值、不取模、不截断或自动提升 interval。反向 PG time→MySQL TIME 须有显式桥接并保留微秒/24:00，typed NULL 仍需原身份；没有 PG 类型身份时不能从同名 TIME 或字符串外观推导等价。PG time with time zone（timetz，OID 1266）不在来源枚举类型中，本块不新增其支持，定型/codec 请求须明确 0A000；普通 time 文本忽略合法偏移与支持 timetz 是两项不同契约。

#### 无时区时间戳契约

timestamp 单独使用即 timestamp without time zone，表达日期加墙上时间，不是 UTC 瞬间；类型由声明、typed literal/cast 或参数上下文决定，不能检查字符串偏移或列名后改成 timestamptz。官方行为依据为 [PostgreSQL 18 §8.5 表 8.9、§8.5.1.3–4、§8.5.2](https://www.postgresql.org/docs/18/datatype-datetime.html)（2026-10-23 查阅）：合法时区说明在已定型的无时区输入中被忽略，不移动日历字段；DateStyle 决定歧义输入与输出。固定 ISO 基线，输出日期和时分秒及必要小数，BC/AD 无外部零年；支持其他官方输入形式时须按配置差分，不能只删除后缀。

精确表示依据锁定官方 **REL_18_0（18.0）** [timestamp.h](https://github.com/postgres/postgres/blob/REL_18_0/src/include/datatype/timestamp.h)::IS_VALID_TIMESTAMP/MIN_TIMESTAMP/END_TIMESTAMP：有限微秒域为 `[-211813488000000000, 9223371331200000000)`，基准 2000-01-01 00:00:00；下端为 Julian day 0（前推 Gregorian 4714-11-24 BC），上端最后微秒为 294276-12-31 23:59:59.999999。手册概略年份不代替精确日序；共享 MySQL 位域与 chrono 不能证明覆盖此域。i64 最小/最大另表示 -infinity/infinity，不是有限端点或 NULL。

[同版本 timestamp.c](https://github.com/postgres/postgres/blob/REL_18_0/src/backend/utils/adt/timestamp.c)::timestamp_in/out/recv/send、anytimestamp_typmod_check、AdjustTimestampForTypmod 定义文本和二进制边界：binary 恰八字节有符号网络序微秒，不是 Unix 秒、纳秒或 MySQL 日期结构；无声明 typmod=-1，timestamp(p) 为 p（0–6，不加 4）。声明负 p 拒绝 22023，超过 6 告警 22023 并收敛到 6；输入先归微秒，再按目标 p 舍入，typmod 半单位按相对 PG epoch 的正负向远离零舍入，不能对负值套正值取整或直接截断。超过六位输入按 PG 解析舍入，不沿用现有拒绝；进位可跨日/年。这些来源未证明 18.6 构建或本地功能通过，实施时需锁定参考构建。

| 方向 | 类型、精度、NULL 与 wire 目标 | 错误与当前差距 |
| --- | --- | --- |
| text 参数→规范 timestamp | OID 1114，按 PG 日期时间及 DateStyle 解析，应用 SQL 目标 p；ParameterDescription 不携 typmod，SQL 约束仍需保留。NULL 长度 -1、无载荷仍有类型，空串不等于 NULL。 | 空串/garbage 为 22007；非法日历、零日期与超范围 22008；UTF-8/NUL 错误 22021。当前固定格式、年限及非整微秒拒绝不符合完整目标，temporal 的错误分类也未验证。 |
| binary 参数→规范 timestamp | 八字节 i64 按有限域或两个特殊哨兵解释；应用 SQL 精度约束，与 text 同规范值；NULL 不走载荷长度/范围检查。 | 不足八字节 08P01，额外字节 22P03，帧声明不符 08P01；其余超有限域整数 22008。当前非 NULL 未开放。 |
| 规范 timestamp→text 结果 | OID/typlen/typmod=1114/8/p 或 -1；输出可回读的墙上时间，保留 BC、扩展年份、微秒或 infinity；NULL 仅长度 -1。 | 完整校验日历和时间，非法内部值 22008；合法 PG 值无法保真承载时明确 0A000 并登记兼容缺口，禁止裁到 9999、epoch、NULL 或自动改 1184。当前粗查日期不足，io::Error 不证明 SQLSTATE。 |
| 规范 timestamp→binary 结果 | 同一列身份及 typmod，八字节网络序微秒或特殊哨兵；NULL 无载荷，不编码为零（零是 PG epoch）。Describe 与 Execute 结果格式切换不改身份。 | 当前经文本再解析的 chrono 路径不证明全域、特殊值与舍入保真；不能以 text 能透传为 binary 可用证据。 |

错误名参照 [PostgreSQL 18 附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html)，长度与消息恢复沿共同契约交工作流 7。epoch 输入归 1970-01-01 00:00:00；infinity/-infinity 独立保存；now/today/tomorrow/yesterday 在读入时按事务时间及会话日期归具体值，Prepare 常量和 Bind 参数的读入时机分别验收。相对词、DateStyle 的状态归工作流 5；精确输入分类可对照官方 REL_18_0 [datetime.c](https://github.com/postgres/postgres/blob/REL_18_0/src/backend/utils/adt/datetime.c)::DecodeDateTime/DateTimeParseError，不能将所有解析失败合为 22P02。

MySQL DATETIME(code=12) 与 TIMESTAMP(code=7) 必须保留 NativeType、Fsp、sql_mode 和会话时区证据：前者按自身日历字段，后者在既有会话/存取转换后可能显示不同字段，不从显示值推断原始瞬间。MySQL→PG 仅在显式桥接策略和来源语义已定、值/精度可保真时输出 1114；缺少会话转换前提明确 0A000，零/非法日期两格式均 22008，不补时区、不暗转 1184。反向 PG→MySQL 显式桥接须校验目标类型范围、Fsp、零日期政策与 TIMESTAMP 会话转换；BC、扩展年份或非有限值无法承载时 0A000，不静默截断，也不修改原 Go/MySQL 错误与告警。显式 text↔timestamp 依上述输入/输出；date→timestamp 的午夜、timestamp→date 的去时间及与 timestamptz 互转的时区前提交转换块/下一类型块确定资格，不因共用数值单位允许隐式 cast。

#### 带时区时间戳契约

timestamptz 是 timestamp with time zone 的别名，身份表达瞬时，不保存原时区名称或输入偏移拼写。输入有偏移时据此归一，无偏移时依当前 TimeZone；输出按会话时区显示。类型必须先由声明、cast 或参数上下文确定，不能靠字符串外观改型；DateStyle 影响输入/显示。依据为 [PostgreSQL 18 §8.5.1.3、§8.5.2–3](https://www.postgresql.org/docs/18/datatype-datetime.html)（2026-10-23 查阅）。例如 `2000-01-01 00:00:00+00` 与 `2000-01-01 08:00:00+08` 同值；会话从 UTC 改为 Asia/Shanghai 只改变显示，不改变存量瞬时。

夏令时歧义按 [PostgreSQL 18 附录 B.2](https://www.postgresql.org/docs/18/datetime-invalid-input.html)：向前跳跃缺失时间采用转换前偏移，向后重复时间采用转换后偏移；不能统一拒绝或取 chrono 默认。America/New_York 的 `2018-03-11 02:30` 解释为 UTC 07:30，显示 03:30-04；`2018-11-04 01:30` 解释为 UTC 06:30，显式 -04 则为 UTC 05:30。时区数据库版本必须随证据记录；数值偏移无此歧义。

规范瞬时以 UTC 的 2000-01-01 午夜为微秒基准；有限域 `[-211813488000000000, 9223371331200000000)`，两端及 BC/扩展年份承接前块精确日序，范围按归一后瞬时检查，不能仅查本地显示年份。i64 最小/最大分别为 -infinity/infinity，均非 NULL。依据锁定官方 **REL_18_0（18.0）** [timestamp.h](https://github.com/postgres/postgres/blob/REL_18_0/src/include/datatype/timestamp.h)::TimestampTz/IS_VALID_TIMESTAMP/TIMESTAMP_NOBEGIN/TIMESTAMP_NOEND；这不是 18.6 参考端运行证据。

身份元数据按同版本 [pg_type.dat](https://github.com/postgres/postgres/blob/REL_18_0/src/include/catalog/pg_type.dat) 为 OID 1184、typlen 8；typmod 未声明 -1，声明 p 为 0–6 的 p，不加 4。ParameterDescription 只有 OID，SQL 目标精度仍须保留。精度处理依据 [timestamp.c](https://github.com/postgres/postgres/blob/REL_18_0/src/backend/utils/adt/timestamp.c)::timestamptz_in/out/recv/send、anytimestamp_typmod_check/AdjustTimestampForTypmod：与前块共用微秒和 p 舍入规则，半单位按 epoch 两侧远离零，进位可跨日；负声明 p 拒绝 22023，超过 6 告警并收敛到 6。合法七位输入不能照搬当前参数拒绝。

| 方向 | 值、精度、NULL 与 wire 契约 | 错误与现状差距 |
| --- | --- | --- |
| text 参数→规范 timestamptz | 已定型 1184，解析日期/偏移或会话时区后归一并应用 SQL p；NULL 长度 -1 无载荷，保留类型；空串不是 NULL。 | 空串/语法错误 22007，非法日历、零日期、归一溢出 22008，UTF-8/NUL 22021；当前非 NULL 1184 为 0A000，完整 PG 解析与错误未验证。 |
| binary 参数→规范 timestamptz | 恰八字节有符号网络序微秒或两个哨兵；载荷没有时区字段，不能再加减会话偏移；应用 SQL p，NULL 跳过载荷解析。 | 短载荷 08P01、多余字节 22P03、消息帧不符 08P01；非哨兵超有限域 22008，并按参考 recv 检查能否转换显示。当前非 NULL 1184 无分支。 |
| 规范 timestamptz→text 结果 | 1184/8/p 或 -1；按 TimeZone/DateStyle 输出带偏移的可回读文本，特殊值输出 infinity/-infinity；NULL 仅长度 -1。 | 非法内部瞬时 22008；合法 PG 值无法承载或所需会话信息缺失时明确 0A000 并登记缺口，不伪造 UTC、不冒充 1114。现有 code=7/12 映射不能证明此方向。 |
| 规范 timestamptz→binary 结果 | 同一身份与精度，八字节网络序瞬时，两格式共享规范值；NULL 无载荷；时区变化不改变字节。 | 共用完整域/特殊值检查，不能从显示文本剥偏移后走现有 1114 路径；当前无独立 1184 codec，SQLSTATE 与客户端往返未验证。 |

显式 timestamp→timestamptz 必须给定会话 TimeZone 或明确指定的区域，按该区日历与夏令时规则解释墙钟；反向先按目标区域显示再去时区，保留墙钟字段，会丢失瞬时唯一性（重复小时可映射相同 timestamp）。UTC 下 timestamp `2000-01-01 00:00:00` 转换为 epoch，Shanghai 下同一墙钟变为前日 UTC 16:00。NULL 保留目标类型，非有限值按 PG 特殊值规则传播；转换溢出 22008。显式 text↔timestamptz 依上表；date 相关转换、隐式/赋值资格及 AT TIME ZONE 语法/运算交本章转换块与工作流 2/4，不因共享 i64 自动允许隐式 cast。

MySQL TIMESTAMP 的既有时区转换需要原类型、存取阶段、来源时区与 Fsp 证据；显示字符串不足以恢复瞬时。DATETIME 无瞬时身份，必须显式指定解释区域；缺信息桥接拒绝 0A000，零/非法日期拒绝 22008。反向桥接校验 MySQL 目标范围/Fsp 与会话转换，BC、扩展年份及非有限值不能保真时 0A000；不裁剪、补零或改成 NULL，不改变原 Go/MySQL 宽松解析、错误和告警。内部表示需补瞬时身份、完整微秒域/特殊值、p 和 typed NULL 保真，不能只增加 OID 表。TimeZone 状态存储、SET/事务生命周期及读入时机由工作流 5 提供确定上下文；本块不实现该状态。

#### 时间间隔契约

PG interval 是独立规范身份，目标保留 `(months:i32, days:i32, time_microseconds:i64)`，允许分量独立负号；年归月、小时归微秒，不能把整数月折为 30 天、把整数日折为 24 小时或将 24 小时自动进位为日。身份和分量必须跨参数、cast、结果保真，不能用比较相等代替分量相同。[PG 18 §8.5.4–8.5.5](https://www.postgresql.org/docs/18/datatype-datetime.html#DATATYPE-INTERVAL-INPUT) 支持单位文本、SQL/ISO 8601 输入及四种 IntervalStyle 输出；固定 postgres 基线，并分别核对 postgres_verbose、sql_standard、iso_8601。输入 `1.5 months` 的小数部分按官方规则归到日，不能由此推广到整数月一律折日；IntervalStyle 影响未独立标号的负字段解释，会话配置交工作流 5。

精确布局以官方 **REL_18_0** 的 [datatype/timestamp.h](https://github.com/postgres/postgres/blob/REL_18_0/src/include/datatype/timestamp.h) 为基线：三个有符号分量，时间单位为微秒；不是 MySQL ±838 小时域。整数分量边界分别是 i32/i64 范围，三者全最小/全最大组合保留为 -infinity/infinity，其他组合不能误判非有限。文本解析、单位累加和精度收敛都须检查溢出；不能把表 8.9 的约 ±178000000 年当每种分量的统一上限。合法 PG 值若当前承载不足，显式 0A000 并登记缺口，不能截断或伪装 NULL。

OID=1186、typlen=16 由 [REL_18_0 pg_type.dat](https://github.com/postgres/postgres/blob/REL_18_0/src/include/catalog/pg_type.dat) 定义；typmod 未声明=-1，有声明按 [utils/timestamp.h](https://github.com/postgres/postgres/blob/REL_18_0/src/include/utils/timestamp.h) 的 `(range & 0x7fff)<<16 | (p & 0xffff)`，不是裸 p。fields 接受 YEAR/MONTH/DAY/HOUR/MINUTE/SECOND 及官方组合，精度 p=0–6；fields 未指定为 full range，fields-only 保留 full precision。按 [REL_18_0 timestamp.c](https://github.com/postgres/postgres/blob/REL_18_0/src/backend/utils/adt/timestamp.c) 的 intervaltypmodin/AdjustIntervalForTypmod 截去最小允许字段右侧的部分，保留更高字段；p 舍入微秒时间，半值远离零，不能以截断替代。负 p 错误 22023，p>6 告警 22023 后归 6。SQL 声明语法交工作流 2。

| 方向 | 类型、值域/精度与 NULL | 编码、元数据、错误与差距 |
| --- | --- | --- |
| text 参数→规范值 | 明确 1186/目标 fields/p 后解析分量，按目标修饰符收敛；typed NULL 保留身份，不解析负一长度载荷。 | text 编码 UTF-8，无 NUL；非法语法 22007，区间解析溢出 22015，最终分量/typmod 舍入溢出按官方阶段 22008；非法编码/NUL 22021。现有 parameter 仅统一不支持，不证明这些分类。 |
| binary 参数→规范值 | 非 NULL 恰 16 字节；网络序依次 i64 微秒、i32 日、i32 月，符号独立；应用目标 typmod，NULL 与零值分开。 | interval_recv/send 为布局依据；短载荷 08P01、多余字节 22P03，帧长度错误 08P01，特殊组合识别一致。当前 binary_parameter 无 1186。 |
| 规范值→text 结果 | 根据 IntervalStyle 输出可回读文本，保留月/日维度、独立符号与微秒；NULL 长度 -1，不是空串或 0。 | Describe/Execute 恒为 1186/16/声明 typmod；输出格式不改类型，特殊值不是 NULL。现有 pg_type 无通用 interval 身份。 |
| 规范值→binary 结果 | 同一三分量直接按 16 字节布局输出，不经秒数总量或显示文本；NULL 长度 -1，零间隔为 16 字节全零。 | 同一 OID/typlen/typmod，格式码 1；当前出口未实现。内部非法状态不得静默绕过验证；错误恢复交工作流 7。 |

转换边界：显式 text↔interval 复用上述词法与输出契约；interval→同身份带 fields/p 的转换按声明收敛且传播 typed NULL。显式/赋值/隐式资格交本章转换块，不由 OID 推导任意数字、time、timestamp 可隐式互转。MySQL duration TIME 若显式桥接，仅在约定且可保真的 `(0,0,微秒)` 子域转换；反向要求月/日为零、时间精度与 MySQL 范围可承载，否则明确 0A000，不自动折算月/日。PG 合法性错误与暂缺桥接能力必须分开。日期加 interval、DST 与比较/justify 的运算实现归工作流 4；本块三分量不规定运算比较算法。以上规范来自 PG 18 文档及 REL_18_0 源码，不构成指定 18.6 的实测结论。

#### 通用唯一标识契约

UUID 目标为独立规范类型身份与固定 16 字节值，覆盖全部 128 位组合，包括全零、全 ff 和任意 version/variant 位；不因生成器只产生某版本而缩小输入域。没有数值精度、scale、时区或舍入参数，typmod=-1。依据 [PG 18 §8.12 UUID Type](https://www.postgresql.org/docs/18/datatype-uuid.html) 及固定版本 [REL_18_0 uuid.c](https://github.com/postgres/postgres/blob/REL_18_0/src/backend/utils/adt/uuid.c)::string_to_uuid/uuid_out/uuid_recv/uuid_send；以下为目标规范，未实测指定 18.6。

文本接受大小写十六进制、可选的一对外层花括号、无连字符以及每四个十六进制字符组后可选连字符；必须恰好 32 个十六进制字符，连字符不得在末尾。标准例 `a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11`、大写、`{a0eebc999c0b4ef8bb6d6bb9bd380a11}`、`a0ee-bc99-9c0b-4ef8-bb6d-6bb9-bd38-0a11` 均归同一值。输出固定小写 8-4-4-4-12，去花括号；不保留输入拼写。空串、前后空白、URN 前缀、非法字符、奇数位、缺/多位、错位连字符及不配对花括号按 22P02 拒绝，不修剪、补位或截断。编码错误沿文本块的 22021 先行检查。

| 方向 | 身份、值域、NULL 与可观察目标 | 元数据、错误与现状差距 |
| --- | --- | --- |
| text 参数→规范 UUID | 已定型 2950 才按上述词法解码；NULL 保持 UUID 身份，无载荷，不等于全零值。 | ParameterDescription=2950；非法文本22P02。当前非NULL无2950分支，能力未接通时明确0A000。 |
| binary 参数→规范 UUID | 恰16字节，按规范文本从左到右每两位对应一字节；`00112233-4455-6677-8899-aabbccddeeff` 对应 `00 11 22 33 44 55 66 77 88 99 aa bb cc dd ee ff`。不采用主机整数序或GUID混合端序，不加version校验。 | OID2950；短载荷08P01，多余载荷22P03，不能静默消费前16字节。当前binary_parameter缺分支。 |
| 规范 UUID→text 结果 | 固定36个ASCII字节规范输出；NULL长度-1，非NULL全零仍输出完整UUID。 | RowDescription=2950/16/-1；文本格式码0，不得因显示字符串变成OID25。现有pg_type无法输出该身份。 |
| 规范 UUID→binary 结果 | 原样输出16字节；NULL长度-1，不能发送空载荷或ASCII十六进制。 | RowDescription=2950/16/-1；格式码1，只变格式不改身份。现有value_binary无2950分支。 |

OID/typlen 依据 [REL_18_0 pg_type.dat](https://github.com/postgres/postgres/blob/REL_18_0/src/include/catalog/pg_type.dat) 的 uuid 项。NULL、格式码和元数据字段依 [PG 18 §54.7 Bind/DataRow/ParameterDescription/RowDescription](https://www.postgresql.org/docs/18/protocol-message-formats.html)。短载荷与剩余字节诊断分别依据 [pqformat.c](https://github.com/postgres/postgres/blob/REL_18_0/src/backend/libpq/pqformat.c)::pq_getmsgbytes 和 [postgres.c](https://github.com/postgres/postgres/blob/REL_18_0/src/backend/tcop/postgres.c)::exec_bind_message；消息帧破损与Sync恢复交工作流7，不混同合法UUID值域错误。

显式 text→uuid 使用上述输入规则，uuid→text 使用规范输出，两方向传播typed NULL；目标结果身份随cast目标变化。普通text/bytea、MySQL CHAR(36)/BINARY(16)不因长度或外观自动成为UUID；bytea不自动获得PG UUID cast资格，尚无有证据桥接时明确0A000。显式、赋值与隐式资格由本章转换块统一核定，禁止无条件复用MySQL字符串强制转换。声明列、显式UUID类型常量/转换以及已声明2950参数应保持身份；OID0参数只能在解析上下文真正定型后使用，不能扫描首个值猜类型。当前通用Parse缺省OID拒绝规则仍为现状；语法交工作流2，推导和PreparedMetadata连续性由本章接线，不能仅加codec声称完成。

#### JSON文本契约

PG `json` 是独立规范身份，保存通过校验的原始JSON文本；不是普通text、MySQL BinaryJSON或PG jsonb的别名。OID=114、typlen=-1、typmod=-1；ParameterDescription与statement/portal Describe、Execute保持身份，不能由首行文本外观推定。SQL声明、明确cast及参数上下文负责定型，OID0推导沿本章公共契约；精度/scale、时区不适用于文本载体，不添加numeric修饰符。

输入允许对象、数组、数字、字符串、true/false/null作为完整根值，保留前后合法JSON空白、键顺序、重复键全部成员、转义拼写、数字尾零和指数。数字只按JSON词法校验，不先转浮点或numeric：`9007199254740993`、`18446744073709551616`、`1e1000000`原样保存，不承诺算术可用或无限资源。`01`、`+1`、`NaN`、`Infinity`、大写TRUE、注释、尾逗号及多根值非法；空串/纯空白不是JSON null。资源上限须独立取证，不能以静默截断或有损降级成功。

固定UTF-8数据库及客户端编码时，直接Unicode字符必须编码合法；JSON字符串内合法四位十六进制转义保持词法，`"\u0000"`及孤立代理转义`"\uD800"`在原始输入中允许。这不承诺函数解码或jsonb转换成功，解码错误另按处理阶段验收；原始NUL字节不是转义序列，传输编码失败目标22021。非法JSON词法/结构目标22P02，两种格式同样校验，binary不因名称而免检。

| 方向 | 类型、值/NULL、错误与wire目标 | 当前差距 |
| --- | --- | --- |
| PG text参数→规范json | OID114定型，编码转换后校验完整文档并保留原文；SQL NULL长度-1单独标记，四字节`null`为非NULL。非法JSON22P02、非法编码22021。 | parameter无114分支；NULL提前返回不证明身份连续。 |
| PG binary参数→规范json | 同为OID114，载荷是JSON文本字节，无版本前缀、无MySQL类型码；按连接/服务器编码转换和校验，再保留文本。NULL=-1；零长度是非法文档22P02。帧长度不一致08P01交工作流7。 | binary_parameter无114分支；不能借bytea或jsonb codec。 |
| 规范json→PG text结果 | OID114/-1/-1，直接输出所存文本并按客户端编码转换；UTF-8到UTF-8逐字节保留。NULL=-1，JSON null长度4；失败不得回空串成功。 | MarshalJSON/String重新序列化，pg_type拒绝245，不具备原文出口。 |
| 规范json→PG binary结果 | 相同元数据，载荷仍为编码后的原文文本，无jsonb版本字节；NULL及错误约束同text。格式码1不代表内部BinaryJSON内存布局。 | 当前无114结果codec或原文身份。 |
| text↔json显式转换及MySQL桥接 | text→json须校验且保留文本，json→text返回存量原文；NULL保持SQL NULL。不将外观相同text隐式变json；隐式/赋值资格交统一转换块。MySQL BinaryJSON已丢失的重复成员/词法不能恢复为原文，只有明确且可验证的转换契约才能产生新的json表示，否则项目拒绝0A000。 | 共用JSON解析器或函数输出不证明PG cast资格；MySQL原Go行为独立保留，不作PG有损回退。 |

官方依据（2026-10-23查阅）：PostgreSQL **18** §8.14 [JSON Types](https://www.postgresql.org/docs/18/datatype-json.html) 定义原文、重复键、Unicode输入与json/jsonb数字域差异；精确codec核对 **REL_18_0** [json.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/json.c) 的 json_in/json_out/json_recv/json_send，输入校验后存text、两格式发送文本；[pg_type.dat](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_type.dat) 确定114及变长。§54.7 [Message Formats](https://www.postgresql.org/docs/18/protocol-message-formats.html) 支持格式、长度/NULL及描述字段，[附录A](https://www.postgresql.org/docs/18/errcodes-appendix.html) 支持22P02/22021/08P01/0A000分类；精确诊断与阶段仍需参考端采集。来源18.6目标保持不变，REL_18_0静态源码不是18.6实测。

#### JSONB规范值契约

PG `jsonb` 保留独立规范身份，OID3802、typlen=-1、typmod=-1；与OID114原文json、text、MySQL245分别定型。保存分解后的对象、数组、字符串、精确number、boolean和JSON null；不承诺原始空白、对象输入键序、重复键或数字原词法。嵌套各层重复键仅保留最后值，键按解码后的字符串识别；数组顺序和重复元素保留，标量不能强制包成数组。SQL NULL是外部缺值，JSON null是非NULL值；精度修饰符、时区、SQL数组维度/下界不适用此标量族。

number沿PG numeric有限数值域精确保留，禁止借f64或受限MySQL decimal静默舍入；超出PG域应拒绝，PG域内暂不能保真则0A000，不能伪称成功。输出按numeric展开指数并保留小数尾零，例如`1.230e-5`输出`0.00001230`；相同数值的不同scale不代表必须输出同字节。NaN/Infinity不是合法JSON number。固定UTF-8时，Unicode转义解码，正确代理对合并；拒绝`\u0000`、孤立/错误代理对与非法编码。对象键顺序不是用户输入顺序；REL_18_0按UTF-8键字节长度再按字节排序，不能用BTreeMap字典序替代。对参考端输出的字节断言须绑定精确构建，不能将内部排序当成跨版本存储格式。

| 方向 | 目标类型、值/NULL及wire契约 | 现状与拒绝边界 |
| --- | --- | --- |
| PG text参数→规范jsonb | OID3802按连接编码接收JSON文本，完整校验、精确数字解析及规范化；-1长度为SQL NULL，`null`为JSON null，空文档非法。 | parameter缺3802分支；未补齐身份与保真前0A000。 |
| PG binary参数→规范jsonb | OID3802载荷为单字节版本01加JSON文本，无内部容器头；版本后仍做同等语义校验。SQL NULL=-1无版本字节。 | binary_parameter缺3802分支；禁止把MySQL BinaryJSON、PG磁盘布局或无版本json文本直接接受。 |
| 规范jsonb→PG text结果 | 输出PG规范JSON文本；RowDescription保持3802/-1/-1，格式0；空集、首行NULL和全NULL均不改变身份。 | pg_type无通用JSON出口；禁止回落text OID25。 |
| 规范jsonb→PG binary结果 | 同一元数据、格式1；载荷01加与text输出对应的JSON文本，长度含版本字节，SQL NULL=-1无载荷。 | 没有通用3802编码；不是直接输出内存字节或json的无前缀binary。 |
| 显式cast与MySQL桥接 | text/json→jsonb重新校验、规范化；jsonb→text/json输出规范文本，无法恢复原始重复键、空白或键序；NULL保持SQL NULL。隐式转换资格由方言先决策，不能自动接受普通text。 | MySQL扩展类型及不能精确保真的数值必须显式拒绝0A000；不重解释已有Go/MySQL函数；cast资格交后续转换块。 |

非法JSON目标22P02，数值越PG numeric域目标22003，无法表示的Unicode转义目标22P05，非法UTF-8/原始NUL按编码阶段22021；错误必须记录阶段。binary空载荷缺版本及外层帧截断按协议错误08P01验收；仅版本01而没有JSON是22P02，版本00/02/ff必须拒绝。未知版本在REL_18_0的jsonb_recv使用无显式errcode的elog(ERROR)，不能擅自统一成22P03；其诊断/SQLSTATE以精确参考构建采集为准。合法版本后尾随垃圾也必须拒绝，不能截取首个值。协议帧长度与恢复归工作流7；尚未支持的codec沿0A000拒绝，不把该临时错误当作最终PG非法输入诊断。

官方依据（2026-10-23查阅）：PostgreSQL **18** §8.14/§8.14.1 [JSON Types](https://www.postgresql.org/docs/18/datatype-json.html) 支持重复键、数字、Unicode与输入输出目标；精确wire核对 **REL_18_0** [jsonb.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/jsonb.c)::jsonb_in/jsonb_recv/jsonb_out/jsonb_send，明确版本1加文本；[jsonb_util.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/jsonb_util.c)::lengthCompareJsonbString/lengthCompareJsonbPair/uniqueifyJsonbObject 支持键排序及最后值；[pg_type.dat](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_type.dat)确定3802及变长。§54.7 [Message Formats](https://www.postgresql.org/docs/18/protocol-message-formats.html) 支持格式、长度/NULL与描述字段；[pqformat.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/libpq/pqformat.c)::pq_getmsgint 支持缺版本读取的协议错误边界，[附录A](https://www.postgresql.org/docs/18/errcodes-appendix.html)支持SQLSTATE分类。以上为静态规范依据，18.6仍是来源目标，未核实该版本实际运行行为。

#### 基础数组契约

基础数组是带元素类型身份的矩形多维值，保存维数、各轴长度和有符号下界、按最后一维变化最快排序的元素及逐元素NULL标记；数组本身SQL NULL另存。`integer[]`与`integer[][]`同属int4数组类型，维数属于值，不形成不同OID，声明大小不充当运行时约束。基础范围采用前述内建标量族，逐族只有身份、输入输出与保真已获证后才开放其数组；不能因标量存在即承诺整个数组族。int4/数组OID为23/1007，text为25/1009，bool为16/1000，bytea为17/1001，int2为21/1005，int8为20/1016，json为114/199；其他族开放前核对官方目录，不能按OID加减推算。RowDescription使用数组OID、typlen=-1；本块无修饰例typmod=-1，有修饰元素按该族typmod传递，不编码维数。内部头的OID是**元素OID**，不能填数组OID。

默认下界1；`[0:1][-2:-1]={{1,2},{3,4}}`保存shape=[2,2]、lower=[0,-2]，不能重排或改为从1起。空数组规范化为零维、零元素，text为`{}`、binary头ndim=0；不要求保留输入零长度轴的虚构下界。它仍有元素类型且不等于数组NULL或含NULL的非空数组。NULL数组无形状载荷；`{NULL}`是一元素非NULL数组。同一轴长度须一致，不能补NULL修复参差形状。REL_18_0最大维数6；维数、维长乘积、边界加法和分配大小须检查溢出，不能截断。有类型空数组不从首元素推断，裸空构造的定型交第2章和转换块。精度、舍入、时区和特殊值沿元素族契约，数组容器不额外降低精度；容器本身无数值精度/时区。

| 方向 | 类型、值、NULL与元数据目标 | 现状差距与错误责任 |
| --- | --- | --- |
| PG text参数→规范数组 | 声明数组OID确定元素输入规则及typmod；花括号、可选`[l:u]...=`前缀共同确定矩形形状；外层长度-1是数组NULL。 | 普通Parse白名单无数组；不能绕为text/JSON成功。未补齐保真前0A000；输入词法/形状与元素错误分层报告。 |
| PG binary参数→规范数组 | 按下述网络字节序头、维度和元素载荷读取；外层-1无头，内层-1仅对应元素NULL。数组OID决定期望元素OID。 | binary_parameter缺通用分支；目录接线不可当通用参数入口。非法头、类型与长度按下述分类拒绝。 |
| 规范数组→PG text结果 | 由规范形状生成花括号和必要下界前缀，逐元素调用该族text输出再数组转义；格式0，稳定数组OID/-1/typmod。 | 现有value_text透传目录字符串不足；空集、首行NULL、全NULL均不得降为text25。 |
| 规范数组→PG binary结果 | 同一规范值按维度/下界输出头并逐元素使用元素binary编码；格式1，元数据与text一致，数组NULL外层-1。 | binary_array仅目录一维/固定下界；不能先扁平为目录字符串再丢失形状、下界、元素NULL。 |
| cast/推导与MySQL桥接 | 数组共同元素类型及隐式/显式资格先按PG决策；允许的逐元素cast保留形状、下界和NULL，失败不返回部分转换结果；元素精度/溢出沿前述类型族。 | text显式输入按目标数组解析；JSON数组、MySQL typed array/vector、目录vector不自动等价。缺保真明确0A000，Go/MySQL原行为不变；完整cast资格由后续转换块细化。 |

文本codec固定UTF-8、`array_nulls=on`作为基本验收前提（其他会话配置先明示支持或拒绝，状态归工作流5）。未引用且未转义的NULL不区分大小写，表示元素NULL；`"NULL"`、转义形成的NULL字样是字符串，`""`是空字符串。含分隔符、花括号、双引号、反斜线、空白或NULL字样的元素输出须双引号包围，内部双引号/反斜线用反斜线转义；逐元素按类型的typdelim处理，基础例用逗号，不把所有PG类型都假定为逗号。SQL字符串字面量的词法转义与数组文本转义是两层：Bind载荷不再做SQL反转义。引号不闭合、残缺转义、缺元素、尾逗号、参差子数组及维度前缀不匹配拒绝22P02；元素非法文本/溢出分别沿元素族（如int4非法22P02、超域22003），非法UTF-8按编码阶段22021。完整ARRAY构造语法归工作流2，其非矩形错误阶段不能直接套用文本输入诊断。

二进制codec使用网络字节序：Int32 ndim、Int32 flags（0或1）、UInt32 element_oid；随后每维Int32 length及Int32 lower；再按行主序为每个元素输出Int32 byte_length及该元素binary载荷。内层length=-1为NULL、0可为合法空变长元素，无元素分隔符、内存对齐填充或PG磁盘varlena头。输出flags反映是否含NULL；输入flags仅允许0/1，不能臆加“flags=0必须无NULL”的参考端拒绝条件。非空int4数组`{1,NULL}`的字段序列为`1,1,23; 2,1; 4,00000001; -1`；空int4数组为`0,0,23`，仍非零字节载荷。

负ndim/非法flags目标22P03，超过6维54000；内建元素OID与期望不符42804；非法维长、维度乘积/下界溢出按参考端限制错误验收（REL_18_0 ArrayGetNItems/ArrayCheckBounds 的精确分支需采集）。元素长度<-1、声明元素长度大于剩余载荷或元素未消费完整目标22P03；不足以读取Int32头/长度的截断与外层Bind/DataRow帧不完整区分，协议读失败目标08P01，元素内部错误沿该族，不能一概归22P03。尾随垃圾须拒绝，精确阶段/码按参考构建采集；禁止忽略超出维长乘积的元素。尚未开放的合法族/codec以0A000拒绝，该临时门禁不能替代最终非法输入诊断。错误响应和Sync恢复交工作流7。

官方依据（2026-10-23查阅）：PostgreSQL **18** §8.15.1/.2/.6 [Arrays](https://www.postgresql.org/docs/18/arrays.html) 支持类型/维数、形状、引用、NULL及下界规则；精确格式以 **REL_18_0** [arrayfuncs.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/arrayfuncs.c)::array_in/array_out/array_recv/ReadArrayBinary/array_send 固定空数组规范化、头字段、元素类型/长度和错误分支，[array.h](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/utils/array.h) 固定MAXDIM=6及行主序，[pg_type.dat](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_type.dat)固定上述元素/数组OID配对。§54.7 [Message Formats](https://www.postgresql.org/docs/18/protocol-message-formats.html)支持外层格式、NULL长度与描述元数据，[附录A](https://www.postgresql.org/docs/18/errcodes-appendix.html)支持SQLSTATE名称。源码tag与大版本手册仅定义目标，18.6仍是来源指定基线，未在该补丁版本运行验证。

#### 显式隐式转换契约

本块承接前16个类型块及第2章转换语法，定义类型分析到值转换的交接。E为显式CAST/::；A为赋值（INSERT/UPDATE目标列）；I为表达式隐式coercion。资格满足I可用于I/A/E，A只用于A/E，E只用于E；同类型仍须执行目标typmod检查。存在路径不保证任意值成功，也不因两类型经text可达就自动串接转换。以下是基础族的有向矩阵，未列组合默认不开放，后续必须取证PG18的具体规则；合法但尚未实现的路径明确0A000，不能伪称参考PG拒绝。

| 源→目标与方向 | E / A / I资格 | 值域、精度、NULL与错误 | 目标身份、wire及现状差距 |
| --- | --- | --- | --- |
| int2→int4→int8→numeric→float4→float8及该链前向内建数值对；反向对 | 前向E/A/I；反向E/A，禁止I | 目标域检查；近似浮点可能损精度，不能称无损提升；numeric/float转整数按各族舍入后查界；越界22003。 | 分别21/23/20/1700/700/701；typmod由结果表达式决定，不继承源修饰符；现有MySQL数值求值仅候选。 |
| int4↔bool | 两向仅E | int4零→false、非零→true；bool→0/1；不推广到所有数值类型或条件真值。 | 23↔16；NULL仍是目标typed NULL；当前MySQL整型载体不证明bool身份。 |
| text/varchar/bpchar内部互转及同类型长度约束 | 基础互转E/A/I；长度行为须区分上下文 | 显式超长截断与赋值超长22001沿文本块，bpchar补空格/去尾空格保留；不能用UTF-8字节截断代替字符规则。 | 25/1043/1042及目标typmod；二进制可coerce不等于长度约束免检。现有cast字符串规则未证PG等价。 |
| 已定型text→bytea/整数/numeric/float/bool/date/time/timestamp/timestamptz/interval/UUID/json/jsonb/基础数组 | 输入输出通用规则允许E；禁止由此开放A/I | 调用目标输入语义，非法文本22P02或时间族22007/22008，超域按族；不得只取数值前缀。 | 目标OID/typmod沿各族；数组输入另保留shape/lower。UUID/jsonb/数组仍有表示与codec缺口。 |
| 上述基础标量/数组→text/varchar/bpchar | 通用输出转字符串允许E/A；不因此开放I；具体目录条目优先 | 使用源类型输出规则，再施加目标字符约束；bytea按自身输出格式，不能把原字节当UTF-8。 | 25/1043/1042；非字符串原身份不能因值像字符串改变。当前结果文本输出不能自动当SQL cast。 |
| date→timestamp、date→timestamptz、timestamp→timestamptz；逆向timestamp/timestamptz→date、timestamptz→timestamp | 前三条E/A/I；列出的逆向E/A | date补午夜；瞬时与墙钟转换使用会话TimeZone，丢时间/区域信息不承诺互逆；范围/特殊值沿时间块。 | 1082/1114/1184；目标p与typed NULL保留。MySQL零日期、隐含来源区域不能直接复用。 |
| timestamp/timestamptz→time；time→interval；interval→time | 前者E/A；time→interval为E/A/I；逆向E/A | 提取日内时间，interval回time的日内归约按PG参考取证，不拿MySQL duration直接映射；精度沿族。 | 1083/1186；月份/天不能错误合并为总秒；现有Duration不是PG interval。 |
| json↔jsonb | 两向E/A，禁止I | json→jsonb校验、规范化且可能失去原文；逆向只得规范文本；jsonb更严格数值/Unicode校验沿族。 | 114↔3802，SQL NULL不同于JSON null；现有BinaryJSON不是任一方向的充分证据。 |
| T[]→U[]（本章已列基础族） | 由元素T→U在相同上下文的资格决定，不因容器可表示放宽 | 逐元素检查，保留维数、维长、下界及元素NULL；单元素失败拒绝整个值，无部分结果。 | 目标数组OID及其元素OID一起改变，typmod沿元素约束；现有JSON typed array不是PG数组。 |
| bytea↔其他非字符串族、UUID↔bytea、任意未列跨族/MySQL桥接 | 本块不新增资格；不能由相同字节宽度或目录OID推导允许 | PostgreSQL18已有局部路径（如整数↔bytea）须单独取证后开放；MySQL unsigned可证提升到numeric，零日期/负或长TIME无等价路径则拒绝0A000。 | 禁止只换OID；缺合法路径时类型分析拒绝，缺实现时能力门禁。精度/时区不适用的固定字节族不人为添加舍入。 |

矩阵各行共同NULL契约：先检查类型资格，再对合法严格转换传播SQL NULL；已定型的非法类型对不能因值为NULL绕过资格检查。unknown NULL可按上下文定型，typed NULL保持目标OID/typmod，不执行值解析；数组NULL、元素NULL、JSON null分别沿各族。本矩阵不是PG全量cast目录；pg_cast之外还有I/O、数组等通用规则，不能以目录无条目认定不存在。

unknown字面量、未定型NULL、Parse未指定OID参数是不同来源，必须保留来源信息。字符串字面量初始unknown，`'1'::int4`直接调用int4输入；`'1'::text`已定型，不能继续当unknown隐式转数值。OID0只表示客户端未指定，Parse分析可由`$1::int4`或目标列确定23，不能读取Bind值猜测。重复$n是同一个参数身份；首次上下文推定类型后，后续引用检查一致性或插入合法coercion，禁止每次引用各自解码。显式OID先固定源类型，再检查上下文转换。无法确定类型目标42P18，不一致推断42P08，具体触发优先级/错误位置须参考端采集；与普通cast失败42846、共同类型/赋值不匹配42804及运算重载失败42883分责。

共同类型推导适用于UNION/INTERSECT/EXCEPT逐列、CASE结果、VALUES列及ARRAY等构造；不能复用MySQL in_union开关代替规则。相同非unknown身份优先；全部unknown定为text，否则忽略unknown选择候选，不同类别拒绝；同类别从左向右依据单向隐式可转换性更新候选，优选类型可终止搜索，最后所有输入均须可隐式转候选。CASE历史顺序以ELSE作为首候选；嵌套UNION逐对解析，不能把全树一次提升。domain不属于本轮开放范围，遇到时能力门禁，不能冒充基础类型支持。运算符/函数重载的候选选择归工作流4，接收本块资格结果而非套用集合算法。

方向责任：parser（工作流2）保留cast目标、参数编号及原位置；类型分析拥有E/A/I资格、共同类型及参数约束；常量折叠可在准备期触发非法值，不能保证所有转换错误延后Execute。Bind codec只按确定参数身份解码，格式0/1不改变SQL类型；求值转换负责目标值域、舍入后溢出与时区；结果codec按固定结果身份编码，不再推导cast。ParameterDescription返回推定参数OID，RowDescription返回分析结果OID/typmod；零行、全NULL、换Bind值以及两种输出格式均不得改变描述，详细生命周期交批次18。PG错误必须拒绝语句，不沿MySQL警告后继续返回裁剪值；MySQL原Go语义保持独立。错误响应/Sync恢复归工作流7；P若提供按原SQL字符位置，精确值及错误阶段均待差分。

方案比较：直接复用共享转换器可减少实现，但现有截断、unsigned、decimal钳制和MySQL日期路径已有反例，不能证明PG资格与值语义等价。选择“连接方言与E/A/I先决策，再复用已证实等价的局部转换”；可候选复用NULL传播、合法定宽整数值操作等，但须把类型检查、PG范围错误与元数据连续性放在调用边界，逐项差分证明后采用。只补当前类型族必需信息，不重建类型系统，不改既有Go行为。

官方依据（2026-10-23查阅）：PostgreSQL **18** [§10.1](https://www.postgresql.org/docs/18/typeconv-overview.html)支持unknown与上下文分类，[§10.4](https://www.postgresql.org/docs/18/typeconv-query.html)支持赋值和typmod，[§10.5](https://www.postgresql.org/docs/18/typeconv-union-case.html)支持共同类型顺序，[PREPARE](https://www.postgresql.org/docs/18/sql-prepare.html)支持参数上下文推定；[§52.10 pg_cast](https://www.postgresql.org/docs/18/catalog-pg-cast.html)定义E/A/I层级和目录非穷举性。具体方向以 **REL_18_0** [pg_cast.dat](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_cast.dat)的numeric、bool、string、datetime、json条目为静态依据；通用I/O与数组资格已核对[parse_coerce.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/parser/parse_coerce.c)::find_coercion_pathway的字符串I/O与元素递归分支。[§8.1](https://www.postgresql.org/docs/18/datatype-numeric.html)支持数值范围/舍入，[附录A](https://www.postgresql.org/docs/18/errcodes-appendix.html)支持错误分类。18.6是来源指定参考目标，本轮未运行该版本或本地SQL；以上均为文档目标，非功能通过声明。

#### 准备描述执行元数据契约

本块承接各类型族及[显式隐式转换契约](#显式隐式转换契约)，固定规范身份→参数/结果描述→codec的责任。Prepare在此指共享准备入口及分析阶段；SQL PREPARE不是wire Parse消息，不能据名称相同推定两种入口均可用。声明类型、参数约束和表达式分析决定类型，不用首行、NULL、值大小或MySQL列名猜测。值域、精度、舍入、时区和数组形状继承对应族；本块不新增值域。typlen是类型固定长度或负的变长标记，不是当前载荷字节数；typmod是类型专用修饰符，不是OID或格式码。

| 方向/阶段 | 类型、NULL、值域与wire目标 | 现状差距及拒绝责任 |
| --- | --- | --- |
| Parse/Prepare→定型 | 显式非0参数OID作为输入约束；省略或0交分析按上下文推断。相同$n的所有出现约束须一致，参数按编号描述，不能按出现次数重复。结果列数/顺序/名称、OID、typlen、typmod由分析结果确定。无值阶段NULL载荷不适用，typed NULL表达式仍有身份。 | 普通路径目前要求显式OID且桥接只暴露参数数量。目标不能推断时拒绝42P18，约束冲突按实际语义诊断；合法但尚未开放的推断明确0A000，不伪称参考端也拒绝。不得执行原查询或样本查询取得类型。 |
| statement Describe→客户端 | ParameterDescription仅含参数数量及OID，无参数typlen/typmod/格式字段；随后RowDescription含结果列数及每列身份/typlen/typmod，格式恒0。无返回行的命令给NoData；返回零行的SELECT仍有描述。值/载荷/值域检查不适用，因为没有绑定值。 | 当前description复用零行QueryResult编码；固定-1不证明声明精度保真。缺类型须显式拒绝，不能伪造text或NoData。 |
| Bind→portal | 参数数量匹配定型参数；输入格式0/1独立于结果格式。NULL长度-1无载荷，按已定型身份保留，不改OID；非NULL按该族值域及codec校验。参数格式数量0/1/N，结果格式数量0/1/列数，0项默认text，1项广播，否则逐项。绑定值只改变值或计划选择。 | tp=6提前返回不能证明typed NULL跨层保真；无支持的binary codec明确拒绝0A000，非法text/binary分别按族22P02/22P03等，数量/格式码错误08P01；不退回text。 |
| portal Describe→客户端 | 只返回RowDescription或NoData，不返回ParameterDescription；身份/列数与statement一致，格式按该portal Bind选择，可逐列混合。typlen/typmod不随格式改变；NULL和值域不参与描述推断。 | Portal已有独立快照；须证明两个portal格式互不污染，不能把statement的0格式原样当binary描述。 |
| Execute→结果codec | 按portal固定类型/列数编码各行，NULL两格式均-1，零行仍保持已知描述。扩展Execute本身不再发送RowDescription；对照portal Describe与实际载荷，不要求不存在的Execute描述包。重复Execute消费同一portal进度；新Bind才提供另一组值。 | 当前编码后比对描述提供局部保护，仍须证明缺类型、行宽、OID、typlen、typmod变化在发送任何DataRow前拒绝。无参数回填只能使用可信准备身份，不能遮蔽真实类型变化；参数化投影必须有明确身份连续性。 |

合法换Bind值、首行/全NULL、零行、text/binary、值相关计划选择均不得改变类型身份。DDL或解析环境变化可能触发重新分析；固定结果的准备语句若结果类型/typmod/列数变化应拒绝0A000，不能静默替换已描述形状；重规划本身不保证出错，也不能承诺永远不重绑定名称。对象已不存在等情况按实际诊断，不统一包装成元数据变化。缺规范类型属于AsterSQL能力拒绝0A000，不是PG对合法SQL的预期错误；具体报错阶段须参考端采集。工作流7拥有消息顺序、错误传递、Sync恢复与portal生命周期，工作流5/6拥有事务/目录失效；本块只固定类型连续性及拒绝边界。MySQL Prepare及原Go行为独立保留，不能把PG OID直接当MySQL类型标志。

官方依据（2026-10-23查阅）：PostgreSQL **18** §54.2.3 [Extended Query](https://www.postgresql.org/docs/18/protocol-flow.html#PROTOCOL-FLOW-EXT-QUERY) 支持Parse/Bind/两种Describe与Execute的分责；§54.7 [Message Formats](https://www.postgresql.org/docs/18/protocol-message-formats.html) 的Parse、Bind、ParameterDescription、RowDescription、DataRow支持上述字段、格式数量及NULL长度。推断失败以 **REL_18_0** [postgres.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/tcop/postgres.c)::pg_analyze_and_rewrite_varparams为依据；固定结果变化拒绝以同tag [plancache.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/cache/plancache.c)::RevalidateCachedQuery为依据。手册与源码定义目标，18.6仍是来源指定基线，没有该补丁版本或本地协议实测。

#### 章节验收与后续交接

本块只收束文档。以下索引与各族方向表、验收条目共同组成完整矩阵，不以索引行代替其精确值域、精度、SQLSTATE或二进制格式。来源18.6仍是指定目标，未取得该精确构建的运行证据；所有能力当前为**未验证**，源码与独立测试只作静态入口。

| 来源步骤 | 累计覆盖位置 | 收束条件 |
| --- | --- | --- |
| 1：双向映射、推导、NULL | 规范类型与协议映射；准备描述执行元数据契约 | 参数类型、规范身份、结果描述及MySQL标志分别核对，TYPE-MAP与META互相引用；OID映射不是支持证明。 |
| 2：全部类型族 | 布尔至基础数组的15个类型族块（整数含三个宽度，文本含三种身份） | 逐族按下表展开四方向、边界及拒绝目标，目录专用类型不能替代通用类型。 |
| 3：cast及不等价边界 | 显式隐式转换契约及各族桥接段 | CAST条目覆盖E/A/I、精度溢出、时区、数组维度/下界；unsigned、零日期、duration保留双方言差异。 |
| 4：验收矩阵及生命周期 | 各族验收表、TYPE-MAP、META | 四方向与NULL/边界/非法输入均有目标、错误和描述断言；消息状态与恢复交工作流7。 |

为每个下表类型建立能力ID `TYPE-ACCEPT/<族>/<IT|IB|OT|OB>/<VALUE|NULL|EDGE|INVALID>/pg`，这是**待新增登记约定**，不是既有测试名或当前通过记录。IT/IB为text/binary参数输入，OT/OB为text/binary结果输出。每格继承对应族列出的身份、值域/精度与错误；四方向均须记录参数OID或结果OID/typlen/typmod、实际格式、预期/实际值和SQLSTATE，不能留白。输入NULL固定外层-1且保留已定型身份；输出NULL同为-1，另跑首行NULL、全NULL和零行META场景。EDGE同时验上下端/特殊值；INVALID在输入测非法文本、长度、越域，在输出用受控非法内部值/缺身份夹具（待新增）验证拒绝，不能通过构造非法SQL冒充输出检查。无成功结果的拒绝场景，结果载荷和成功列元数据写“不适用（拒绝且无DataRow）”，仍断言拒绝前未发送成功结果。成功SQLSTATE写“不适用（成功）”。

| 族与既有验收索引 | IT正例 / 反例 | IB目标 / 非法输入 | OT / OB目标与边界 | 当前静态入口与缺口 |
| --- | --- | --- | --- | --- |
| bool；BOOL | true→真 / unknown拒绝22P02 | 02→真 / 超长拒绝22P03 | t / 01；16/1/-1，false及NULL独立 | pg_types_test、pg_extended_test有局部断言；四方向全集未验证。 |
| int2/int4/int8；INT | 1 / 各宽度上限+1拒绝22003 | 网络序定宽 / 短载荷按族长度错误 | 十进制 / 2、4、8字节；21/23/20及对应typlen，typmod=-1；负端与unsigned提升 | pg_types_test、pg_extended_test；宽度/unsigned binary及typed NULL缺证，未验证。 |
| numeric；NUM | 12.340 / 12bad拒绝22P02 | base-10000格式 / 非法头或digit拒绝 | 精确保真文本 / numeric格式；1700/-1/族typmod；舍入、负scale、大精度及特殊值 | mydecimal_test、pg_types_test；共享Decimal域与PG目标不同，未验证。 |
| float4/float8；FLOAT | 1.5 / 域外数值拒绝22003 | 4/8字节位型 / 错长度拒绝 | 目标宽度可逆文本 / 网络位型；700/701、4/8、-1；±0、无穷、NaN | pg_types_test局部binary断言；窄化及全边界未验证。 |
| text/varchar/bpchar；TEXT | é / NUL或非法编码拒绝22021 | 编码字节 / 同一编码拒绝 | 编码文本 / 同一字节；25/1043/1042及声明typmod；空串、长度/尾空格与22001 | pg_types_test、pg_extended_test；声明身份与长度保真未验证。 |
| bytea；BYTEA | hex 00ff / 奇数hex拒绝22P02 | 原始00ff / 帧截断拒绝08P01 | 选定bytea_output / 原始字节；17/-1/-1；空字节非NULL | pg_extended_test；escape与设置组合未验证。 |
| date；DATE | 2024-02-29 / 2026-02-29拒绝22008 | 四字节日序 / 超日期域22008 | DateStyle文本 / 日序；1082/4/-1；BC、有限端点、±infinity，零日期拒绝 | pg_types_test、pg_extended_test；完整日历域及binary输入未验证。 |
| time；TIME | 24:00:00 / 负duration拒绝 | 微秒日内域 / 越域拒绝 | 日内文本 / 微秒；1083/8/精度typmod；p舍入及24时边界 | pg_extended_test当前反而拒绝24时；完整目标未验证。 |
| timestamp；TS | 含6位小数的墙钟 / 零日期拒绝 | 微秒墙钟 / 越域拒绝 | 无时区文本 / 微秒；1114/8/精度typmod；BC、特殊值、舍入 | pg_types_test、pg_extended_test；扩展域及精度连续性未验证。 |
| timestamptz；TZ | 两种偏移同一瞬间 / 非法区域拒绝 | 瞬时微秒 / 越域拒绝 | 会话区域显示 / 同一瞬时；1184/8/精度typmod；DST、特殊值 | 前序TZ模块静态入口；通用瞬时表示与四方向未验证。 |
| interval；IV | 1 month 1 day / 非法单位拒绝 | 微秒/日/月三分量 / 非法长度拒绝 | IntervalStyle文本 / 三分量；1186/16/fields与p typmod；混合符号、非有限及溢出 | 前序IV模块静态入口；不得折为总秒，四方向未验证。 |
| uuid；UUID | 合法128位文本 / 非hex拒绝22P02 | 16字节 / 错长度拒绝 | 规范小写文本 / 原字节；2950/16/-1；全0及全ff，无数值精度 | 前序UUID模块静态入口；专属身份与通用codec未验证。 |
| json；JSON | 保留重复键原文 / 非法语法拒绝22P02 | JSON文本字节 / 非法编码拒绝 | 原文 / 同一原文；114/-1/-1；JSON null与SQL NULL分开，大数原文保留 | 前序JSON模块静态入口；MySQL BinaryJSON不能证明原文保真，未验证。 |
| jsonb；JSONB | 重复键取最后值 / 非法Unicode拒绝 | 版本1加文本 / 错版本拒绝 | 规范语义文本 / 版本1加文本；3802/-1/-1；numeric域和scale | 前序JSONB模块静态入口；精确值与版本codec未验证。 |
| 基础数组；ARRAY | 非1下界矩形数组 / 参差拒绝22P02 | 维度/元素OID/逐元素载荷 / OID不符42804 | 带必要下界文本 / 维度头与元素codec；数组OID/-1/元素typmod；空、数组NULL、元素NULL分开 | 前序ARRAY模块及目录局部测试；通用shape/lower保真未验证。 |

表中未重复的精确错误分支、有限端点和格式字段全部按对应族正文及验收表逐格继承，不能统一套22P02或22P03；合法但未开放的表示/codec明确0A000，不能改写PG目标为拒绝。类型族矩阵需要IT→OT、IT→OB、IB→OT、IB→OB四组合均验证同一规范值。转换另按CAST矩阵验E/A/I与typed NULL；Prepare/Describe/Execute按META七组验全链。数组逐元素族展开，标量通过不自动开放数组。MySQL关联ID用/mysql，独立保留原Go契约，不要求两端OID相等。

本轮查阅官方PostgreSQL **18** [第8章](https://www.postgresql.org/docs/18/datatype.html)确认各族规范入口，[第10章](https://www.postgresql.org/docs/18/typeconv.html)确认转换上下文分责，[§54.7](https://www.postgresql.org/docs/18/protocol-message-formats.html)确认Bind、ParameterDescription、RowDescription、DataRow字段；精确值/二进制细节继续绑定前序块的具体小节及源码版本。本块不新增二进制规范断言，不把18手册当18.6实测。

| 工作流 | 输入/输出交接与验收责任 |
| --- | --- |
| 2：方言身份与解析边界 | 输入连接方言、类型名称、cast/数组AST、$n编号；本章返回类型与转换约束，语法/名称拒绝仍归2。 |
| 4：方言函数与运算符契约 | 接收类型、共同类型及cast资格；负责重载、运算、返回类型及算术错误，不由codec决定运算语义。 |
| 5：SQL会话与事务语义 | 提供TimeZone、DateStyle、IntervalStyle、bytea_output、编码及事务时间；负责状态/失效，本章验配置下的值与元数据。 |
| 6：系统目录与内省契约 | 提供类型/对象身份及修饰符、失效信息；目录特例不能证明通用族，目录查询形状归6。 |
| 7：线协议认证与数据通道 | 接收已定型身份、参数/结果codec与SQLSTATE目标；负责帧、消息顺序、portal、错误传递和Sync恢复。 |
| 8：客户端与双方言共存验收 | 接收以上能力ID及四方向/边界用例；锁定参考构建、客户端和配置，按第1章采集差分及MySQL独立回归证据。 |

### 范围外

章节验收与后续交接不运行数据库或测试、不建立差分平台，不修改工作流2/4/5/6/7/8正文，不以文档验收承诺完整支持；未开放类型与扩展生态保持原范围。

[规范类型与协议映射](#规范类型与协议映射)只定义映射与交接，不实现生产代码、类型系统、完整 cast、codec、数据库环境或差分平台。逐族输入语法、舍入/时区、数组形状及精确二进制布局交本章后续文档块，不因列出 OID 提前承诺支持。parser/名称绑定归工作流 2；运算重载归 4；时区等会话状态归 5；目录类型身份归 6；消息状态机、格式选择、错误响应和恢复归 7；真实客户端与双方言共存证据归 8。扩展、自定义类型及目录专用类型向通用引擎推广均不在本块范围。

[布尔类型契约](#布尔类型契约)不实现存储或类型系统，不改 Go/MySQL 真值行为；不扩展为 bool 数组、完整 cast 矩阵、运算符/聚合或协议状态机。声明解析、运算、消息恢复分别交工作流 2、4、7；本块只固定类型与 codec 的可观察交接。

[整数与无符号边界](#整数与无符号边界)不实现算术、cast 或 numeric codec，不扩写聚合、serial/identity、oid、数组及完整类型推断。工作流 2 提供声明/转换语法；4 拥有重载求值；7 拥有帧校验、错误响应和恢复；8 采集客户端与双方言证据。MySQL 原行为不因 PG 目标更改。

[精确数值契约](#精确数值契约)不实现大数表示、cast 或 codec，不改 MySQL/Go 裁剪及告警行为；不扩大到算术/聚合、索引排序、numeric 数组或协议状态机。工作流 2 接声明语法，4 接数值运算与特殊值比较，7 接消息/错误响应及恢复，8 接客户端差分。

[浮点数值契约](#浮点数值契约)不实现内部表示、cast、codec 或运算，不改 Go/MySQL 的浮点行为，不扩展浮点数组、索引、聚合或完整推导系统。类型语法归工作流 2，比较/算术归 4，输出会话配置归 5，消息错误与恢复归 7，真实客户端与双方言差分归 8。

[文本类型契约](#文本类型契约)不实现生产表示、cast、codec 或 MySQL 行为修改；比较、排序、collation 算法、LIKE/正则交工作流 4，编码会话状态交 5，目录专用类型交 6，协议分帧/错误恢复交 7，双方言客户端差分交 8。此处仅交接语义，不扩建子系统。

[字节类型契约](#字节类型契约)不实现生产表示、cast、codec，不改 MySQL BIT/HEX 或 Go 原行为。字面量解析归工作流 2，函数/比较归 4，bytea_output 状态归 5，帧长度/错误恢复归 7，客户端差分归 8；large object/lo、bytea 数组和扩展生态不在本块范围。

[日期类型契约](#日期类型契约)不实现生产表示、转换或 codec；日期字面量语法归工作流 2，日期算术/比较归 4，DateStyle、事务时间和 TimeZone 状态归 5，消息恢复归 7，客户端差分归 8。date 数组和其他时间族在各自块承接，不为扩展年份缺口扩建存储或执行子系统。

[日内时间类型契约](#日内时间类型契约)不实现生产表示或 codec，不增加 timetz、time 数组及完整时区数据库能力。字面量/类型语法归工作流 2，算术/比较归 4，TimeZone 与事务时刻归 5，目录身份归 6，消息状态恢复归 7，客户端差分归 8；本块只交接类型与拒绝契约。

[无时区时间戳契约](#无时区时间戳契约)不实现生产表示、cast、codec 或 MySQL 会话行为；timestamp 数组、完整时间运算和时区数据库不在本块。语法归工作流 2，算术/比较归 4，DateStyle/TimeZone/事务时间归 5，目录身份归 6，消息及错误恢复归 7，真实客户端差分归 8；timestamptz 独立承接，不能因 1114 映射扩大承诺。

[带时区时间戳契约](#带时区时间戳契约)不实现生产表示、cast、codec、时区数据库或完整时间运算；数组另块承接。会话状态与配置生命周期归工作流 5，语法归 2、算术/比较归 4、目录归 6、消息/恢复归 7、真实差分归 8；不扩大为完整时区子系统。

[时间间隔契约](#时间间隔契约)范围外：日期/时间戳加减 interval、比较、justify、age、extract 等函数/运算由工作流 4 承接；IntervalStyle 状态生命周期由工作流 5、SQL 声明解析由工作流 2、消息恢复由工作流 7 承接。本块只定义表示、转换及 codec 契约，不新增函数子系统，不改变 MySQL TIME 既有行为。

[通用唯一标识契约](#通用唯一标识契约)范围外：UUID生成、版本提取、排序/索引及函数运算归工作流4，目录曝光归工作流6，解析归工作流2，消息恢复归工作流7；UUID数组交基础数组块。本块不新增生成器或更改Go/MySQL UUID函数语义。

[JSON文本契约](#JSON文本契约)范围外：JSON操作、路径、构造/聚合、取值时重复键处理及Unicode解码归工作流4；jsonb规范化交下一块，数组交基础数组块。语法/定型接口归2，编码会话状态归5，目录身份归6，协议错误恢复归7，真实客户端差分归8。本块不实现生产代码、不改Go/MySQL函数、不新建可执行实现任务。

[JSONB规范值契约](#JSONB规范值契约)范围外：运算符、比较/包含、路径、索引、聚合与扩展生态归工作流4或来源范围决策；本块不实现这些能力。JSON原文由前块负责，SQL数组由基础数组块负责；语法定型归2，编码会话配置归5，目录身份归6，协议恢复归7，客户端差分归8。不扩建生产子系统，不生成执行任务。

[基础数组契约](#基础数组契约)不实现生产表示、cast或codec，不扩展自定义/复合/domain数组、下标/切片、比较/运算、索引、聚合或完整存储子系统；目录vector和ACL编码保留独立责任，不推广为普通数组。构造语法归工作流2，运算归4，array_nulls/编码状态归5，目录身份归6，消息及恢复归7，真实客户端差分归8。只定义本章类型与codec交接，不修改Go/MySQL行为。

[显式隐式转换契约](#显式隐式转换契约)只交付上述基础族转换资格与责任；完整重载、domain/用户定义cast、扩展类型、函数/算术、存储、优化器及协议状态机均不实现。bytea与整数等PG18新增局部转换不因本块列出而开放；不补建全量cast目录，不修改其他章节或Go行为。

[准备描述执行元数据契约](#准备描述执行元数据契约)不实现准备器、参数推断、缓存失效、codec或协议状态机，不扩建planner/执行器；SQL PREPARE/EXECUTE语法、事务及目录变更实现分别交工作流2、5、6，消息/恢复交工作流7。不将元数据验证推广为无副作用执行或完整缓存兼容。

### 验收

章节验收与后续交接要求逐项核对来源四步、15族四方向及VALUE/NULL/EDGE/INVALID格，沿各族验收表填精确目标、正反例、SQLSTATE、元数据、静态入口与缺口；所有格当前运行状态为未验证。TYPE-ACCEPT登记后按第1章记录提交、精确版本、环境、完整命令/退出码、有效用例数、原始产物、预期与实际；无运行用例不得填通过。META补零行、全NULL、重绑定、混合格式、typmod、缺元数据与不执行探测。文档验收另核对五公共小节、本块唯一标题、引用存在、无占位和仅第3章差异；这些检查只证明文档完成。

[规范类型与协议映射](#规范类型与协议映射)的以下能力 ID 为**待新增**验收入口，非既有测试或功能通过记录。每项按第 1 章记录方言、定型前提、输入、类型、NULL、wire 元数据、错误与当前证据；本轮全部运行行为为未验证。成功项 SQLSTATE 不适用；错误目标参照 PostgreSQL 18 [附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html)，不以现有 io::Error 自动推导其正确传输。

| 入口与场景 | 可观察目标与反例 | 当前证据状态/责任 |
| --- | --- | --- |
| TYPE-MAP-IDENTITY：同值不同类型 | PG 定型 `1::int2`、`1::int4`、`1::int8`、`1::numeric` 及 TRUE 保持各自 OID/typlen/typmod；text '1' 不变整数。两种格式的身份一致。 | 既有 boolean bridge 测试只断言 MySQL 同列同值及 B 差异；完整场景待新增、未验证。本章定型，工作流 7 描述。 |
| TYPE-MAP-EMPTY：空结果 | `SELECT 1::int4 AS v WHERE false` 返回零行但列为 23/4/-1；Prepare/statement Describe 与 portal Describe/Execute 的类型相同，格式字段按实际模式分别验收，不能要求 text/binary 格式码相等。 | 待新增、未验证；缺元数据即使零行也拒绝，不依赖首行。 |
| TYPE-MAP-NULL：全 NULL | `SELECT NULL::int4` 返回 23/4/-1、DataRow 长度 -1；typed NULL 参数在 Bind 前后仍保留 int4 身份；空 text/bytea 与 SQL NULL 区分；JSON null 独立。 | 既有 NULL JSON 拒绝断言不能证明此场景；待新增、未验证。不确定裸 NULL 类型先按 PG 上下文取证，不能沿当前 code 6 回退视为通过。 |
| TYPE-MAP-UNSIGNED：提升 | 共享引擎/MySQL U16/U32/U64 边界，分别得到 int4/int8/numeric；u64 最大值不丢位，PG numeric 反向窄化须检查目标 unsigned 域。MySQL 描述仍保留 U。 | unsigned_binary_and_unsupported_types 有 u64 文本断言；Prepare、一致性、binary numeric 与反向边界待新增、未验证。PG 窄化溢出目标 22003，禁止截断或浮点提升。 |
| TYPE-MAP-UNKNOWN：未知或缺类型 | 直接构造未知 code、native 数量不匹配及缺必要身份；分别覆盖有行、零行、全 NULL，均不得发成功描述或猜 text；未知 PG 类型入口拒绝，已知类型但未实现 codec 明确不支持。 | 既有 missing_original_metadata 测试覆盖缺类型的有行边界，JSON 非支持含 NULL 有断言；其余待新增、未验证。项目不支持路径目标 0A000，非宣称参考 PG 也拒绝该类型。 |
| TYPE-MAP-DIRECTIONS：参数/推导/结果 | 每个已开放族跑 text/binary 参数→规范值→结果；OID 声明与上下文共同决定身份，OID 0 无法推断则拒绝，目标 42P18；反例为同载荷不同 OID、非法 text、错误 binary 长度及范围越界。 | 参数身份及 typmod 往返待新增、未验证；错误目标依原因分别 22P02、22P03、22003，字符编码 22021；具体日期等族细化错误另取证，不一律套 22P02。 |
| TYPE-MAP-NONISOMORPHIC：不可逆域 | 零日期、负/超过一天的 duration、缺时区瞬间、JSON 身份折叠、空数组无元素身份、未知数组下界不得静默包装；有明确转换才能按转换目标返回。 | 既有测试有零日期和 25:00:00 拒绝意图；其余待新增、未验证；不支持范围目标 0A000，非法已支持域的输入按各族数据错误，MySQL 原行为单独对照。 |

文档完成条件是五个公共小节均能关联本块，上述方向/类型族及五项必需边界（同值异类型、空结果、全 NULL、unsigned、未知类型）无缺项，当前事实、目标和缺证分开。文档结构通过不等于 Describe/Execute、codec、cast、SQLSTATE、双方言隔离或性能通过；本轮没有当前客户端、差分记录或有效运行用例数。

[布尔类型契约](#布尔类型契约)验收入口以下均为**待新增**，不是已执行用例；当前运行证据全部为“未验证”，成功项 SQLSTATE 不适用。每项同时采集 AsterSQL 与精确 PG 参考版本、值/NULL、类型、格式和错误；错误恢复交工作流 7，不能以 io::Error 或测试名称代替 wire 错误断言。

| 能力 ID / 场景 | 可观察目标与反例 | 已读证据与缺口 |
| --- | --- | --- |
| BOOL-IDENTITY | SELECT TRUE,FALSE,NULL::boolean；声明 bool 列含 true/false/NULL，并并列 int4 值 1/0/NULL；bool 16/1/-1，整数 23/4/-1，不能同值改型。 | common_type_oids_and_values 与 bridge 测试只覆盖部分字面量；声明列/typed NULL 待新增，未验证。 |
| BOOL-TEXT | OID 16 参数逐测完整合法词、前缀、大小写和空白；空串、o、2、truth、unknown 拒绝 22P02，非法编码 22021；输出固定 t/f。 | text_parameters_are_typed_and_reject_loss 仅 true/unknown；其余待新增，未验证。 |
| BOOL-BINARY | OID 16 载荷 00/01/02/ff 依次 false/true/true/true，结果规范化 00/01；长度 0 为 08P01，长度 2 为 22P03。非法二进制“值”反例是拒绝 02/ff 的错误实现；带 bool 身份的内部值 2 则应拒绝。 | pg_binary_native_results 只含 false 输出；输入边界及内部非法值拒绝待新增，未验证。 |
| BOOL-NULL-DESCRIBE | OID 16 的 text/binary NULL 参数、首行/全 NULL 及零行 bool 结果；ParameterDescription 为 16，statement/portal Describe 与 Execute 列身份始终 16/1/-1；NULL 长度 -1，空载荷独立拒绝。 | 参数 tp=6 与准备/执行桥接尚缺身份连续性证据；待新增，未验证。 |
| BOOL-CAST | 0::int4::boolean=false，2/-1::int4 显式转为 true；TRUE::int4=1，FALSE::int4=0；TRUE::text='true'；NULL 转换保留目标身份；WHERE 1 与整数赋 bool 列拒绝 42804。 | Datum::ToBool/test_to_bool 是 MySQL 意图，不能证明 PG；完整显式/隐式及双方言隔离待新增，未验证。 |

[整数与无符号边界](#整数与无符号边界)以下入口均为**待新增**；所有运行行为未验证。成功项 SQLSTATE 不适用，失败须断言具体 SQLSTATE，不以 is_err 代替。

| 能力 ID / 场景 | 可观察目标与反例 | 现有断言意图/缺口 |
| --- | --- | --- |
| INT-RANGE | PG int2/int4/int8 各 min/max 成功，min-1/max+1 的 text 参数及显式窄化拒绝 22003；空串/1x/1.5 拒绝 22P02；按官方输入语法补空白、符号、进制和下划线正反例。 | text_parameters_are_typed_and_reject_loss 仅 int4 最小值和上越界 is_err；其余及错误码未验证。 |
| INT-UNSIGNED | 对表内每个 signed/unsigned 域测 min/max 与外侧一值；合法 U8/U16/U24/U32/U64 依次映射 int2/int4/int4/int8/numeric。U64 的 9223372036854775808 和 18446744073709551615 精确保留，18446744073709551616→U64 及 -1→各 unsigned 拒绝。 | test_convert 有各宽度上下界及部分裁剪错误值断言；unsigned_binary_and_unsupported_types 有最大 U64 text/OID 断言。桥接拒绝、全宽 wire 和双方言隔离未验证。 |
| INT-CODEC | 每个 PG signed 宽度对 min/-1/0/1/max 做 text/binary 参数→结果，binary 长度/网络序准确；少/多一字节分别 08P01/22P03。U16/U32 扩宽两格式精确，U64 numeric 两格式精确或未开放 binary 时显式拒绝。 | pg_binary_native_results 有 i64::MIN、-2 int2、i32::MAX 输出断言；完整往返、长度错误及 numeric binary 未验证。 |
| INT-NULL-METADATA | 每个 signed 类型、各 unsigned 提升后类型含 typed NULL 参数、首行 NULL、全 NULL、空结果、混合 min/max 行；ParameterDescription 输入身份及 statement/portal Describe/Execute 结果 OID/typlen/typmod 稳定，格式选择不改变类型。 | NULL 参数现仅 is_null 断言，不证明类型连续性；U64 的零值与最大值均须保持 1700/-1/-1，全部待新增、未验证。 |
| INT-CAST-ARITH | 所有 signed 宽度间域内 cast 保值，窄化越界拒绝；负值转 unsigned 不 wrap。int4 最大值加一/各 signed 最小值取负拒绝 22003，先显式转足够宽类型后的可表示结果成功；NULL 不改结果类型，溢出不动态改 OID。 | 共享转换测试只证明 MySQL 意图；PG 重载、显隐式资格、错误码与元数据全链未验证，算术执行交工作流 4。 |

[精确数值契约](#精确数值契约)以下入口均为**待新增**，所有运行行为未验证；每项分别采集 text/binary、值与 scale、OID/typlen/typmod、NULL 和 SQLSTATE，成功项 SQLSTATE 不适用。

| 能力 ID / 场景 | 可观察目标和反例 | 已读证据与验证缺口 |
| --- | --- | --- |
| NUM-PRECISION | numeric(3,1)：±99.9 成功；±99.94 舍入为 ±99.9；±99.95 拒绝 22003。p=1/1000、s=-1000/1000 声明边界成功，p=0/1001、s=±1001 拒绝 22023；numeric(3,5) 的 .00999 成功，.009995 进位超域拒绝。 | MyDecimal 测试有 word 边界，不证明 PG 声明域；全部 PG 断言未验证。 |
| NUM-ROUND | ±1.25→numeric(3,1) 为 ±1.3；±1.249 为 ±1.2；numeric(2,-3) 的 99499/-99499→99000/-99000，99500/-99500 拒绝 22003。±1.5→int4 为 ±2；整数上下界附近舍入后超域拒绝。 | TestRoundWithHalfEven 有正负半值/负 scale，TestRoundWithTruncate 有另一模式；均未运行，不证明 PG cast。 |
| NUM-LARGE | 无约束 numeric 接收 18446744073709551615、82 位全 9 整数及 1000 位有限整数，保持精确；按 PG 实现极限验证小数点前 131072/131073 位和后 16383/16384 位的成功/22003。共享表示不足须明确 0A000 并记为兼容缺口。 | U64 仅文本断言；大精度表示、两格式往返、极限全部未验证，禁止 float8 回退。 |
| NUM-TEXT-SPECIAL | 12.340、1.25e2 精确；空串、12bad、NaNx 拒绝 22P02；非法编码 22021。NaN 保持 numeric，±Infinity 在无约束成功、constrained 拒绝 22003；当前缺表示时明确 0A000。 | 参数已有 12.340/12bad 意图；其余未验证，字符串透传不能验收特殊值。 |
| NUM-BINARY | 12.340 的字段为 (2,0,0000,3;12,3400)，结果 scale=3；U64 最大值两格式无损。构造 (2,0,0000,2;12,3456) 接收为 12.34，不能误按普通 typmod 舍入为 12.35。负值、零、特殊 sign 分别往返；非法 digit=10000/sign/scale、短载荷/额外字节分别按方向表拒绝。 | 当前 numeric binary 无实现；MyDecimal TestToBinFromBin 是 MySQL 格式，不能替代此项，全部未验证。 |
| NUM-NULL-METADATA | NULL::numeric(8,2)、typed NULL 参数、首行 NULL/全 NULL、空结果及多行不同值，均保持 1700/-1 与预定 typmod；numeric(8,2) typmod=524294，无约束/U64 为 -1。statement/portal Describe 与 Execute 身份一致，参数 OID=1700；负 scale typmod 往返不误读成正 scale。 | 既有 DECIMAL(8,2) 仅 text/OID 意图；typmod、NULL、Prepare 全链及双方言隔离未验证。 |

[浮点数值契约](#浮点数值契约)以下能力入口均为**待新增**，全部运行行为为未验证；每项分别采集两宽度、两格式、类型身份、NULL 与 SQLSTATE，成功项 SQLSTATE 不适用。codec 有限值按目标宽度位型比较，零另比符号，NaN 比类别；文本同时核对 PG 默认拼写和回读，不用统一宽容误差替代。运算另定逐操作误差模型，不沿用 codec 放宽阈值。

| 能力 ID / 场景 | 可观察目标与反例 | 已读证据与缺口 |
| --- | --- | --- |
| FLOAT-IDENTITY | 1.5::real 与 1.5::double precision 同值异型，分别 700/4/-1、701/8/-1；裸 1.5/1e0 为 numeric；float(1)/(24) 为 float4、float(25)/(53) 为 float8，float(0)/(54) 拒绝 22023。 | pg_type 有映射但无这些推导/Describe 全链断言；未验证。 |
| FLOAT-ROUNDTRIP | ±1.5、0.1、16777217 text→float4 为 16777216，float8 保留 16777217；直接 float4 输入 7.038531e-26 对照官方结果，拒绝二次舍入。默认可逆文本→同宽解码位型一致，binary 长度/字节准确。 | pg_binary_native_results 仅 1.5_f32；完整 text/binary 往返、默认拼写未验证。 |
| FLOAT-RANGE | 各宽度 ±最大有限、±最小正规及其相邻位型，±最小次正规（位型 00000001 / 0000000000000001 加符号）均成功；text 1e39→float4、1e309→float8 拒绝 22003；1e-100→float4、1e-400→float8 下溢为零拒绝。binary 对应无穷位型成功，不能按 text 溢出拒绝。 | 有网络序转换代码，无完整边界/错误断言，未验证。 |
| FLOAT-SPECIAL-ZERO | 两宽度 text/binary 的 +0/-0 符号保留，float4 binary 00000000/80000000、float8 0000000000000000/8000000000000000；NaN/±Infinity/±Inf 文本输入，输出规范词，非 NULL。NaN 与自身相等、±0 相等的 SQL 比较由工作流 4 验收。 | 仅 float8 +∞ binary 输出意图；负无穷、NaN、带符号零和比较未验证；payload 保存无承诺。 |
| FLOAT-INVALID | 空串、1x、NaNx 拒绝 22P02，非法编码 22021；float4 的 0/3/5 字节、float8 的 0/7/9 字节分别覆盖不足/额外错误；NULL 不走长度错误路径。 | 现有 binary 长度一律 22P03，与目标短载荷分类有差距；精确 wire 错误未验证。 |
| FLOAT-CAST | float4→float8→float4 保留有限位型；float8 1e39 窄化溢出、1e-100 窄化变零均 22003；±2.5→int4 在最近偶数平台为 ±2，±3.5 为 ±4；NaN/∞转整数拒绝。 | 共享转换/格式化测试不能证明 PG cast 或双方言隔离，全部未验证；类型资格交转换块。 |
| FLOAT-NULL-METADATA | 两宽度 typed NULL 参数、首行/全 NULL、空结果及跨边界多行；ParameterDescription 700/701、statement/portal Describe 与 Execute 的 OID/typlen/typmod 连续，NULL 长度 -1，格式变化不改类型。 | pg_binary_native_results 有两宽度 NULL 长度 -1 的局部输出意图；参数 NULL tp=6 与共用 f64 值盒仍不证明身份连续；完整客户端证据未验证。 |

[文本类型契约](#文本类型契约)以下 TEXT 用例族为待新增，按第 1 章登记两方言独立 case；所有运行行为**未验证**。PG 基线为 UTF-8，text/binary 参数和结果均检查，成功 SQLSTATE 不适用。MySQL 基线固定 utf8mb4、具体 collation、sql_mode 和 CHAR 取回模式，记录其独立错误码/SQLSTATE/告警，不从 PG 推导。现有 hello、NULL 和目录 char 断言仅为静态入口。

| 场景 | PG 可观察目标 | MySQL 对照与证据状态 |
| --- | --- | --- |
| TEXT-EMPTY-NULL | text/varchar(2) 空串结果长度 0；char(2) 空串成为两个空格；三类 typed NULL 长度 -1，无载荷，元数据仍保留。 | 自身空串/NULL 分开，CHAR 取回的尾空格按已记录模式验收；完整三类型两格式缺证。 |
| TEXT-MULTIBYTE | 中🙂 在 varchar(2) 成功，UTF-8 字符数 2、字节数 7；e 加 U+0301 是 2 字符，不能归一化当 1；char(3) 的中补两个空格，载荷 5 字节。 | utf8mb4 下字符限制与协议字节宽度分别断言，不复用 PG typmod；未验证。 |
| TEXT-LENGTH-CAST | 对 varchar(2)/char(2) 赋值 ab 成功，abc 拒绝 22001，ab 加一个 ASCII 空格允许裁掉；ab 后加制表符拒绝。显式 cast abc 到两者得到 ab；char(3) 输入 a 输出 a 后跟两空格。n=0、10485761 声明拒绝 22023；n=1、10485760 声明接受，不代表极大载荷已测。 | 自身 VARCHAR/CHAR 长度边界分别在严格及非严格模式采集拒绝或截断/告警，禁止用 PG cast 规则替代；全部未验证。 |
| TEXT-SPACES | text/varchar 的 a 后跟空格在两格式输出保留；char(3) 转 text/varchar 后为 a；char 比较忽略尾空格的责任交工作流 4，输出字节不先 trim。 | 分开验收 CHAR 取回和 VARCHAR 保留，以及 collation 的 PAD 属性；不能仅凭结果相同推断比较等价，未验证。 |
| TEXT-INVALID | Bind 两格式的 61 00 62、FF、C3 28 拒绝 22021；不得输出替换字符。合法多字节不能被字节截断。结果方向用构造非法内部值的候选测试检查提前拒绝与错误传输。 | NUL 的合法性、非法 utf8mb4 的错误/告警独立采集，不套 PG 拒绝；当前 PG 输入校验和结果 NUL 校验有源码证据，运行未验证。 |
| TEXT-METADATA | 声明 text/varchar(2)/char(2) 依次 25/1043/1042，typlen=-1，typmod=-1/6/6；无长度 varchar 为 1043/-1/-1，SQL char 为 1042/-1/5。检查无约束 bpchar、typed NULL、零行、重复 Execute 和两种结果格式，OID 不随值改变。 | 检查自身 type code、charset、flags、长度，不含 PG typmod。普通结果目前折 text，目录 1042 测试不能代替；全链路未验证。 |

[字节类型契约](#字节类型契约)下列 BYTEA 入口均为**待新增**，运行行为全部**未验证**；成功 SQLSTATE 不适用。固定 PG 版本、UTF-8、bytea_output，采集两格式参数→规范值→两格式结果的原字节、NULL、类型与 SQLSTATE；MySQL 单独采集 binary 类型，不套 PG 语法。

| 场景 | 可观察目标与反例 | 已读证据与缺口 |
| --- | --- | --- |
| BYTEA-EMPTY-NULL | text 空串/`\x` 与 binary 零长度均为零字节；默认 text 输出 `\x`，binary 长度 0。两格式 OID 17 NULL 参数/结果只写 -1，不变空串。 | 现有测试无完整 bytea 空/NULL 断言，未验证。 |
| BYTEA-RAW | 00、ff、c3 28、00 00 01 00、00 ff 5c 逐个往返；完整 00–ff 的 256 字节序列两格式往返逐字节相等，长度 256，不出现 ef bf bd 替换或丢前导零。binary 输入 ASCII `\x00` 保留四字节 5c 78 30 30，不能解成 00。 | 现有结果仅 00 ff[/5c]，参数仅 hex 00 ff；全字节、非 UTF-8 与完整往返缺证。 |
| BYTEA-HEX | `\xDEad`、`\xde ad`→de ad，输出 `\xdead`；`\x0`、`\xgg`、`\xa b` 拒绝 22P02。hex text 与对应 raw binary 参数结果相同。 | 当前合法空白不支持；奇数仅 is_err，无错误码，未验证。 |
| BYTEA-ESCAPE | Bind 字符 abc→61 62 63、`\000\377\\`→00 ff 5c；`\400`、`\08`、`\q`、尾部单反斜线拒绝 22P02。hex/escape 输出均回读为原字节。text 原始 NUL/非法 UTF-8 拒绝 22021，binary 对应原字节接受。 | 当前非 hex 返回 0A000 为兼容缺口；escape 配置与输出、精确错误未验证。 |
| BYTEA-METADATA | 相同 abc 载荷的 text/bytea 分别 25/17；参数描述 17，statement/portal Describe 与 Execute 均 17/-1/-1；零行、首行/全 NULL、重复 Execute、格式切换不改型。构造 metadata 与 Value::Bytes 冲突时提前拒绝，不按首行猜类型。 | 局部 OID/输出断言不能证明 charset 桥接、typed NULL 或准备全链，未验证。 |
| BYTEA-LENGTH | binary 任何合法帧长度按实际字节解释；帧声明与实际长度不符拒绝 08P01。资源上限/hex 膨胀处须检查失败，不能溢出或截断；记录实际限制后做大载荷边界差分。 | length 的 i32 转换错误入口不能证明存储/输出极限或 wire 错误码，未验证。 |

[日期类型契约](#日期类型契约)以下 DATE 入口均为**待新增**，全部运行行为**未验证**。成功项 SQLSTATE 不适用；按第 1 章记录具体参考版本、DateStyle、TimeZone、事务时间、两格式原字节、类型、NULL 与错误。每个合法值覆盖 text/binary 参数→规范值→text/binary 结果四种组合；特殊词先变规范日期再比较，不要求保留输入拼写。

| 能力 ID / 场景 | 可观察目标与反例 | 已读证据与缺口 |
| --- | --- | --- |
| DATE-CALENDAR | 2024-02-29、2000-02-29 成功；2023-02-29、1900-02-29、2024-04-31、月 00/13、日 00/32 拒绝 22008。构造内部 2023-02-29，text/binary 均拒绝，不能格式选择改变合法性。 | 当前只有一对闰日参数 is_ok/is_err，结果粗校验有差距，精确错误未验证。 |
| DATE-EPOCH | 1999-12-31、2000-01-01、2000-01-02 对应 ff ff ff ff、00 00 00 00、00 00 00 01；1970-01-01 为 -10957 日。四组合值和字节一致，改变 TimeZone 不移动已存日期。 | 仅 -1 日结果局部断言，参数 binary 未开放，完整往返未验证。 |
| DATE-RANGE | 日序 -2451545/2145031948 及相应文本成功，外侧各一天拒绝 22008；1 BC 与 1 AD 跨界无外部零年，10000-01-01 合法并保留。表示不足明确 0A000 且记缺口，不能把合法 PG 值裁到 9999。 | 共享 u64 字段和 chrono 路径不证明全域；边界和 BC 未验证。 |
| DATE-ZERO | 输入 0000-00-00、0000-01-01、2024-00-01、2024-01-00 拒绝 22008；桥接 MySQL 零/非法日期两种结果格式均拒绝，不映射 epoch、infinity 或 NULL。MySQL 单独在固定 sql_mode 下保持原错误/告警行为。 | 当前结果仅零/粗范围检查，双方言隔离和 SQLSTATE 未验证。 |
| DATE-SPECIAL-STYLE | -infinity/infinity 两格式分别往返 80 00 00 00/7f ff ff ff，非 NULL；epoch→1970-01-01。固定事务与时区核对 now/today/昨日/明日及 Prepare 后读取时机；allballs、空串、garbage 拒绝 22007。ISO 基线及 MDY/DMY 歧义输入按官方值，输出回读同日。 | 现有日期入口未覆盖非有限/相对词/DateStyle，均未验证。 |
| DATE-NULL-METADATA | typed NULL 参数、NULL::date、首行/全 NULL、零行及重复 Execute；参数描述 1082，statement/portal Describe 与 Execute 1082/4/-1，结果格式切换不改身份；NULL 长度 -1，不能混同 binary 0 日或空串。 | 有局部 NULL 输出意图，参数 tp=6 连续性及准备全链未验证。 |
| DATE-INVALID-BINARY | 非 NULL 0/3 字节不足拒绝 08P01，5 字节额外拒绝 22P03；四字节 -2451546/2145031949 拒绝 22008，两个哨兵接受；帧声明与实长不符拒绝 08P01。text 非法 UTF-8/NUL 拒绝 22021。 | date binary 参数当前 0A000，长度/域/恢复及精确 SQLSTATE 均未验证。 |

[日内时间类型契约](#日内时间类型契约)以下 TIME 入口均为**待新增**，运行行为全部**未验证**；成功项 SQLSTATE 不适用，精度收敛告警单独记录。固定具体 PG 构建、编码、TimeZone 与 typmod，合法值覆盖两格式参数→两格式结果四种组合，采集原字节、值、NULL、元数据和错误；MySQL 独立记录 sql_mode、Fsp、类型及自身错误/告警。

| 能力 ID / 场景 | 可观察目标与反例 | 静态证据和未验证项 |
| --- | --- | --- |
| TIME-DAY | 00:00:00、23:59:59.999999、24:00:00 分别为 0、86399999999、86400000000 微秒；24:00 不等于零点，binary 为各整数八字节网络序。24:00:00.000001、25:00:00、12:60:00、12:00:61 拒绝 22008。 | 已有 1 微秒结果局部断言；24:00 参数既有拒绝与目标相反，text 完整校验及四组合未验证。 |
| TIME-PRECISION | time(0) 的 12:34:56.5→12:34:57；time(3) 的 12:34:56.1234→.123、.1235→.124；23:59:59.999999 到 time(0)/(3) 均进位 24:00:00。p=0/6 保留声明，负 p 拒绝 22023，p=7 告警 22023 并归 6。超过六位输入如 12:34:56.1234567 按参考微秒舍入为 .123457，不能直接拒绝。 | 共享 MySQL 舍入断言不证明 PG 输入/typmod；当前非整微秒拒绝，全部 PG 精度边界未验证。 |
| TIME-TEXT-ZONE | 04:05→04:05:00、040506→04:05:06、04:05 PM→16:05:00、allballs→00:00:00；04:05:06+08:00 输入普通 time 后仍 04:05:06，切换 TimeZone 不移动它且 OID 始终 1083。 | 当前固定格式不证明官方输入集/时区处理，均未验证；timetz 请求明确拒绝 0A000。 |
| TIME-DURATION | MySQL -00:00:01、25:00:00、838:59:59 在自身规则下单独验收，桥接 PG time 两格式均 0A000；PG 24:00:00 不因旧 hour<24 规则被误拒绝。反向显式桥接保留 24 小时与微秒，禁止暗变 interval。 | 仅已有 25 小时结果 is_err 和共享负 duration 数值断言；双方言全链与精确错误未验证。 |
| TIME-NULL-METADATA | NULL::time(3)、typed NULL 参数、首行/全 NULL、零行、多行和重复 Execute 均保持 1083/8/3；无声明 typmod=-1，time(6) 为 6；参数描述 1083。两种结果格式与 statement/portal Describe 一致，NULL 长度 -1，零点长度 8 的 binary 全零。 | 有局部 binary NULL 输出；参数 tp=6 与固定 typmod=-1 不证明身份/修饰符连续，完整流程未验证。 |
| TIME-INVALID | 空串/garbage→22007，非法 UTF-8/NUL→22021；binary 0/7 字节→08P01、9 字节→22P03，八字节 -1/86400000001/i64 极值→22008；帧实长不符→08P01。构造内部 12:60:00，text/binary 均拒绝，格式不改变合法性。 | 当前 binary 参数未开放、text 只查小时；所有精确错误和协议恢复未验证。 |

[无时区时间戳契约](#无时区时间戳契约)以下 TS 用例均为**待新增**，运行行为全部**未验证**；成功 SQLSTATE 不适用，精度声明告警另记。按第 1 章固定参考构建、UTF-8、DateStyle、TimeZone、事务时间、p；合法值覆盖 text/binary 参数→text/binary 结果四组合，记录原字节、规范值、NULL、元数据及错误。MySQL 独立记录 code=7/12、Fsp、sql_mode、写入/读取时区和自身错误/告警。

| 能力 ID / 场景 | 可观察目标及反例 | 静态证据及缺口 |
| --- | --- | --- |
| TS-EPOCH-FRACTION | 1999-12-31 23:59:59.999999、2000-01-01 00:00:00、同日 .000001 分别为 i64 -1/0/1 网络序；六位微秒逐位保留。输入 .1234567 归 .123457；timestamp(3) 的 .1234→.123、.1235→.124。epoch 两侧 ±500000 微秒收敛 p=0 为 ±1000000，不能按日历秒对负值舍错方向。 | 只有 -1 微秒结果及六位参数局部断言；现有七位拒绝与目标有差距，全部往返未验证。 |
| TS-CALENDAR-PRECISION | 2024-02-29 合法；2023-02-29、1900-02-29、2024-04-31、月13、分钟60 拒绝 22008；2024-12-31 23:59:59.999999 到 p=0/3 进位 2025-01-01。p=0/6 接受，负 p 拒绝22023，p=7 告警22023归6。构造非法内部日期/时分秒，两格式同拒绝。 | text 粗查与 binary chrono 校验不等域；日期边界、精度及精确错误未验证。 |
| TS-RANGE-ZERO | 精确有限上下端微秒成功，-211813488000000001、9223371331200000000 拒绝22008；BC/AD无年份0，10000年不裁剪。0000-00-00、0000-01-01、2024-00-01、2024-01-00 均22008；MySQL零/非法日期桥接两格式同拒绝。合法PG值承载不足须0A000并记缺口。 | 年1–9999限制与目标有差距；原date零值测试不证明timestamp零值，全域和桥接未验证。 |
| TS-ZONE-IDENTITY | TIMESTAMP '2004-10-19 10:23:54+02' 仍为10:23:54/OID1114；已存2024-03-10 02:30:00在UTC、Asia/Shanghai、America/New_York切换后日历字段与binary不变，不能因DST缺口当瞬间纠正。MySQL分别用DATETIME/TIMESTAMP写后切换时区，记录自身结果与桥接前提，缺前提拒绝0A000。 | code7/12映射不足以证明会话隔离；列名含timezone或相同文本不构成1184证据，未验证。 |
| TS-SPECIAL | -infinity/infinity 两格式分别往返i64最小/最大且非NULL；epoch→1970-01-01 00:00:00。固定事务核对now/today/昨日/明日，在Prepare常量与Bind参数各自读入时转换，不随后续Execute重新解释已存值。 | 当前无特殊值/相对词断言，未验证。 |
| TS-NULL-METADATA | NULL::timestamp(3)、typed NULL参数、首行/全NULL、零行、多行及重复Execute，ParameterDescription=1114；statement/portal Describe与Execute均1114/8/3，无声明typmod=-1。两格式NULL长度-1，binary epoch长度8全零，空串拒绝；不由首行或格式改型。 | 有局部NULL结果意图；tp6与固定typmod=-1不足以证明身份连续，完整客户端流程未验证。 |
| TS-INVALID-LENGTH | 空串/garbage→22007，非法UTF-8/NUL→22021；binary非NULL 0/7字节→08P01、9字节→22P03，八字节域外非哨兵→22008，帧实长不符→08P01；错误后恢复由工作流7验收。 | binary参数当前0A000，所有精确错误及恢复未验证。 |

[带时区时间戳契约](#带时区时间戳契约)下列 TZ 用例全部**待新增、未验证**。固定参考构建、DateStyle、TimeZone、tzdata、p、编码与读入阶段；每个合法值覆盖 text/binary 参数到 text/binary 结果四组合，记录规范瞬时、原字节、元数据、NULL 和错误。成功 SQLSTATE 不适用，告警单列；已有 1114 测试只作静态对照。

| 用例 | 可观察验收目标 | 证据状态 |
| --- | --- | --- |
| TZ-SAME-INSTANT | 上述 +00/+08 两输入相等，binary 都是八字节零；同墙钟 +00/+08 则相差八小时，不比较原输入拼写。 | 无 1184 独立断言，未验证。 |
| TZ-SESSION | 同一已存/已绑定瞬时 UTC 显示午夜+00，Shanghai 显示08:00+08；二进制不变。无偏移文本绑定时记录当时区域，改区后不重新解释存量值；解析时特殊词另记录求值时机。 | PgSession 无 TimeZone 字段；状态交接及真实客户端未验证。 |
| TZ-DST | New_York 春季02:30→07:30Z、秋季01:30→06:30Z；显式-04→05:30Z，显式-05→06:30Z；text 重显示与 binary 瞬时一致。 | 以官方 B.2 为预期，未运行参考端或本地。 |
| TZ-PRECISION-RANGE | epoch±1微秒保真，p=0 的 ±500000→±1000000；七位输入舍入及跨日进位。有限两端成功，端外一微秒22008，偏移归一跨边界亦拒绝；BC无年份0、扩展年份、infinity/-infinity 两格式一致，零日期22008。 | 当前 CoreTime/chrono 与全域有差距；全部未验证。 |
| TZ-CAST-BRIDGE | timestamp 午夜在UTC/Shanghai解释得到不同瞬时；重复小时两个瞬时转无时区可同值，不能声称互逆。NULL/特殊值按目标契约；缺MySQL来源时区0A000，不改Go原有行为。 | 只有Go显式字段转换意图，无PG转换/桥接实测。 |
| TZ-NULL-METADATA | NULL::timestamptz(3)、typed NULL参数、首行/全NULL/零行、多行及重复Execute，ParameterDescription=1184；statement/portal Describe与Execute保持1184/8/3，未声明-1；切换格式或时区不改身份。NULL长度-1区别于epoch八字节零。 | 当前tp6及1114映射不能证明，未验证。 |
| TZ-INVALID | garbage/空串22007，2月30日及归一溢出22008，非法编码/NUL22021；binary 7/9字节分别08P01/22P03，帧错误08P01；未知区名按PG输入错误分类对照，不能回退本地默认时区。合法能力缺口0A000单独登记，非成功替代。 | 当前统一不支持不是完整错误分类；恢复由工作流7验收，未验证。 |

[时间间隔契约](#时间间隔契约)以下 IV 场景均为**待新增、未验证**。固定 PG 构建、UTF-8、IntervalStyle、fields/p；合法输入覆盖 text/binary 参数到 text/binary 结果四组合，采集原字节、三分量、NULL、元数据和 SQLSTATE。成功 SQLSTATE 不适用；告警单记。text 回读按分量验收，不只按显示或比较结果判等。

| 场景 | 可观察目标及反例 | 当前证据/责任 |
| --- | --- | --- |
| IV-COMPONENTS | `1 month`→(1,0,0)，`30 days`→(0,30,0)，`1 day`→(0,1,0)，`24 hours`→(0,0,86400000000)；四组合保持原维度。`1.5 months`→(1,15,0)，明确区别输入小数单位归整与存量整数月折算。 | 月→720小时的既有断言证明不能直接复用；PG 往返未验证。日期算术另交工作流4。 |
| IV-SIGNS-STYLE | postgres 下 `-1 month +2 days -03:04:05.123456`→(-1,2,-11045123456)；纯负、零与四种输出样式回读保真。`-1 2:03:04` 在 postgres/sql_standard 分别按独立符号/统一负号核对。 | 仅 MySQL 单一总量负号测试，PG 混合符号与会话分流未验证。 |
| IV-PRECISION-FIELDS | interval(3) 的 ±1.2345秒→±1.235秒；interval(0) 的 ±0.5秒→±1秒；p=0/6、七位小数微秒收敛与舍入进位按参考端。`1 day 2:03:04` HOUR TO MINUTE→(0,1,7380000000)，YEAR 对 `1 year 2 months`→12个月。负p拒绝22023、p=7告警后归6。 | 不以 MySQL Fsp 断言替代；PG 修饰符与精确元数据未验证。 |
| IV-RANGE-SPECIAL | binary 独立月/日 i32 极值、时间 i64 极值与相邻值保真；三分量全最小/全最大专门往返 -infinity/infinity，非 NULL；其他混合极值保持有限。文本 `2147483648 months` 拒绝22015；单位累加/收敛溢出按阶段核对22015/22008，禁止绕回或饱和。 | 现有 MySQL TIME 小域不能证明 PG 全域；边界与错误阶段未验证。 |
| IV-NULL-METADATA | NULL::interval(3)、typed NULL参数、首行/全NULL/零行、多行和重复Execute保持 ParameterDescription=1186；statement/portal Describe与Execute均1186/16/编码后的typmod，未声明-1。两格式NULL=-1；binary零间隔长度16全零，不能猜为text。 | 当前tp6提前返回不证明身份连续；准备全链未验证。 |
| IV-INVALID | 空串/garbage→22007、非法UTF-8/NUL→22021；binary非NULL 0/15字节→08P01、17字节→22P03，帧声明/实长不符→08P01。错误后恢复由工作流7核验；不能用当前0A000冒充目标输入校验。 | 所有精确错误及真实客户端未验证。 |
| IV-BRIDGE | MySQL -51小时仅在显式约定下桥接(0,0,-183600000000)；含月/日的反向桥接或超MySQL域均0A000，不能折日或截断；typed NULL保留目标身份。MySQL自身值/错误按原sql_mode另验收。 | 当前通用映射仍1083；桥接资格及双方言隔离未验证。 |

[通用唯一标识契约](#通用唯一标识契约)以下 UUID 场景均为**待新增、未验证**；固定PG精确构建及UTF-8，合法值覆盖text/binary参数到text/binary结果四组合，比较规范16字节、输出、NULL、元数据和错误。成功SQLSTATE不适用；不以字符串相等替代类型断言。

| 场景 | 可观察验收目标 | 当前证据状态 |
| --- | --- | --- |
| UUID-TEXT | 标准、大写、无连字符、花括号及每四位连字符变体均得到同一16字节；输出统一小写36字节格式；显式text↔uuid往返保持值及目标类型。 | 官方uuid.c静态依据；独立测试未见上述断言，未运行。 |
| UUID-BITS | 全零、全ff、首/尾字节01/80及00112233样本两格式往返；任意version/variant位均接受；无字节交换。 | 固定字节布局有源码依据，四组合全链未验证。 |
| UUID-INVALID | 空串、31/33位、g字符、奇数位、错位/尾连字符、单侧花括号、URN、前后空白拒绝22P02；非法UTF-8走22021；binary长度0/15走08P01、17走22P03；不截断。 | 现有2950缺分支，目标诊断及协议恢复均未验证。 |
| UUID-NULL-METADATA | typed NULL参数、NULL::uuid、首行/全NULL、零行、多行、重复Execute仍保持参数2950及结果2950/16/-1；两格式NULL长度-1，全零binary长度16。 | 现有tp6提前返回不能证明身份保真；Describe/Execute全链未验证。 |
| UUID-INFERENCE-BRIDGE | 声明列/显式cast/2950定型结果一致；OID0从明确上下文定型，歧义拒绝沿推导契约；普通UUID外观text及16字节bytea保持自身身份，无依据桥接0A000。 | 现有MySQL推导与PG出口分离证据；目标推导、桥接及双方言回归未验证。 |

[JSON文本契约](#JSON文本契约)下列 JSON-TEXT 场景全部**待新增、未验证**；固定PG精确构建及UTF-8配置，每个合法值覆盖text/binary参数到text/binary结果四组合，分别记录输入原字节、保存文本及结果载荷，不能只比较解析后值树。成功SQLSTATE不适用，错误同时记录阶段与SQLSTATE。

| 场景 | 可观察目标及反例 | 当前证据状态 |
| --- | --- | --- |
| JSON-TEXT-PRESERVE | 输入 ` { "b":1, "a":2,"b":3 } `及制表/换行变体，输出原样，两个b成员、顺序和外侧空白都保留；`1.230e+02`不改写123。 | BTreeMap与重序列化暴露差距；既有规范文本往返不能证明保留，未验证。 |
| JSON-TEXT-NUMBER | 9007199254740993、18446744073709551616、1e1000000以及长小数两格式逐字节保留，不能舍入或以numeric域拒绝；01/+1/NaN/Infinity拒绝22P02。 | appendBinaryNumber的f64回退不能证明；大数PG往返未验证。 |
| JSON-TEXT-SCALAR | `42`、`true`、`false`、`"x"`、`[]`、`{}`均为非NULL json；根字符串结果保留引号，不能调用json_unquote。 | 现有MySQL容器/标量断言不证明OID114，未验证。 |
| JSON-TEXT-UNICODE | `"中"`、`"\u4e2d"`分别原样保存，`"\u0000"`及`"\uD800"`不在纯存取时被解码/拒绝；`"\uZZZZ"`及原始未转义控制字符22P02，非法UTF-8/原始NUL22021。 | serde值树不是PG词法保留证据；处理函数或jsonb阶段另验收，未验证。 |
| JSON-TEXT-INVALID | 空串、纯空白、多根值`{} []`、`[1,]`、未闭合字符串、注释均22P02；binary前缀字节01加合法文档不能当jsonb接受；错误后恢复交7。 | TestParseBinaryFromString仅有MySQL错误意图，PG两格式诊断未验证。 |
| JSON-TEXT-NULL-METADATA | typed NULL参数、NULL::json、首行/全NULL/零行/多行/重复Execute保持ParameterDescription114及结果114/-1/-1。SQL NULL两格式长度-1，JSON null长度4且字节6e756c6c，JSON字符串`"null"`带引号长度6；格式码切换不改身份。 | 已有NULL JSON拒绝断言不是支持证据，准备全链未验证。 |
| JSON-TEXT-CAST-BRIDGE | text显式转json校验并保留；反向转text得到原文；不自动把普通text、MySQL245或jsonb冒充原文json。缺有证据桥接时0A000；MySQL独立按原Go契约验证。 | 转换资格及双方言回归未验证，不用输出字符串相同证明类型相同。 |

[JSONB规范值契约](#JSONB规范值契约)以下JSONB场景全部**待新增、未验证**。合法值覆盖text/binary参数×text/binary结果四组合，记录精确PG构建、编码、参数OID/格式、结果OID/长度/typmod/格式、语义值及原始载荷；去掉binary的01前缀后应与同一值的text输出一致。错误记录阶段和SQLSTATE，成功SQLSTATE不适用。

| 场景 | 可观察目标 | 证据状态 |
| --- | --- | --- |
| JSONB-DUP-ORDER | `{"aa":1,"b":2,"aa":3}`保留aa=3，固定REL_18_0输出`{"b": 2, "aa": 3}`；嵌套重复键、`"a"`与`"\u0061"`同键最后值；输入换序不改变规范对象。 | 已读MySQL往返测试未断言此PG键序/解码重复键；未验证。 |
| JSONB-NUMBER | `18446744073709551617`及高精度小数无舍入；`1.230e-5`输出`0.00001230`，`1.00`保留小数尾零；超PG numeric域如`1e200000`拒绝22003，NaN/Infinity及非法数字22P02。PG域内不能承载时明确0A000。 | appendBinaryNumber走f64不足；未验证精度、scale及诊断。 |
| JSONB-SCALAR-NEST | `{}`、`[]`、嵌套`[null,{"x":[true,"中",1]}]`及独立`1`、`"x"`、false、null均保持种类；`[2,1,2]`保持顺序和重复元素。 | TestBinaryJSONMarshalUnmarshal仅MySQL意图；PG往返未验证，深度资源边界需按参考端测，不能照搬100。 |
| JSONB-UNICODE | `"中"`与`"\u4e2d"`输出同值，`"\uD83D\uDE00"`正确合并；`"\u0000"`及孤立代理对22P05；`"\uZZZZ"`22P02；非法UTF-8/原始NUL在编码阶段22021。 | PG类型输入比json文本严格；阶段/诊断均未实测。 |
| JSONB-NULL-METADATA | SQL NULL长度-1，JSON null的text为`null`、binary为`01 6e 75 6c 6c`；空集/首行NULL/全NULL、重绑定均保持参数3802与结果3802/-1/-1，Describe/Execute一致，不变text。 | unsigned_binary_and_unsupported_types仅证明当前JSON结果拒绝；typed NULL未验证。 |
| JSONB-WIRE-INVALID | binary长度0缺版本拒绝08P01；仅01无文档22P02；00/02/ff版本拒绝并采集参考SQLSTATE；01后非法/截断JSON、多根值、尾随垃圾22P02；声明长度超可用帧08P01。 | 当前无3802 codec；不能用0A000替代最终参考诊断，所有长度/版本场景未验证。 |
| JSONB-CAST-BRIDGE | text/json显式转jsonb规范化；含json可保存但jsonb不能接受的转义/超域数字在转换时拒绝；反向json/text只得规范文本；MySQL opaque/time扩展不自动映射。 | 方言cast资格、MySQL不变回归和PG错误阶段未验证。 |

[基础数组契约](#基础数组契约)以下ARRAY场景全部**待新增、未验证**；SQL构造须先满足第2章交接，codec可直接使用带数组OID的`SELECT $1 AS v`参数场景。每个合法值覆盖text/binary参数×text/binary结果四组合，记录精确参考构建、编码、array_nulls、原始载荷、参数及结果OID/typmod/格式、形状/下界/元素NULL。成功SQLSTATE为“不适用（成功）”；错误记录阶段、实际SQLSTATE和协议恢复，不能以目录测试替代。

| 场景ID | 可观察目标与边界 | 当前证据状态 |
| --- | --- | --- |
| ARRAY-SHAPE | int4[] `{1,2}`保存一维[2]/下界[1]；`{{1,2},{3,4}}`保存二维[2,2]/[1,1]；顺序1,2,3,4，二者结果OID1007，维数不改变类型。 | 目录编码仅一维；普通SQL/参数四组合未验证。 |
| ARRAY-LOWER | int4[] `[0:1][-2:-1]={{1,2},{3,4}}`四组合往返保留shape/lower；text仍带维度前缀，binary每维下界不改1。 | 现有binary_array固定下界不能作保真证据；未验证。 |
| ARRAY-EMPTY-NULL | `{}`为非NULL零维空int4数组、OID1007、binary为12字节头0/0/23；NULL::integer[]外层-1；`{NULL}`为一维[1]、一个元素-1；三者不同。空集/首行NULL/全NULL及重绑定Describe/Execute均保持1007/-1/-1。 | pg_binary_native_results只断言目录空int2数组及NULL外层；通用typed NULL和稳定元数据未验证。 |
| ARRAY-TEXT-ESCAPE | text[] `{"NULL",NULL,"","a,b","a\\b","a\"b","中"}`区分NULL字样、NULL元素、空串及转义；小写未引用null为NULL；引用/反斜线不丢失，不按逗号简单切割。 | 通用输入、输出引用及四组合未验证；固定array_nulls=on。 |
| ARRAY-TEXT-INVALID | int4[] `{{1,2},{3}}`、`[0:2]={1,2}`、`{1,}`、未闭合引号/转义均22P02；`{x}`22P02、`{2147483648}`22003；不补NULL、不截断。 | 输入codec缺失；文本路径诊断未验证，ARRAY构造路径另采阶段。 |
| ARRAY-WIRE-TYPE | 外层1007配头元素23成功；改头为内建25拒绝42804，即使空数组也校验元素身份；外层误报1009不得依值猜为int4[]。 | pg_oid_test只目录对象OID唯一性；数组OID配对及拒绝未验证。 |
| ARRAY-WIRE-LENGTH | ndim=-1、flags=2、元素length=-2/超剩余/未消费完整拒绝22P03；缺头/长度字段按08P01边界；维长负值、乘积/下界溢出、7维54000、元素数不符和尾随垃圾分别记录参考诊断。flags=0且NULL元素单列参考接受性，输出应规范反映NULL。 | 未验证；不得让长度错误越过载荷边界，未采集码的分支不冒充通过。 |
| ARRAY-ELEMENT-CAST | int4[]→int8[]在允许显式转换下保留shape/lower/NULL、结果1016且头20；超域/非法元素按元素契约失败，无部分结果；同值text[]不能保留1007冒充原身份。 | cast资格及元素精度/时区继承待转换块；PG往返与MySQL隔离回归未验证。 |

[显式隐式转换契约](#显式隐式转换契约)待新增CAST用例族如下，全部PG/双方言运行均未验证。每条成功项须比对值、目标OID/typmod及NULL标志，拒绝项须比对SQLSTATE、阶段、是否有P和原位置；simple Q与扩展路径分别采集，不用测试名称或编译替代行为。

| 用例 | 可观察目标与反例 | 证据状态 |
| --- | --- | --- |
| CAST-LEGAL-INVALID | `'12'::int4`→12/OID23；`'12x'::int4`拒绝22P02；`'2147483648'::int4`拒绝22003，无裁剪行。 | 现有test_str_to_num的前缀接受恰为非等价证据；PG未验证。 |
| CAST-CONTEXT | `1::boolean`→true/16；`SELECT 1 WHERE 1`拒绝42804；int4列赋值boolean拒绝，显式true::int4→1。另int4→int2赋值合法但超域拒绝22003。 | E/A/I分离目标；MySQL独立验证原真值/告警，均未运行。 |
| CAST-UNKNOWN-NULL | `SELECT NULL::int4`返回typed NULL/23；`SELECT '1' UNION SELECT 2`定为int4，而`SELECT '1'::text UNION SELECT 2`拒绝42804；单列全unknown NULL集合定为text/25。 | 源码NULL短路不能证明定型；未验证。 |
| CAST-COMMON | `SELECT 1.2 UNION SELECT 1`为numeric/1700；`CASE WHEN true THEN 1 ELSE 2.2 END`为numeric；`VALUES (1),(2.2)`同列numeric；`SELECT NULL UNION SELECT NULL UNION SELECT 1`因左侧先定text而拒绝42804。 | 共同算法、逐对顺序、CASE候选及空结果描述均未验证。 |
| CAST-PARAMETER | Parse省略OID的`SELECT $1::int4`推定23，Bind文本12/binary整数12/NULL均保持结果23；`SELECT $1 IS NULL`无上下文目标42P18。用INSERT到int4、text两列的`VALUES ($1,$1)`采集不一致推断42P08；显式OID25的`SELECT $1 + 1`拒绝重载42883，不能因绑定“1”改变资格。 | 当前普通Parse OID0为0A000；目标参数冲突优先级、位置及两格式均未验证。 |
| CAST-ROUND-OVERFLOW | numeric `1.235::numeric(4,2)`→1.24，`99.995::numeric(4,2)`舍入后越界22003；`2.5::numeric::int4`→3、`(-2.5)::numeric::int4`→-3；float半值按浮点族单独采集，不能混用numeric规则。 | 现有decimal钳制为MySQL证据；PG舍入与溢出未验证。 |
| CAST-FAMILY-METADATA | 固定UTC/Shanghai比较timestamp↔timestamptz；json→jsonb规范化再返json不得恢复原文；int4[]→int8[]保留非1下界、shape及元素NULL，结果1016/元素20；text→UUID合法与非法按UUID块；bytea→text按bytea输出。 | 前序族验收关联；元数据在空行/全NULL/重绑定及text/binary均一致，全部未验证。 |
| CAST-LENGTH-BRIDGE | `'abcd'::varchar(2)`→ab；向varchar(2)列直接赋值'abcd'拒绝22001。unsigned最大值提升numeric保真；MySQL零日期/负或超24小时duration不得冒充PG日期/time，无依据桥接0A000。 | 沿文本、整数与时间块；双方言隔离及无部分写入均未验证。 |

[准备描述执行元数据契约](#准备描述执行元数据契约)以下META入口全部为**待新增、未验证**。按第1章锁定PG精确构建、客户端、编码、会话/目录前提，采集Parse OID、两种Describe原包、Bind格式/值和Execute载荷及SQLSTATE；成功SQLSTATE不适用。MySQL按自身Prepare协议独立回归，不比较OID相等。

| 场景 | 可观察目标与负例 | 当前证据状态 |
| --- | --- | --- |
| META-EMPTY-NULL | `SELECT NULL::int4 AS v WHERE false`零行、`SELECT NULL::int4 AS v`全NULL，以及预建int4列首行NULL后接1；两Describe均23/4/-1/一列，Execute零行或NULL=-1，首行不定型。 | 目录零行有静态断言；通用三场景全链未验证。 |
| META-PARAM | `SELECT $1::int4 AS v`分别用无OID、[0]、[23] Parse，目标参数23、结果23/4/-1；`SELECT $1 IS NULL`无OID目标42P18。额外显式text参数经合法cast时参数25与结果23须分别记录，不能强制相等。 | 普通未指定OID当前0A000；显式23表谓词有静态意图，参数化投影未验证。 |
| META-REBIND | 同一int4语句依次新Bind 1、2、NULL及两种参数格式；描述不变、值分别对应；重复/乱序$n按编号绑定。域外int4拒绝22003，不能升级结果OID。 | 重绑定/乱序有局部静态断言，表达式/范围/typed NULL连续性未验证。 |
| META-FORMAT | 同一两列int4/text语句创建text、binary及混合portal；statement格式全0，各portal格式与Bind匹配，OID/typlen/typmod/列数相同。切换portal及重复Execute不串格式；错误格式数量/码拒绝08P01。 | binary局部载荷断言不覆盖完整portal隔离，未验证。 |
| META-TYPMOD | `SELECT NULL::numeric(8,2), NULL::varchar(3), NULL::timestamp(3)`及非NULL/零行变体；OID1700/1043/1114，typlen -1/-1/8，typmod分别为前序族规定的编码，statement/portal一致。 | encode_formats固定-1为已知差距，未验证。 |
| META-MISSING-CHANGE | 注入缺native_types、数量不符、执行行宽不符及准备/执行OID或typmod冲突，均应在DataRow前明确拒绝；预建单列int4表后Parse `SELECT *`，ALTER增加列再新Bind，固定结果变化目标0A000；另独立夹具改int4为int8同样拒绝。重新Parse可建立新描述。 | 缺元数据/数量静态断言及描述比对代码已读；变化与DDL全链未验证，不保证DDL自身已支持。 |
| META-NO-PROBE | 用执行调用计数或受控有副作用函数夹具观测Parse及两种Describe：执行原查询次数0，无数据副作用；只有Execute触发一次，分批重复Execute不重新求值。分析/规划合法的常量折叠不等于执行查询探测。 | Describe不调用执行闭包为静态证据，计数/副作用及准备内部链未验证；夹具为待新增，不引用现有函数名。 |

### 后续细分

章节验收与后续交接仅保留候选边界：内部表示先确认各族身份、值域、typmod及typed NULL；cast依赖表示与E/A/I上下文；参数codec依赖稳定参数OID、元素codec和会话前提；结果codec依赖相同规范值、固定结果描述及portal格式。按族重新核实依赖与占用后由用户决定实施划分，本轮不生成执行任务。后续推荐工作流4承接类型/转换契约，工作流8补真实运行证据；15族、转换和元数据的未验证状态不得随文档完成升级。未发现需修改其他章节的契约冲突。

[规范类型与协议映射](#规范类型与协议映射)仅给候选边界：先取证 FieldType/NativeType 能保留的身份、修饰符与参数生命周期，再决定最小内部表示补充；cast 候选依赖该身份并先分方言确定转换规则；参数 codec 候选依赖确定的参数类型及值域；结果 codec 候选依赖相同的准备/执行列类型契约。各族顺序服从本章后续文档，不生成可执行代码任务、不递归扩建共享内核。协议消息/恢复的实现由工作流 7 独立承接。

来源步骤覆盖：来源任务步骤 1 的“规范类型↔MySQL/PG 映射、推导、参数/结果、NULL”由两个方向表与共同约束覆盖；本编号任务要求的方案比较由保真缺口决策覆盖；验收五边界由 TYPE-MAP-IDENTITY/EMPTY/NULL/UNSIGNED/UNKNOWN 覆盖。来源步骤 2–4 的逐族完整语义、cast 与精确 codec 留在本章其余编号文档块，本块映射索引不替代那些交付。第 1 章证据规则与第 2 章类型/数组语法交接保持只读，未发现需要改写前序契约的冲突。

[布尔类型契约](#布尔类型契约)候选边界为：内部表示先验证 bool 身份能随字面量、声明列、有类型 NULL 和参数跨 Prepare/Execute 保留；cast 依赖该身份并区分显式、赋值与隐式上下文；参数 codec 依赖已定型 OID，补齐文本词集与二进制接收域；结果 codec 依赖同一列身份并保持 Describe/Execute 一致。这里只描述候选边界与依赖，不生成可执行实现任务。

布尔块来源覆盖：来源步骤 2 的 bool 族由方向表覆盖；步骤 3 的 bool 显式/隐式边界由转换段及 BOOL-CAST 覆盖；步骤 4 的 text/binary、NULL、非法输入、元数据一致性由 BOOL-TEXT/BINARY/NULL-DESCRIBE 覆盖。与前块的“不能自动将非零整数视为 bool”一致，显式 int4 cast 和 OID 16 单字节接收是有来源的限定场景；未发现需改动第 1/2 章或来源任务的冲突。

[整数与无符号边界](#整数与无符号边界)候选边界：内部表示先验证声明宽度、signed/unsigned 与 typed NULL 跨 Prepare/Execute 保留；cast 依赖身份及方言上下文，独立核对窄化与负值；参数 codec 依赖已定型 OID，补输入语法及错误分类；结果 codec 依赖稳定结果元数据，U64 binary 另依赖 numeric 契约。本块不生成可执行实现任务。来源步骤 2 的整数族由值域表及方向表覆盖，步骤 3 的 unsigned/溢出由桥接及算术交接覆盖，步骤 4 的格式/NULL/非法输入/元数据由 INT-RANGE/UNSIGNED/CODEC/NULL-METADATA/CAST-ARITH 覆盖。未发现与前序布尔限定场景或第 1/2 章冲突；功能、参考端与性能均未实测。

[精确数值契约](#精确数值契约)候选边界：先核定 MyDecimal 与 cast Decimal 可保真域及规范身份的 precision/scale/特殊值缺口，再决定内部表示；cast 依赖该表示和方言资格，保留 PG 舍入/错误及 MySQL 原行为分流；参数 codec 依赖已定型 OID 和上下文 typmod，结果 codec 依赖同一身份及稳定 Describe 元数据。这里只给边界与依赖，不生成可执行任务。来源步骤 2 的 numeric 族由身份与方向表覆盖，步骤 3 的精度溢出/cast 由 NUM-PRECISION/ROUND 覆盖，步骤 4 的格式、非法输入、NULL 与元数据由 NUM-TEXT-SPECIAL/BINARY/NULL-METADATA 覆盖；NUM-LARGE 承接前块 U64 与更大精度差距。未发现前块或第 1/2 章冲突；本轮无功能、性能及参考端实测。

[浮点数值契约](#浮点数值契约)候选边界：内部表示先核对 KindFloat32、共享 f64 值盒与 NativeType 的定宽不变量及 typed NULL 保真；cast 依赖该不变量和方言上下文，分别核对舍入/范围与特殊值；参数 codec 依赖确定 OID，直接按目标宽度解析并分类错误；结果 codec 依赖同一列身份和输出配置，分别输出目标宽度文本及网络位型。这里只给边界和依赖，不生成实现任务。来源步骤 2 的浮点族由身份/方向表覆盖，步骤 3 的舍入和溢出由 FLOAT-RANGE/CAST 覆盖，步骤 4 的格式、NULL、非法输入和元数据由其余验收入口覆盖。与 numeric 的精确性和半值规则分开，与第 1/2 章术语及前序块无冲突；没有功能或性能实测。

[文本类型契约](#文本类型契约)候选边界与依赖为：内部表示先核对身份、声明字符长度、charset/collation 与 typed NULL 的连续性；cast 在该身份基础上区分显式截断、赋值拒绝和 char 去尾空格；参数 codec 依赖 OID 定型与 SQL 目标约束，分别校验编码/NULL；结果 codec 依赖同一列身份和编码状态，保持 OID/typmod 与补空格字节一致。这里只描述边界，不生成实现任务。来源步骤 2 由类型/方向表覆盖，步骤 3 由长度 coercion/cast 规则覆盖，步骤 4 由 TEXT 用例族覆盖；与第 1/2 章及前五块无契约冲突。无功能、兼容性或性能实测。

[字节类型契约](#字节类型契约)候选边界：内部表示先核对 bytes、binary charset、OID 17 及 typed NULL 跨桥接保真；cast 依赖该身份与方言资格；参数 codec 依赖已定型 OID，补 hex 空白/escape 并隔离 raw binary；结果 codec 依赖同一列身份和 bytea_output 配置，保持可逆及 Describe/Execute 一致。这里只给边界与依赖，不生成实现任务。

字节块来源覆盖：来源步骤 2 由值域/映射及四方向表覆盖；步骤 3 由转换边界段覆盖，完整资格交转换块；步骤 4 由 BYTEA 六项验收覆盖。与前块 text 禁止 NUL、bytea 允许 NUL 及第 1/2 章语法/证据分层一致，无需改其他章节；无功能、兼容性或性能实测。

[日期类型契约](#日期类型契约)候选边界：内部表示先核对 date 身份、有限日序、BC/扩展年份/非有限值及 typed NULL 的保真缺口；cast 依赖该身份和方言上下文，不复用 MySQL 宽松解析作为 PG 规则；参数 codec 依赖确定 OID、DateStyle 与特殊词解析时机；结果 codec 依赖相同规范值与稳定列元数据，两格式共享完整日历校验。这里只描述边界和依赖，不生成可执行实现任务。

日期块来源覆盖：来源步骤 2 的 date 族由日历/值域及四方向表覆盖；步骤 3 的零日期、不等价边界和转换交接由拒绝路径与 DATE-ZERO/RANGE 覆盖；步骤 4 的格式、NULL、非法输入及元数据由 DATE 七项验收覆盖。与第 1/2 章及前七块无须改写的冲突；精确下端采用官方源码日序而非概略年份表，后续实施须锁定参考版本复核。无功能、性能或参考端实测。

[日内时间类型契约](#日内时间类型契约)候选边界：内部表示先核对独立 time 身份、含 24:00 的微秒日内域、p 与 typed NULL，评估复用 Duration 是否能保真；cast 依赖方言身份并先拒绝负/长 duration，不自动提升；参数 codec 依赖定型 OID/SQL 约束，补 PG 文本与 binary 校验；结果 codec 依赖同一规范值和稳定元数据，共用完整值域检查。这里只描述候选边界与依赖，不生成可执行任务。

日内时间块来源覆盖：步骤 2 的 time 族由四方向表覆盖；步骤 3 的 duration 不等价、精度及转换由 TIME-DURATION/PRECISION 覆盖；步骤 4 的格式、NULL、非法输入和 Describe/Execute 一致性由 TIME-DAY/TEXT-ZONE/NULL-METADATA/INVALID 覆盖。与第 1/2 章及日期块无须改写的冲突；timetz 保持来源范围外。无功能、性能或真实参考端实测。

[无时区时间戳契约](#无时区时间戳契约)候选边界：内部表示先核对无时区身份、微秒有限域/特殊值、p 与 typed NULL 的保真，避免借 MySQL TIMESTAMP 标签发明瞬间；cast 依赖该身份并明确 MySQL 会话前提及 PG 转换资格；参数 codec 依赖已定型 OID、SQL p、DateStyle 和读入时机；结果 codec 依赖同一规范值及稳定 Describe 元数据，两格式共享完整值域校验。这里只给候选边界与依赖，不生成可执行实现任务。

无时区时间戳块来源覆盖：步骤 2 的 timestamp 族由四方向表与有限域覆盖；步骤 3 的精度、零日期及不引入时区由 TS-EPOCH-FRACTION/CALENDAR-PRECISION/RANGE-ZERO/ZONE-IDENTITY 覆盖；步骤 4 的格式、NULL、非法输入、长度与 Describe/Execute 由 TS-SPECIAL/NULL-METADATA/INVALID-LENGTH 覆盖。与第 1/2 章及前序块无须改写的冲突，timestamptz 留下一块承接；无功能、性能或参考端实测。

[带时区时间戳契约](#带时区时间戳契约)候选边界：内部表示先核定独立瞬时身份、域/特殊值、p 与 typed NULL；cast 依赖规范身份和工作流 5 提供的区域上下文，区分墙钟解释与瞬时显示；参数 codec 依赖 OID 定型、SQL精度及解析时会话配置；结果 codec 依赖同一规范值、稳定 Describe 和显示区域，binary 不能经有损墙钟中转。这里只描述候选边界和依赖，不生成可执行实现任务。

带时区时间戳块来源覆盖：步骤 2 由瞬时定义、元数据及四方向表覆盖；步骤 3 的精度/时区和MySQL不等价由 TZ-SESSION/DST/PRECISION-RANGE/CAST-BRIDGE 覆盖；步骤 4 由四格式组合、TZ-SAME-INSTANT/NULL-METADATA/INVALID 覆盖。与第1/2章及无时区块的身份、时区前提一致，无需改写来源或其他章节；没有功能、性能或18.6参考端实测。

[时间间隔契约](#时间间隔契约)候选边界：内部表示先核定独立 interval 身份、三分量、非有限值、fields/p 与 typed NULL 保真；cast 依赖身份和方言资格，禁止用总秒数替代日历分量；参数 codec 依赖已定型 OID、SQL 修饰符和 IntervalStyle；结果 codec 依赖相同规范值与稳定 Describe 元数据，直接输出三分量。这里只描述边界和依赖，不生成可执行任务。

时间间隔块来源覆盖：步骤2的interval族由身份、范围与四方向表覆盖；步骤3的精度/不等价转换由IV-PRECISION-FIELDS/RANGE-SPECIAL/BRIDGE覆盖；步骤4的格式、NULL、非法输入及元数据由IV-COMPONENTS/SIGNS-STYLE/NULL-METADATA/INVALID覆盖。与第1/2章及前序时间块无须改写的冲突；没有功能、性能或参考端实测。

[通用唯一标识契约](#通用唯一标识契约)候选边界：内部表示先确定独立UUID身份、固定16字节和typed NULL；cast依赖此身份及统一方言资格；参数codec依赖已定型2950，分别实施词法解析与固定字节解码；结果codec依赖规范值和稳定Describe元数据，输出规范文本或原字节。这里只描述边界与依赖，不生成可执行任务。

UUID块来源覆盖：步骤2由规范身份及四方向表覆盖；步骤3的显式转换与不等价桥接由UUID-TEXT/INFERENCE-BRIDGE覆盖（精度、溢出、时区不适用于固定128位类型）；步骤4由UUID-BITS/INVALID/NULL-METADATA和四组合往返覆盖。第1章证据等级、第2章语法交接及前序text/bytea身份规则保持一致，无需改写其他块。文档完成不代表UUID功能、性能或18.6差分通过。

[JSON文本契约](#JSON文本契约)候选边界：内部表示先验证独立json身份、原文载体与typed NULL连续性，保留现有MySQL BinaryJSON；cast依赖该身份及方言资格；参数codec依赖已定型114与编码上下文，校验但不重建；结果codec依赖同一原文和稳定Describe元数据，两格式直接输出文本。这里只描述边界和依赖，不生成实现任务。

JSON文本块来源覆盖：步骤2的JSON族由原文身份与五方向表覆盖；步骤3的精度/不等价转换由JSON-TEXT-NUMBER/CAST-BRIDGE覆盖，时区与数组维度不适用于本标量族；步骤4由四格式组合及PRESERVE/SCALAR/UNICODE/INVALID/NULL-METADATA覆盖。与第1章证据规则、第2章语法交接和前序类型身份契约一致，无需改写其他章节；jsonb独立承接，不宣称功能、性能或18.6差分通过。

[JSONB规范值契约](#JSONB规范值契约)候选边界：内部表示先确立独立jsonb身份、精确numeric及scale、解码字符串、对象去重和typed NULL；cast依赖该表示和方言转换资格，复用json原文校验入口时仍执行jsonb严格校验。参数codec依赖定型3802与编码上下文，binary先验版本再解析；结果codec依赖同一规范值、参考输出与稳定元数据，两格式共享输出语义。这里只描述边界及依赖，不创建代码任务。

JSONB块来源覆盖：步骤2由独立身份、值域及五方向表覆盖；步骤3由NUMBER/UNICODE/CAST-BRIDGE覆盖精度与非等价转换，时区及SQL数组维度不适用；步骤4由四格式组合及DUP-ORDER/SCALAR-NEST/NULL-METADATA/WIRE-INVALID覆盖。沿第1章证据规则、第2章语法交接及批次14原文责任，无跨块冲突；全部PG能力仍待实现取证，不宣称功能或性能通过。

[基础数组契约](#基础数组契约)候选边界：内部表示先保留元素身份/typmod、矩形shape、下界及两层NULL；cast依赖元素族与方言资格，逐元素转换不能丢形状。参数codec依赖定型数组OID、元素codec和会话编码，先校验头/边界再解码；结果codec依赖同一规范值、稳定元数据与元素输出，两格式共享shape/lower/NULL语义。目录binary_array仅作局部取证入口，不预设直接复用。这里只给候选边界与依赖，不生成可执行任务。

基础数组块来源覆盖：步骤2由元素/数组身份、值域和五方向表覆盖；步骤3由ARRAY-SHAPE/LOWER/ELEMENT-CAST覆盖维度、下界及转换，精度/时区继承元素族；步骤4由四格式组合及EMPTY-NULL/TEXT-ESCAPE/TEXT-INVALID/WIRE-TYPE/WIRE-LENGTH覆盖元数据、NULL与非法输入。与第1章证据规则、第2章数组树形/定型交接和前序各标量族一致，无需改其他章节；数组开放逐族取证，不以本块文档完成声称功能可用。

[显式隐式转换契约](#显式隐式转换契约)候选边界：内部表示先承接各族身份、typmod、unknown来源及typed NULL；cast依赖该身份和连接方言，先决策E/A/I、共同类型与参数约束，再复用等价值转换；参数codec依赖分析得到的稳定OID，结果codec依赖目标表达式身份及固定描述。这里只交接候选边界与依赖，不生成实现任务。来源步骤3的显式/隐式、精度溢出、时区、数组维度/下界及MySQL不等价边界由方向矩阵与CAST八组覆盖；步骤4相关NULL、格式和准备/执行描述交批次18承接生命周期。与第1章证据规则、第2章语法及前16块一致，无需改写其他章节；功能、性能、18.6参考端与MySQL回归未实测。

[准备描述执行元数据契约](#准备描述执行元数据契约)仅给候选边界：内部表示承接前序各族身份/typmod与typed NULL，并核定准备描述的可信来源；cast依赖定型信息，负责参数约束和结果身份；参数codec依赖稳定参数OID与逐参数格式；结果codec依赖固定结果描述及portal格式。生命周期/失效协作工作流5–7，不生成可执行任务或重建共享子系统。

准备描述执行块来源覆盖：步骤1由五阶段方向表覆盖规范类型、参数/结果及NULL分离；步骤4由META七组覆盖格式、NULL、值域/非法输入与Prepare/Describe/Execute一致性。与第1章证据规则、第2章参数语法及前17块一致，未发现需修改其他章节或来源的冲突；文档完成不表示功能实现，所有运行行为、性能及18.6/MySQL参考端均未实测。

## 4. 方言函数与运算符契约

### 模块

#### 三层身份与契约模板

本块只建立本章公共记录规则及一个标量候选，承接第 1 章的能力 ID/证据分级、第 2 章的连接方言和名称环境、第 3 章的规范类型、E/A/I 转换资格与准备/执行描述。后续契约实例统一引用下方模板，再填写自身差异；不复制一份可能漂移的公共规则。本轮文档完成不表示函数已支持。

静态取证基线为 `d98e9f395b83d26fc2a5bef0aecc232d3bc376c9`（2026-10-23），以下源码和独立测试均只阅读、未执行。两个目标包均无 `doc.go`；通过仓库 RustCodeGraph 索引定位真实源码，没有以 `.rs.md` 说明代替接线证据。

| 入口与实际符号 | 已核实的责任与证据限制 |
| --- | --- |
| [builtin.rs](../pkg/expression/builtin.rs)：`formal_registry::FunctionClassRegistry`、`registeredFunctionClass::getFunction`、`invoke_factory`、`CORE_BUILTIN_FACTORIES`、`BUILTIN_SPECS` | 注册表键是名称字符串，类检查参数数量后调用工厂；有名称但无工厂时明确返回“complete implementation is not linked”。`abs` 在规格表中，已读核心工厂表无对应条目；本块未证明另有动态工厂接入，不据名称宣称可求值。该表也未证明 PG 按 namespace、参数类型及方言选重载。 |
| [lib.rs](../pkg/expression/lib.rs)；[scalar_function.rs](../pkg/expression/scalar_function.rs)：`newFunctionImpl`、`NewFunction`、`ScalarFunction::EvalInt` / `EvalReal` / `EvalDecimal` | crate 挂载正式 builtin、scalar 和独立测试；构造沿 `funcs.get → getFunction → RetType`，标准构造可常量折叠，求值委托具体 builtin。此为应优先复用的标量链；没有证明 PG 身份、宽度和 typed NULL 已贯通。 |
| [builtin_math.rs](../pkg/expression/builtin_math.rs)：`abs_signature`、`abs_int` / `abs_uint` / `abs_real` / `abs_decimal` | 按共享 EvalType/unsigned 区分局部算法，`abs_int` 检查 i64 最小值；不包含 PG int4 返回身份及 int4 最小值专属检查的完成证据，不能用 i64 容器自动实现 int4 重载。 |
| [builtin_test.rs](../pkg/expression/builtin_test.rs)：`test_is_null_func`；[builtin_registry_aster_unit_test.rs](../pkg/expression/builtin_registry_aster_unit_test.rs)：`builtin_registry_matches_complete_go_function_table`；[builtin_math_test.rs](../pkg/expression/builtin_math_test.rs)：`abs_ceil_floor_and_round_cover_normal_boundary_and_overflow`；[builtin_math_16_aster_unit_test.rs](../pkg/expression/builtin_math_16_aster_unit_test.rs)：`abs_round_and_metadata_match_go_branches` | 公共测试委托注册表套件，不能将保留的 Go 注释当已执行断言；注册表测试主要核对名称，数学测试实际断言局部值、i64 溢出及 unsigned 签名意图，均不证明 PG SQL/协议全链。 |
| [pg_result.rs](../pkg/server/pg_result.rs)：`pg_type`、`encode_formats`、`value_text`、`value_binary`；[pg_types_test.rs](../pkg/server/pg_types_test.rs)：`common_type_oids_and_values`、`native_metadata_bridge_preserves_boolean_and_mysql_columns` | wire 归 server，按 ColumnInfo/NativeType 而非单元值选 OID，typmod 当前固定 -1，NULL 编码 -1；boolean 测试意图表明相同 MySQL 列和值仍需不同 native 身份。普通字面量测试预期 1 的 OID 为 20，并不是 PG int4 函数签名证据。 |
| [builtin_math.go](../pkg/expression/builtin_math.go)：`absFunctionClass.getFunction`；[builtin_math_test.go](../pkg/expression/builtin_math_test.go)：`TestAbs` | MySQL/Go 原契约依据：Int/Decimal/Real 分派、unsigned 与返回精度信息，以及 nil、正负整数/浮点断言意图；不运行 Go 测试，不将其替换为 PG 宽度规则。 |

#### 注册工厂与方言可见性

本块静态基线为 `e4915aee9239f58320be5691a5716d085e703519`（2026-10-23），已确认前块模板和第 1–3 章交付存在；`pkg/expression/doc.go` 不存在。按 RustCodeGraph 的 `status`、`explore`、定点 `node` 读取以下真实链路及独立测试，未执行测试，也未据注册规模推断支持。

| 入口/符号 | 实际责任及证据边界 |
| --- | --- |
| [builtin.rs](../pkg/expression/builtin.rs)：`FunctionClassRegistry::from_specs` / `get`、`funcs`、`extensionFuncs` | 内建表由 `BUILTIN_SPECS` 建类，并把 `CORE_BUILTIN_FACTORIES` 注入工厂表；扩展表初始为空。键为字符串，已读结构没有方言、namespace 或参数类型组成的重载键，不能证明连接方言过滤已接入。 |
| 同文件：`IsFunctionSupported`、`VerifyArgsWrapper`、`GetBuiltinList` | 前者仅查内建表；参数 wrapper 对不存在的类返回 Ok，假设调用者已查支持；列表合并内建/扩展并过滤展示名。这三种结果均不能独立证明签名合法、方言可见或可执行，扩展存在也不能由仅查内建的接口否定。 |
| 同文件：`registeredFunctionClass::getFunction`、`invoke_factory`、`registerBuiltinFactory` | 常规注册类先验参数数目，再取工厂；`invoke_factory` 对 GROUPING/FTS 有 planner bridge 特例，其余查动态工厂表，缺失时返回 registered-but-not-linked 错误。规格中的 abs 不在核心工厂表；尚未排除其他初始化动态注册，故只能作为受控缺工厂夹具候选，不能宣称生产调用必失败。 |
| [lib.rs](../pkg/expression/lib.rs)；[scalar_function.rs](../pkg/expression/scalar_function.rs)：`newFunctionImpl`、`NewFunction`、`NewFunctionBase`、`ScalarFunction::Eval` / `EvalInt` / `EvalString` | crate 挂载并导出正式构造与求值器。常规构造先查 `funcs`，再查 `extensionFuncs`，未知名按 CurrentDB 返回 No database selected 或 FUNCTION … does not exist；之后调用类工厂、接收返回类型并包装 ScalarFunction。标准构造会折叠，Base 不折叠，求值按类型委托具体 builtin；特殊 CAST 等提前分支不能当普通函数候选解析器。 |
| [builtin_test.rs](../pkg/expression/builtin_test.rs)：`test_is_null_func`；[builtin_registry_aster_unit_test.rs](../pkg/expression/builtin_registry_aster_unit_test.rs)：`builtin_registry_matches_complete_go_function_table`、`builtin_argument_ranges_match_go_unbounded_and_optional_rules`、`migrated_string_control_and_like_factories_match_go_runtime` | 公共入口委托独立注册表套件；后者分别断言名称存在/未知名、参数边界及未知名 wrapper 的 Ok、通过正式 NewFunctionBase 构造 IFNULL(NULL,'fallback') 并 EvalString 得到非 NULL fallback。名称计数与这些局部断言均不是 PG 可见性或双方言协议通过证据，本轮只读断言意图。 |

#### 重载与隐式转换选择

本块静态基线为 `83aea884a48778cc6032d28f1897acbdc56125b9`（2026-10-23），前置批次 1/2 的提交 `e4915aee92` / `83aea884a4` 及五节正文已核对。`pkg/expression/doc.go`、`pkg/server/doc.go` 均不存在。通过 RustCodeGraph status、explore、query 和定点 node 阅读真实源码及独立测试，未执行测试。

| 入口/符号 | 已读事实与限制 |
| --- | --- |
| [builtin.rs](../pkg/expression/builtin.rs)：`registeredFunctionClass::getFunction`；[scalar_function.rs](../pkg/expression/scalar_function.rs)：`newFunctionImpl` | 参数数目检查后调用 `invoke_factory`；构造沿 `funcs.get / extensionFuncs.Load → getFunction → getRetTp → ScalarFunction`，普通函数先调用 `typeInferForNull`（IF/IFNULL/NULLIF/ROW 除外），再按 fold 策略折叠。该共享流程不能作为 PG unknown、typed NULL 或按精确类型选重载的证据。 |
| [builtin.rs](../pkg/expression/builtin.rs)：`formal_registry::BuildCastFunction`；[core_support.rs](../pkg/expression/core_support.rs)：`BuildCastFunctionWithCheck`、`WrapWithCastAsInt` | 正式 CAST 固定目标元数据、继承可空性/字符属性并构造 DynamicKind::Cast，非 JSON 可折叠；wrapper 委托正式入口，ETInt 可直接返回原表达式。这是 MySQL EvalType 级包装，不能证明 PG int4/int8 身份或 E/A/I 资格。 |
| [builtin_cast.rs](../pkg/expression/builtin_cast.rs)：`FieldKind` / `FieldType`、`BuildCastFunctionWithCheck`；[builtin_cast_4_aster_unit_test.rs](../pkg/expression/builtin_cast_4_aster_unit_test.rs)：`null_and_identity_casts_are_preserved` | 局部模型在构建时保存目标类型、可空性和 in_union；测试断言 NULL 转换保留可空性、求值仍 NULL、同 Int 包装恒等。该 API 与正式 trait Expression 入口分别取证，不能自动等同或据此宣称 PG 隐式资格已接线。 |
| [pg_extended_test.rs](../pkg/server/pg_extended_test.rs)：`parse_bind_execute_sync` | 断言目录 Q 与 portal Describe/Execute 一致、零行仍有描述、显式 OID 的重复参数按编号绑定；省略 OID 的普通 `WHERE id = $1` 明确断言 0A000。仅有断言意图，没有本轮运行证据，也没有本块五组 PG 重载全链证据。 |

#### 等价共享与别名门槛

本块取证基线 `e128b200ac2d251b79f73eaad8a4f7bc9ef19c69`（2026-10-23）；批次 1–3 的提交及正文已核对，`pkg/expression/doc.go` 不存在。RustCodeGraph status、explore、query、定点 node 用于定位源码和独立测试，图未提供名称字面量位置时以 rg 补充。全部为静态阅读，未运行行为验证。

| 真实入口/符号 | 事实与证据限制 |
| --- | --- |
| [builtin_compare.rs](../pkg/expression/builtin_compare.rs)：`coalesce`、`aggregate_compare_type`、`cast_datum` | 对已物化 Datum 列表推导聚合类型，再扫描首个非 NULL，遇 Error 传播、选中后转换。Datum::Error 哨兵可表达短路意图，不证明 SQL 子表达式延迟求值、PG 分析期类型选择或 typed NULL 元数据。 |
| [builtin_control.rs](../pkg/expression/builtin_control.rs)：`infer_type_for_control`、`derive_charset` | 全 Null 类型得到 Null/binary，其他参数按本地 EvalType、flags、flen/decimal 及字符属性聚合；字符串分支可继承首个非 binary 参数 collation。这些 MySQL 局部规则不能代替 PG unknown→text、共同类型或显式 collation 冲突规则。 |
| [builtin.rs](../pkg/expression/builtin.rs)：`BUILTIN_SPECS`、`CORE_BUILTIN_FACTORIES`、`invoke_factory`；[scalar_function.rs](../pkg/expression/scalar_function.rs)：`newFunctionImpl`；[lib.rs](../pkg/expression/lib.rs) | COALESCE 规格为 1..无限参数；已读核心工厂表无 coalesce/nullif，缺工厂会拒绝。动态注册未全面排除，不能断言所有生产调用必失败。正式构造先查类、推导 NULL（NULLIF 属例外）、调用工厂并取得 RetType，随后可折叠；crate 挂载 compare/control kernel 和独立测试。未证明 PG 特殊表达式或 NULLIF 重写已贯通。 |
| [builtin_compare_test.rs](../pkg/expression/builtin_compare_test.rs)、[builtin_compare_7_aster_unit_test.rs](../pkg/expression/builtin_compare_7_aster_unit_test.rs)：`coalesce_matches_go_first_non_null_and_error_order`、`string_comparison_uses_build_context_collation_snapshot` | 前者断言 NULL/1→1、全 NULL→NULL、首个非 NULL 后 Error 被跳过、前置 Error 传播；后者通过正式 NewFunctionBase/EvalInt 检查 EQ 的构建上下文字符比较。后者不是 NULLIF 或 PG collation 全链证据。 |
| [builtin_control_test.rs](../pkg/expression/builtin_control_test.rs)：`if_and_ifnull_cover_truth_conversion_short_circuit_and_null`、`control_type_inference_keeps_width_flags_and_string_fixups` | 验证意图为 IF/IFNULL 未达 Error、全 NULL、类型 flags 与宽度；不能由 IFNULL 局部断言批准 COALESCE 别名，更没有 NULLIF 比较/提升的独立 SQL 证据。 |

#### 同名异义分流

本块取证基线 `6e15d36d648129eaf2694da2e5d3edc9274f1ff3`（2026-10-23）；批次 1–4 的提交和正文已核对，累计依赖满足。`pkg/expression/doc.go` 不存在。使用 RustCodeGraph status/explore/定点 node 阅读源码，名称字面量和模块挂载位置以 rg 补充；只读静态证据，不运行测试。

| 入口及真实符号 | 已读事实与限制 |
| --- | --- |
| [builtin_string.rs](../pkg/expression/builtin_string.rs)：`STRING_FUNCTION_SPECS`、`verifyStringFunctionArgs`、`concat` | CONCAT 规格为 1..无限；局部拼接接收已转换的可空字符串，任一 None 返回 None。不负责 PG 任意类型输出、共同元数据或正式 SQL 调用，不能直接充作 PG concat。 |
| [builtin_compare.rs](../pkg/expression/builtin_compare.rs)：`greatest` / `least` → `extremum` → `aggregate_compare_type` / `cast_datum` / `compare_non_null` | 非空局部列表遇 NULL 即返回 NULL，NULL 前 Error 传播，后方 Error 跳过；比较消费本地 collation/时态模式。局部空列表拒绝但单参数可计算，不能以此覆盖正式规格的两参数下限，也不证明 PG 共同类型/忽略 NULL。 |
| [builtin.rs](../pkg/expression/builtin.rs)：`BUILTIN_SPECS`、`CORE_BUILTIN_FACTORIES`、`invoke_factory`；[scalar_function.rs](../pkg/expression/scalar_function.rs)：`newFunctionImpl`；[lib.rs](../pkg/expression/lib.rs) | 规格 CONCAT 为 1..无限、GREATEST/LEAST 为 2..无限；已读核心工厂表无三者条目，动态注册未全面排除，不能断言所有生产调用失败。正式链仍是名称查类→参数检查→工厂→RetType→ScalarFunction/可选折叠→求值，缺工厂明确拒绝；crate 挂载局部实现及独立测试。未证明 PG 分流接线。 |
| [builtin_string_test.rs](../pkg/expression/builtin_string_test.rs)：`TestConcat` → [builtin_string_25_aster_unit_test.rs](../pkg/expression/builtin_string_25_aster_unit_test.rs)：`run_string_parity_suite` / `numeric_rounding_repeat_and_concat_preserve_edge_cases` | 真断言为 a/空串/中→a中、a/None→None；保留的 Go 混型 case 注释不是 Rust 运行断言，不能证明数值转字符串或 SQL/协议全链。模块名经 lib.rs 的 path 属性映射，不能误引不存在的同名文件。 |
| [builtin_compare_test.rs](../pkg/expression/builtin_compare_test.rs)：`greatest_and_least_preserve_go_evaluation_order_before_null`；[builtin_compare_7_aster_unit_test.rs](../pkg/expression/builtin_compare_7_aster_unit_test.rs)：`greatest_and_least_preserve_null_and_temporal_string_rules` | 真断言意图覆盖整数极值、ci 字符比较、NULL、Error 顺序和日期/字符串；均直接调用局部函数，无 PG 正式入口证明。 |
| [builtin_string.go](../pkg/expression/builtin_string.go)：`concatFunctionClass.getFunction` / `builtinConcatSig.evalString`；[builtin_compare.go](../pkg/expression/builtin_compare.go)：`resolveType4Extremum` / `greatestFunctionClass.getFunction`；[builtin_compare_test.go](../pkg/expression/builtin_compare_test.go)：`TestGreatestLeastFunc` | MySQL/TiDB 原契约依据：CONCAT 转 ETString 并保留长度/packet 限制；极值按聚合类型和时态模式构造，Go 测试含字符串/整数混型预期。这里只读，不运行 Go、不修改原语义。 |

#### 时间截断与提取

本块静态基线为 `af99cc260d`（2026-10-23）；已核对批次 1–5 的提交及正文，累计依赖满足。`pkg/expression/doc.go`、`pkg/server/doc.go` 不存在。使用 RustCodeGraph status、explore、query、定点 node 阅读以下真实源码与独立测试；图未定位名称字面量时以 rg 补查，不以说明文件或注册名称证明支持，全部测试均未运行。

| 入口/符号 | 实际责任与证据限制 |
| --- | --- |
| [builtin_time.rs](../pkg/expression/builtin_time.rs)：`extract` | 局部 kernel 输入 NaiveDateTime、返回 i64；SECOND 只取整数秒，MICROSECOND 只取秒内小数，另有 MySQL YEAR_MONTH 等复合单位。不能承接 PG SECOND 的小数、MICROSECONDS 的整秒加小数、numeric/float8 身份、timestamptz 或 interval 三分量。 |
| [builtin.rs](../pkg/expression/builtin.rs)：`extract_factory`、`CORE_BUILTIN_FACTORIES`、`CoreBuiltinKind::Extract`、`ExtractMode` | 正式 extract 已注册为两个参数，构造按单位及第二参数 EvalType 选择 Datetime/Duration/DatetimeFromString、插入 MySQL cast，返回 ETInt；evalInt 传播 NULL 并委托 `types_dependency::time::ExtractDatetimeNum/ExtractDurationNum`。不是调用上述局部 kernel 的证据；已查本文件未见 date_trunc/date_part 字面量，不能据此排除其他动态接线或宣称完整系统必失败。 |
| [scalar_function.rs](../pkg/expression/scalar_function.rs)：`newFunctionImpl`、`NewFunctionBase`；[lib.rs](../pkg/expression/lib.rs) | 正式构造经类工厂取得 RetType 包装 ScalarFunction，标准入口可折叠、Base 不折叠；crate 挂载时间 kernel、公共时间测试及独立注册表测试。应复用此链并在方言签名处分流，不以 MySQL ETInt 覆盖 PG 返回身份。 |
| [builtin_time_test.rs](../pkg/expression/builtin_time_test.rs)：`test_convert_tz`；[builtin_time_30_aster_unit_test.rs](../pkg/expression/builtin_time_30_aster_unit_test.rs)：`timezone_conversion_honors_named_and_fixed_offsets`、`run_time_parity_suite` | 前者调用 suite 并实际断言微秒跨日转换、Amsterdam 秋季重叠及春季缺口；后者断言固定/命名时区及非法区名。保留 Go 注释不是新增断言，这些 MySQL 转换断言也不是 PG date_trunc 或 DST 日桶证据。 |
| [builtin_registry_aster_unit_test.rs](../pkg/expression/builtin_registry_aster_unit_test.rs)：`extract_factory_matches_go_temporal_dispatch_and_pb_signatures` | 通过 NewFunctionBase/EvalInt 断言 YEAR→2020、HOUR→12、DAY_SECOND→260304，并检查三个 TiPB 分支；只有 MySQL 正式工厂局部断言意图，不能证明 PG numeric、时区、SQL 语法或协议。 |
| [pg_types_test.rs](../pkg/server/pg_types_test.rs)：`common_type_oids_and_values` | Q 路径断言 DATETIME→1114、DECIMAL→1700 等 OID/文本；没有本块 date_part→701、timestamptz→1184、interval→1186 的函数结果或 binary/typed NULL 专项证据。协议出口责任沿第 3 章，不能用已有类型字面量断言替代函数全链验收。 |

#### 格式化函数契约

本块静态基线为 `a485e8e82c0e977ae44600bee20982d421f20464`（2026-10-23），批次 1–6 提交与五个全局小节中的前序正文齐全，累计依赖满足。`pkg/expression/doc.go`、`pkg/server/doc.go` 不存在；经 RustCodeGraph status、explore、query、定点 node 阅读真实源码，图未覆盖名称字面量和模块路径映射时用 rg 补查。下列均为静态取证，测试未运行。

| 入口/符号 | 已核实事实及证据限制 |
| --- | --- |
| [builtin_time.rs](../pkg/expression/builtin_time.rs)：`date_format` | NaiveDateTime 到 String 的 MySQL `%` 掩码内核，含英文月份/星期、微秒及 ISO 周字段；不携带 PG timestamp/timestamptz/interval 身份、数值模板、locale 或 NULL 上下文。不能机械替换模板后作为 to_char。 |
| [builtin.rs](../pkg/expression/builtin.rs)：`BUILTIN_SPECS`、`CORE_BUILTIN_FACTORIES`、`registeredFunctionClass::getFunction`、`invoke_factory` | date_format 规格为 2 参数；已读核心工厂表无 date_format、to_char，本文件未找到 to_char 名称。常规类验证参数数目后取动态工厂，缺失时报 registered-but-not-linked；未排除其他初始化接线，不能据此断言所有 SQL 必失败。 |
| [scalar_function.rs](../pkg/expression/scalar_function.rs)：`newFunctionImpl`；[lib.rs](../pkg/expression/lib.rs) | 正式构造依次查 funcs/extensionFuncs、调用 getFunction、取返回类型并包装 ScalarFunction，标准构造可折叠；应沿此链增加方言签名策略，不另建 PG 求值器。lib 将时间测试别名挂载到独立 30 号文件。 |
| [builtin_time_test.rs](../pkg/expression/builtin_time_test.rs)：`test_date_format`、`builtin_date_format`；[builtin_time_30_aster_unit_test.rs](../pkg/expression/builtin_time_30_aster_unit_test.rs)：`run_time_parity_suite` | 前者实际只调用通用套件，原 Go DATE_FORMAT 的 NULL/模板断言与 helper 主体仍在注释；后者覆盖时间运算、TIME_FORMAT、时区等，不能当作 DATE_FORMAT 专项或 PG to_char 已执行证据。 |
| [builtin_string.rs](../pkg/expression/builtin_string.rs)：`formatByLocale`；[builtin_string_25_aster_unit_test.rs](../pkg/expression/builtin_string_25_aster_unit_test.rs)：`construction_metadata_char_format_load_and_weight_match_go` | FORMAT 是数值、decimals、可选 locale 的独立 MySQL 算法，未知 locale 回退 en_US 并报告识别标志；测试断言 en_US/de_DE 输出意图。不支持据此推断 PG D/G/L、模板语法或会话 locale。 |
| [pg_types_test.rs](../pkg/server/pg_types_test.rs)：`common_type_oids_and_values` | Q 路径对 'hello'/NULL 断言 OID 25，对普通时间/数值有其他 OID 断言；没有 to_char 重载、模板、扩展协议和 binary 结果专项证据。 |

#### 数组函数契约

本块静态基线为 `2168dce847743526d87fa6f54e401c61f6b7f32c`（2026-10-23）。批次 1–7 的连续提交及五节正文已核对，前置完成任务文件已删除，无需恢复。`pkg/types`、`pkg/expression`、`pkg/server` 均无 `doc.go`。按 RustCodeGraph status/explore/定点 node 阅读以下真实源码和独立测试；图未定位名称字面量时以 rg 补充。全部是静态取证，测试未运行。

| 入口/实际符号 | 已核实责任与证据限制 |
| --- | --- |
| [datum.rs](../pkg/types/datum.rs)：`Datum`、Kind 常量、`Clone` / `Copy` / `IsNull` | 统一盒含 Kind、字节、collation 及 Any 载荷；已读 Kind 集合未见通用 PG 数组标签，结构未直接承载元素身份、shape/lower 和元素 NULL 位图。JSON/VectorFloat32/Any 的存在不证明 PG 数组保真；不排除其他载体，记为表示交接缺口。 |
| [datum_test.rs](../pkg/types/datum_test.rs)：`test_to_json`、`test_is_null`、`test_clone_datum` | 断言意图分别含 JSON 文本数组转换、标量 NULL、字节复制；没有据此证明 PG 空数组/两层 NULL、多维或下界。 |
| [builtin.rs](../pkg/expression/builtin.rs)：`BUILTIN_SPECS`、`CORE_BUILTIN_FACTORIES`、`registeredFunctionClass::getFunction`、`invoke_factory` | 本文件未找到 array_length/cardinality/array_append 名称或核心工厂；json_array_append 是不同契约。常规链验参数数目后找工厂，已注册但缺工厂明确拒绝；未穷查动态扩展或其他 PG 路径，不能断言所有 SQL 必失败。 |
| [scalar_function.rs](../pkg/expression/scalar_function.rs)：`newFunctionImpl` | `funcs/extensionFuncs → getFunction → getRetTp → ScalarFunction`，标准构造可折叠。这是应复用的规范标量链，未证明 PG 多态元素定型、数组返回身份或函数可见性接入。 |
| [pg_result.rs](../pkg/server/pg_result.rs)：`CatalogColumnType::wire_type`、`pg_type`、`value_binary`、`encode_formats` | 目录专用 code 映射若干数组 OID，binary 分支调用 binary_array；元数据按类型而非值生成，NULL 长度 -1。这些目录桥接不能推广成任意 SQL 数组的参数、形状或函数结果能力。 |
| [pg_types_test.rs](../pkg/server/pg_types_test.rs)：`common_type_oids_and_values`、`unsigned_binary_and_unsupported_types` | 前者仅断言普通标量 Q 结果；后者明确断言原生 JSON 即使为 NULL 也不得冒充 PG 目录数组。没有三函数、通用数组结果或扩展协议专项通过证据。 |

#### 范围函数契约

本块静态基线 `df3ca02b067f05466eec8eb01b020383e9f94fb9`（2026-10-23）；批次 1–8 的连续提交和五节正文均已核对，前置任务已删除。`pkg/types`、`pkg/expression`、`pkg/server` 无 doc.go。RustCodeGraph status/explore/query/定点 node 用于读取真实源码，图未提供名称字面量时以 rg 补充；以下只证明静态事实与断言意图。

| 入口/实际符号 | 已读事实与限制 |
| --- | --- |
| [datum.rs](../pkg/types/datum.rs)：`Datum`、Kind 常量、`Clone` / `Copy`；[datum_test.rs](../pkg/types/datum_test.rs)：`test_datum` | 已读 Kind 无通用 PG range 标签；盒含字节和 Any，未直接承载 range/subtype 身份、empty、无界与开闭标志。测试检查整数/字符串/字节及 Any 存取，不证明 range 保真。第 3 章未承诺通用 range 表示，这是显式前置缺口，不能用两端点元组、JSON 或 Any 存在来判支持。 |
| [builtin.rs](../pkg/expression/builtin.rs)：`BUILTIN_SPECS`、`CORE_BUILTIN_FACTORIES`、`FunctionClassRegistry::from_specs`、`invoke_factory` | lower/upper 有单参数名称规格，但已读核心工厂表无这两个工厂或 range 工厂；本文件未找到 isempty/int4range/numrange 名称。名称不能证明 range 重载；动态工厂缺失会明确拒绝，未穷查其他初始化路径，不能断言全部 SQL 必失败。 |
| [scalar_function.rs](../pkg/expression/scalar_function.rs)：`newFunctionImpl`；[builtin_test.rs](../pkg/expression/builtin_test.rs)：`eval_builtin_func` | 正式构造取得类、调用 getFunction、读取 getRetTp 并包装 ScalarFunction，可按策略折叠；通用测试 helper 仍含迁移注释，不是 range 求值断言。应沿正式链接入已选签名，不新建 PG 求值器。 |
| [pg_result.rs](../pkg/server/pg_result.rs)：`pg_type`、`value_text`、`value_binary`；[pg_types_test.rs](../pkg/server/pg_types_test.rs)：`common_type_oids_and_values` | 已读映射为普通标量及目录专用桥接，未见 range OID 分支；文本透传不能证明 range 规范值或 binary codec。测试断言普通标量 Q 的 OID/值，没有本块 range 构造、边界、扩展协议专项证据。 |

#### 必需系统函数契约

本块静态基线 `f120e9541b21982d5033d704ea44d368f348eb00`（2026-10-23）；批次 1–9 提交及五节正文齐全，累计依赖满足。`pkg/expression/doc.go`、`pkg/server/doc.go` 均不存在。经 RustCodeGraph status、explore、定点 node 阅读真实源码；图未覆盖的名称字面量与模块挂载用 rg 补查。以下仅静态源码及测试断言意图，未运行测试。

| 实际入口/符号 | 已核实责任、客户端需求与限制 |
| --- | --- |
| [builtin_info.rs](../pkg/expression/builtin_info.rs)：`SessionInfo`、`database`、`connection_id`、`version`；[builtin_info_test.rs](../pkg/expression/builtin_info_test.rs)：`session_information_matches_go_values_and_missing_state_errors`、`expression_metadata_and_version_helpers_return_go_fields` | MySQL 局部 kernel：空 current_db→NULL，连接 u64 转 i64，版本取 MySQL ServerVersion；独立测试断言 test/空库、连接 1、版本非空。不是 PG 数据库身份、int4 backend PID 或 PG 版本返回身份的证据。 |
| [builtin.rs](../pkg/expression/builtin.rs)：`BUILTIN_SPECS`、`CORE_BUILTIN_FACTORIES`、`registeredFunctionClass::getFunction` / `invoke_factory`；[scalar_function.rs](../pkg/expression/scalar_function.rs)：`newFunctionImpl`；[lib.rs](../pkg/expression/lib.rs) | 名称查类→参数检查→工厂→RetType→ScalarFunction/可选折叠的共享链应复用；crate 挂载 info kernel 和独立测试。规格含 database/connection_id/version，已读核心工厂表没有这三项，动态注册未全面排除；没有证明五个 PG 候选走正式标量工厂。 |
| [pg_session.rs](../pkg/server/pg_session.rs)：`SessionQuery::parse` / `metadata`、`PgSession::schema` / `execute`；[pg_conn.rs](../pkg/server/pg_conn.rs) 的 Q 分支；[pg_extended.rs](../pkg/server/pg_extended.rs) 的 Parse/Execute 分支 | 专用语法只接受无 FROM 的单项 current_database()/current_schema() 及别名/pg_catalog 限定；Schema 读连接路径，Database 委托 SELECT DATABASE()；Q 与扩展查询调用 session.execute，扩展目录执行刷新 current_schema。此有界接线不是任意表达式或正式函数注册的替代品；启动 server_version 常量也不能证明 SQL version()。 |
| [pg_catalog.rs](../pkg/server/pg_catalog.rs)：`CatalogQuery::parse_session_with_types` / `expr_type` / `evaluate`；[pg_catalog_query_test.rs](../pkg/server/pg_catalog_query_test.rs)：`catalog_select_structure`、`catalog_binding_has_explicit_support` | 当前库来自上下文、schema 来自连接快照；current_database/current_catalog 限 pg_database，current_schema 限 pg_namespace，类型分支暂用 code 253。测试解析真实目录探测形状，并断言 current_database() FROM pg_locks 被 0A000 拒绝；这只是当前门禁，PG 目标允许普通表达式调用。 |
| [pg_query_test.rs](../pkg/server/pg_query_test.rs)：`pg_introspection_namespace_session_state`、`datagrip_catalog_live_metadata`、`namespace_catalog_live_metadata`；[pg_session_test.rs](../pkg/server/pg_session_test.rs)：`pg_introspection_namespace_parser_boundaries`；[pg_client_integration_test.rs](../pkg/server/pg_client_integration_test.rs) 的 libpq 目录探测脚本 | DataGrip 目录排序包含 current_database/current_schema，libpq 脚本包含 current_schema 探测，故这两项是客户端证据最强的首批；独立测试还有两连接、空路径 NULL、RESET、Parse 后 SET 再 Execute 的断言意图。version 为握手/能力探测次序候选，现有 server_version 断言只覆盖启动参数；pg_backend_pid/pg_typeof 未找到同等客户端需求断言，列为身份与类型一致性候选，不虚构需求频率或运行通过。 |

#### MySQL专属函数隔离

静态基线 `e45ddcd620396699286faabecbc8f53e2525ca1d`（2026-10-23）；批次 1–10 的连续提交和五节累计正文已核对，依赖满足。`pkg/expression/doc.go` 不存在；使用 RustCodeGraph status/explore/定点 node，名称字面量用 rg 补查。以下是源码及独立断言意图，均未运行。

| 实际入口/符号 | 已核实内容及证据边界 |
| --- | --- |
| [builtin_control.rs](../pkg/expression/builtin_control.rs)：`eval_if_null`；[builtin_control_test.rs](../pkg/expression/builtin_control_test.rs)：`if_and_ifnull_cover_truth_conversion_short_circuit_and_null` | 局部测试断言非空首参跳过 Error、NULL 回退及全 NULL；正式链另由 builtin.rs 的 ifnull_factory、CoreBuiltinKind::IfNull 和 InferType4ControlFuncs 构造/分型求值，不把局部 SqlValue helper 冒称实际工厂调用。 |
| [builtin_time.rs](../pkg/expression/builtin_time.rs)：`date_format`；[builtin_time_test.rs](../pkg/expression/builtin_time_test.rs)：`test_date_format`；[builtin_time.go](../pkg/expression/builtin_time.go)：`dateFormatFunctionClass.getFunction` / `builtinDateFormatSig.evalString` | Rust helper 接受非 NULL NaiveDateTime 与掩码；测试入口委托通用套件，DATE_FORMAT 专项 NULL/模板断言仍为注释。Go 真实工厂按 datetime/string 转换、推导模板长度，求值保留 NULL 与非法/零日期的 sql_mode 处理；helper 不能证明全部已移植。 |
| [builtin_json.rs](../pkg/expression/builtin_json.rs)：`json_extract`；[builtin_json_test.rs](../pkg/expression/builtin_json_test.rs)：`test_json_extract`；[builtin_json.go](../pkg/expression/builtin_json.go)：`jsonExtractFunctionClass` / `builtinJSONExtractSig.evalJSON` | helper 走 parsed_paths→BinaryJSON.Extract；独立测试断言多路径 [1,2]、缺失 None、非法路径 Err。Go 工厂验证类型并对文档/路径分别转换；Rust 正式 JsonExtract 分支同样逐参求值、解析路径、Extract，SQL NULL 与 JSON null 不能混为一谈。测试不证明协议或双方言门禁。 |
| [builtin_info.rs](../pkg/expression/builtin_info.rs)：`tidb_version`；[builtin_info_test.rs](../pkg/expression/builtin_info_test.rs)：`expression_metadata_and_version_helpers_return_go_fields`；[builtin_info.go](../pkg/expression/builtin_info.go)：`tidbVersionFunctionClass.getFunction` / `builtinTiDBVersionSig.evalString` | 实际存在的 TiDB 管理/构建内省函数 TIDB_VERSION()，Rust 与 Go 都取 printer.GetTiDBInfo；独立断言仅非空。Go 为 ETString、flen 按信息长度、返回非 NULL；不把 PG version() 或握手版本代入该值。 |
| [builtin.rs](../pkg/expression/builtin.rs)：`BUILTIN_SPECS`、`CORE_BUILTIN_FACTORIES`、`invoke_factory`；[scalar_function.rs](../pkg/expression/scalar_function.rs)：`newFunctionImpl` | 四名称规格为 2/2、2/2、2/不限、0/0；已读核心表包含 ifnull/json_extract，未见 date_format/tidb_version，动态注册未全面排除。正式调用仍为名称查类→参数检查→工厂→getRetTp→ScalarFunction/折叠；缺工厂明确报 implementation is not linked。存在 helper/规格不证明 SQL 可用，更不证明 PG 隐藏。 |

#### 运算符调用契约

本块基线 `66cac76435e4b8313f42e18ed8a60e3301ee6685`（2026-10-23）；已核对批次 1–11 的连续提交和对应五节正文，最近交付为 MySQL 专属函数隔离。`pkg/expression/doc.go`、`pkg/parser/doc.go` 与 `pkg/server/doc.go` 不存在。使用 RustCodeGraph status、explore、query、定点 node 读取真实源码，图未定位名称字面量时用 rg 补充；以下全为静态事实及断言意图，未执行测试。

| 真实入口/符号 | 责任与限制 |
| --- | --- |
| [builtin_arithmetic.rs](../pkg/expression/builtin_arithmetic.rs)：`ArithmeticExpr::build` / `eval`、`ArithmeticOp`、`Signature`、`EvalContext` | 局部 owned Expression 内核将 Divide 分到 Real/Decimal，将 IntDivide 分到整数/Decimal 整除；NULL 传播，除零按上下文报错或警告。不能因 IntDivide 算法存在便声称 PG `/` 的 int4 签名已接入。 |
| [builtin_compare.rs](../pkg/expression/builtin_compare.rs)：`compare_datums`、`Op::NullEq`；[builtin_op.rs](../pkg/expression/builtin_op.rs)：`logical_or`、`unary_minus_int` | 局部比较先处理 Error，再处理 NULL：NullEq 两空为 true、一空为 false，普通比较为 NULL；一元负号检查 signed/unsigned 溢出。MySQL 真值规则与 i64 容器不证明 PG boolean 或 int4 身份。 |
| [builtin.rs](../pkg/expression/builtin.rs)：`CORE_BUILTIN_FACTORIES`、`invoke_factory`、`CoreBuiltinKind::Arithmetic` / `Compare`、`eval_boolean` / `evalReal`；[scalar_function.rs](../pkg/expression/scalar_function.rs)：`newFunctionImpl` | 正式链为名称表 → getFunction → 工厂 → RetType → ScalarFunction 求值；div、intdiv、unaryminus、nulleq 有核心工厂。正式 div 选 Real 或 Decimal，nulleq 求值处理两空；已读正式 REAL 除零直接返回 NULL，不等同局部内核警告策略。两条路径分别取证，不能把局部断言当正式链或 PG resolver 证据。 |
| [lib.rs](../pkg/expression/lib.rs)、[builtin_arithmetic_test.rs](../pkg/expression/builtin_arithmetic_test.rs)、[builtin_arithmetic_2_aster_unit_test.rs](../pkg/expression/builtin_arithmetic_2_aster_unit_test.rs) | crate 挂载算术内核及独立测试入口，入口再挂 go_parity；`divide_preserves_division_by_zero_policy_overflow_and_decimal_scale` 断言除零 NULL/警告、错误模式和 1/8 的 scale 4，`integer_division_handles_four_signedness_pairs_and_minimum_overflow` 断言 -7 DIV 2 为 -3 和 i64 最小值除 -1 溢出；均非 PG SQL 证据。 |
| [builtin_op_test.rs](../pkg/expression/builtin_op_test.rs)、[builtin_compare_test.rs](../pkg/expression/builtin_compare_test.rs)、[builtin_compare_7_aster_unit_test.rs](../pkg/expression/builtin_compare_7_aster_unit_test.rs) | 前者 `unary_minus_covers_signed_unsigned_decimal_and_overflow`、`logical_operators_cover_mysql_three_valued_logic_and_short_circuit` 覆盖局部负号/逻辑；比较入口挂载的 `all_comparison_operators_match_null_and_unsigned_semantics` 覆盖两空、一空、等值和 unsigned 比较。未证明 PG IS NOT DISTINCT FROM 分析/求值或 wire 返回 boolean。 |
| [lexer.rs](../pkg/parser/lexer.rs)：`HasPipesAsConcatMode` 分支 | 未启用 PIPES_AS_CONCAT 时将 pipes token 转成 pipesAsOr；说明 MySQL `\|\|` 受 sql_mode 影响，不能从内部 concat 名称推断两入口同义。PG 语法接线和优先级归第 2 章，本块不修改解析器。 |

#### 普通聚合调用契约

静态基线 `9ee00a13fb92aae94dc0dbc4fa9cce168dc03f99`（2026-10-23）；批次 1–12 的连续提交及五节累计正文已核对，前置文档依赖满足。`pkg/expression/doc.go`、`pkg/expression/aggregation/doc.go`、`pkg/executor/doc.go` 均不存在。使用 RustCodeGraph status/explore/query/定点 node 阅读源码，模块挂载和图未表达的字面量用 rg 补查。下表仅为源码及独立测试断言意图，未执行测试。

| 实际入口/符号 | 已读责任与证据边界 |
| --- | --- |
| [descriptor.rs](../pkg/expression/aggregation/descriptor.rs)：`NewAggFuncDesc`、`AggFuncDesc::GetAggFunc` / `Split`；[base_func.rs](../pkg/expression/aggregation/base_func.rs)：`TypeInfer`、`typeInfer4Count` / `typeInfer4Sum` / `typeInfer4Avg` | 描述符含 Args/RetTp、Mode、HasDistinct、OrderByItems，未见 FILTER 字段；构造先推断类型，GetAggFunc 按名称构造 count/sum/avg，Split 为 AVG 生成 count/sum 两列的最终输入。现有类型规则为 MySQL COUNT longlong、整数 SUM/AVG decimal、浮点 double，不能证明 PG 重载或 FILTER 已接线。 |
| [aggregation.rs](../pkg/expression/aggregation/aggregation.rs)：`Aggregation`、`AggEvaluateContext`、`aggFunction::CreateContext`；[count.rs](../pkg/expression/aggregation/count.rs)：`countFunction::Update` / `GetResult`；[sum.rs](../pkg/expression/aggregation/sum.rs)：`sumFunction`；[avg.rs](../pkg/expression/aggregation/avg.rs)：`avgFunction::Update` / `GetPartialResult` / `GetResult` | 每组 context 初值 Count=0、Value=NULL，可带 DISTINCT checker；COUNT 跳过 NULL、最终阶段累加局部计数；SUM 委托 updateSum，AVG 保存并合并 count/sum，decimal 最终化使用 MySQL div_precision_increment 与 scale。不是普通标量工厂逐行求值的替代形式，也不是 PG 精度/错误证据。 |
| [builder.rs](../pkg/executor/builder.rs)：`BuildTypedPhysicalPlan`、`build_typed_physical_plan`；[typed_hash_agg.rs](../pkg/executor/typed_hash_agg.rs)：`TypedHashAgg::new` / `prepare` / `update_row`、`update_aggregate` / `finish_aggregate` | PhysicalHashAgg 传 AggFuncs、GroupByItems、context 给 TypedHashAgg；它另持每组 AggregateState，按编码 key 分组及去重，空输入仅无 GROUP BY 时补一个组；RetTp 生成 schema。已读更新路径未消费 OrderByItems，也未见逐聚合 FILTER；COUNT 使用 wrapping_add。此为可复用执行链入口，不证明 PG 分组相等、排序、溢出或两条聚合路径等价。 |
| [count_test.rs](../pkg/expression/aggregation/count_test.rs)：`dedup_mode_does_not_produce_a_count`；[avg_test.rs](../pkg/expression/aggregation/avg_test.rs)：`zero_count_float_partial_state_matches_go_ieee_division` / `zero_count_decimal_partial_state_matches_go_logged_error_result` | COUNT 仅断言 DedupMode 不计数；AVG 人工构造非 NULL sum/零 count，分别预期 infinity/非 NULL 零 decimal，不能用来证明正常空输入的 PG NULL 行为。独立测试由 aggregation/lib.rs 挂载，均未运行。 |
| [typed_hash_agg_test.rs](../pkg/executor/typed_hash_agg_test.rs)：`typed_hash_agg_groups_across_input_pages_and_pages_output`、`typed_hash_agg_scalar_empty_input_emits_count_zero` | 经 BuildTypedPhysicalPlan 构造真实 scan/HashAgg，断言跨输入页两组 count 为 2/1、空全局输入一行 count=0；executor/lib.rs 挂载。未覆盖本块 PG FILTER、排序、numeric 元数据或协议，不能据此升级 SQL 支持。 |

### 目标行为

#### 三层身份与契约模板

三层分别回答“谁计算”“SQL 调用选哪个契约”“客户端看到何种类型”。规范实现 ID 是文档中的稳定算法/语义身份，需附适用输入域和版本；不是 SQL 名称、Rust 类型、TiPB 枚举值、PG 函数对象 OID 或返回类型 OID，也不预设新增 Rust API。同一算法可承接多个已证等价的方言签名；同名不等价签名必须独立分流，同值不等于同型。第 1 章的能力 ID 标识可验收契约，与实现 ID 分别追踪。

| 层 | 标量候选贯通示例（目标，当前未验证） | 选择/拥有者与禁止推导 |
| --- | --- | --- |
| 规范实现 | 文档候选 `scalar.abs.signed.checked/v1`：已定型有符号整数的绝对值，按声明域检查结果；`abs_int` 仅为可复用的局部 i64 算法候选。 | 共享 expression 求值拥有算法；PG int4/int8 可候选共享值操作，但各自域检查和结果身份不可省略，尚未证明可直接复用。 |
| 方言签名 | `FUNC-ID-ABS/int4/pg`：可见内建 `pg_catalog.abs(int4) → int4`；关联 `FUNC-ID-ABS/int8/pg` 与 `FUNC-ID-ABS/signed/mysql`，后者 SQL 名称 ABS、按 MySQL Int 规则返回 BIGINT。这些为待新增能力记录。 | 由所属连接方言及第 2 章名称环境筛选，再按参数类型和第 3 章隐式 cast 资格选择；不能仅按 `abs` 和参数数目选择。函数重载不由期望返回 OID 或 Bind 的值决定，完整候选排序交下一块。 |
| 协议返回类型 | PG int4 结果为 OID 23、typlen 4、typmod -1；int8 为 20/8/-1。MySQL signed 结果走自身 BIGINT code 8/unsigned 标志未设置，其他长度/flags/decimals 需补参考证据。 | 第 3 章保留规范身份，server 分别生成 PG/MySQL 元数据及载荷。23/20 是结果类型 OID，不是 abs 函数对象 OID；返回 1、SQL NULL 或零行不能决定类型。 |

下表既是本章统一模板，也是上述 signed ABS 候选的首个实例。后续每个实例必须填写“方言目标、证据来源、当前缺口”三项；不适用写原因，未知区分“待定义（目标缺口）”与“未验证（当前运行缺口）”，不留空、不用裸 null。公共状态与用例关联沿第 1 章，不建立第二套证据格式。

| 必填字段 | signed ABS 候选填写、依据与缺口 |
| --- | --- |
| 关联、分类与调用模型 | 关联上表能力 ID、规范实现候选 ID、场景 ID 与基线 SHA；标量。正整数/域内绝对值是共享语义候选，开放等价范围前仍须双方言分别验收；不推广到全部数值族。来源为本块源码与第 1 章证据规则，当前运行未验证。 |
| 名称/别名与方言可见性 | PG 的 abs 及 pg_catalog 限定名，MySQL 的 ABS 及自身大小写规则分别记录；没有批准额外跨方言别名。PG 限定名、search_path 与重载规则依据官方 §10.3、第 2 章；当前两入口可见性及函数名称接线未验证。 |
| 重载与隐式 cast | 首例只接受已定型 int4/int8 的精确 PG 签名；MySQL signed 按原 Int 分支。unknown 字面量、未定型 NULL 与参数 OID 0 分开记录；候选转换只能消费第 3 章 I 资格，不能借结果 wire 扩宽放开 SQL 转换。完整歧义/候选排序本块待定义，交注册/重载块，不能套 UNION 共同类型算法。 |
| NULL 与求值条件 | PG 两个重载为 strict：合法 typed NULL 返回同型 SQL NULL，不调用非 NULL 算法；无匹配签名不能由 NULL 绕过类型检查。依据 REL_18_0 的 pg_proc.dat 与 pg_proc.h 默认值。MySQL NULL 传播目标由 Go TestAbs 的 nil 断言支持；Rust 全链 typed NULL 仍未验证。 |
| collation、时区与会话前提 | 对已定型整数的绝对值运算不适用 collation/时区（纯整数，不比较文本或处理时间）；转换或名称查找若依赖会话则另外列明，不能从算法无依赖推广为 resolver 无依赖。依据整数算法和第 2/3 章，跨连接隔离未验证。 |
| 确定性、折叠与副作用 | PG 首例目标 immutable、无数据副作用，依据 pg_proc 默认属性及 int4abs；常量输入可在规划期求值报错，不能保证错误只在 Execute。MySQL 优化属性继续沿原契约；当前折叠、缓存及方言隔离未验证。其他函数须分别记录 stable/volatile 和状态依赖，不从同名或缓存策略猜测。 |
| 返回类型、值域与修饰符 | PG int4→int4、int8→int8，结果域仍为各自 signed 域，负最小值取绝对值拒绝，NULL 不改型；MySQL Int 保持 BIGINT/unsigned 与 precision/scale 原规则。PG 依据 §9.3、REL_18_0 pg_proc.dat 与 int.c；MySQL 依据 Go 工厂。Rust PG 结果宽度保真未验证。 |
| wire 元数据与格式 | PG 23/4/-1、20/8/-1，列名由 AS v 固定；表达式来源表/属性号为 0，格式依 Q/statement Describe/portal，NULL 无载荷且长度 -1。PG RowDescription 不含可空性位，逻辑可空性单独记，不能发明 wire 字段。MySQL 独立核对 code/flags/charset/length/decimals 及 NULL；除上表 code/unsigned 目标外其精确字段待定义，需对应客户端参考记录。当前两入口均未采集。 |
| 下推与执行位置 | 默认不批准跨方言下推；须逐签名证明类型、NULL、值域/错误、collation/时区和结果元数据等价后才能批准。TiPB 签名存在、注册名存在或局部 abs_int 可用均不足；当前 PG ABS 下推等价证据未知、无性能结论。 |
| 错误字段与失败阶段 | PG int4 负最小值目标 22003；零参数 abs() 无匹配签名目标 42883。成功时 SQLSTATE/错误字段不适用。错误记录 S/V/C/M 与适用 D/H/P 的实际存在性、原 SQL 字符位置及分析/折叠/求值阶段；P 和本地映射未验证。MySQL 错误码、SQLSTATE、消息/警告按自身契约采集，不直接复用 PG 字段。依据 int.c、附录 A、§54.8；本块不规定未取证的完整消息文本或位置数值。 |
| 验收与证据状态 | 使用下方 FUNC-ID 用例族，关联源码、独立测试、官方来源及运行记录；静态源码/断言意图已定位，AsterSQL/PG/MySQL 实测均未采集。实现者须补实际构建、精确参考版本、完整命令/退出码、有效断言数量、原始输出；零用例不能升级支持。 |

官方来源（2026-10-23 查阅，版本 PostgreSQL **18**）：[§9.3 数学函数](https://www.postgresql.org/docs/18/functions-math.html)支持 ABS 同族返回；[§10.3 函数类型解析](https://www.postgresql.org/docs/18/typeconv-func.html)支持名称/参数候选、精确匹配及隐式转换筛选；[§52.39 pg_proc](https://www.postgresql.org/docs/18/catalog-pg-proc.html)定义 strict/volatility。精确 int4/int8 签名和 strict/immutable 默认属性核对官方固定 tag **REL_18_0** 的 [pg_proc.dat](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_proc.dat) 与 [pg_proc.h](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_proc.h)，结果 OID/typlen 核对同 tag [pg_type.dat](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_type.dat)，int4 最小值溢出核对 [int.c::int4abs](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/int.c)。[§54.7 消息格式](https://www.postgresql.org/docs/18/protocol-message-formats.html)支持类型/格式/NULL 字段，[§54.8 错误字段](https://www.postgresql.org/docs/18/protocol-error-fields.html)与[附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html)支持诊断记录。以上定义目标；18.6 仍仅为来源指定基线，无该补丁版本运行证据，不将 18 大版本手册或 18.0 静态源码记为 18.6 差分通过。

#### 注册工厂与方言可见性

沿用“三层身份与契约模板”，本块只定义可见候选到正式工厂的责任顺序。所属连接方言和第 2 章名称环境必须先限制可见集合，PG 再依 schema/search_path、名称与参数候选选择签名，类型转换只消费第 3 章资格；不能先在进程全局表命中另一方言实现后再靠结果 OID 修补。限定名称不能绕过方言边界；同一实现可被双方言签名引用，但别名须有逐签名等价证据。`extensionFuncs` 是共享内核的扩展注册入口，不自动授权 PG 扩展加载、目录对象或跨方言公开。

| 阶段 | 必须取得的证据；未满足时的边界 |
| --- | --- |
| 名称存在且可见 | 连接方言、原名称/限定名、名称环境和候选签名的记录；注册表存在只说明内部名称存在，隐藏名称不得泄露到 SQL 可见列表或落入另一方言候选。 |
| 参数合法 | 可见签名的参数数目、声明类型和可用隐式 cast 证据；参数数目合法不等于重载唯一，NULL 也不绕过签名选择。完整排序、unknown/多态/可变参数和歧义由下一块细化。 |
| 工厂接线 | 真实 crate 挂载、初始化顺序、类到具体工厂及返回类型传播；不能把注册列表、TiPB 签名或 `.rs.md` 当接线证据。已选签名缺工厂为实现缺口，不能改选另一方言或冒充未知名。 |
| 可求值及可交付 | 正式构造和求值的值/NULL/错误断言，再加双方言 SQL/协议类型与元数据证据；局部算法或工厂成功不覆盖 SQL 入口。缓存、折叠和跨连接复用必须保留选定方言签名，不能以值或全局当前方言选实现。 |

本块四个用例族引用首块全部必填字段，差异和缺口如下；均为待新增能力/用例，不是当前支持声明。

| 模板字段 | 本块填写与限制 |
| --- | --- |
| 关联、分类、调用模型、实现 ID | 下方 FUNC-REG 四族均为标量名称/工厂门禁契约；date_trunc 为 PG 专属，IFNULL 为 MySQL 专属，UNKNOWN 为拒绝边界，UNLINKED 为工程缺口夹具。ABS 继承首块候选 ID；date_trunc/IFNULL 具体规范实现 ID 待函数族块定义，不以 SQL 名称代替。 |
| 名称、别名、可见性、重载、cast | 以验收表明确的签名/方言为限，不批准 DATE_TRUNC 的 MySQL 别名、IFNULL 的 PG 别名或 COALESCE 改写；使用内建限定名验证 PG，不新增 resolver。测试库不含同名用户/扩展函数；精确重载、显式已定型参数优先，其他 cast/排序由下一块给证据。 |
| NULL 与求值条件 | IFNULL 的一空取后值、全空返回同型 NULL；date_trunc typed NULL 的 strict 目标待函数族官方属性补证，本块不以此作为已定义成功断言。UNKNOWN/UNLINKED 无成功值或 NULL，NULL 参数不消除门禁。ABS NULL 沿首块；所有当前全链行为未验证。 |
| collation、时区、会话 | 固定 UTF-8、PG search_path 含 pg_catalog、无同名用户函数；MySQL 选定测试库，记录 sql_mode、charset/collation。本例 timestamp without time zone 按字面时间截断，不依 TimeZone 转换；文本 IFNULL 沿 MySQL collation，精确派生规则待对应函数族取证。 |
| 确定性、折叠、副作用、下推 | 本块不新增 volatility/折叠批准，具体优化属性待函数族块；纯常量可能在规划折叠，门禁必须先于使用隐藏实现。四族无数据副作用，UNLINKED 构造拒绝；不批准跨方言下推，工厂存在不提供等价证据，无性能结论。 |
| 返回类型、wire、错误、证据 | 见下表，PG timestamp 的 OID/typlen 为 1114/8，typmod 目标 -1；MySQL 字符串精确 code/flags/charset/length/decimals 待参考采集，不能猜测。成功列名 v、表达式来源表/属性号为 0；拒绝没有成功列/载荷。PG 错误沿首块 S/V/C/M 与适用 D/H/P，精确消息/P/失败阶段和 MySQL 诊断仍待采集；各族当前为静态取证与文档目标，运行未验证。 |

官方依据（2026-10-23 查阅，PostgreSQL **18**）：[§10.3 函数解析](https://www.postgresql.org/docs/18/typeconv-func.html)定义可见候选与精确/隐式转换匹配；[§9.9 日期时间函数](https://www.postgresql.org/docs/18/functions-datetime.html#FUNCTIONS-DATETIME-TRUNC)定义 date_trunc(text,timestamp) 的返回类型与截断行为；[§9.18 条件表达式](https://www.postgresql.org/docs/18/functions-conditional.html#FUNCTIONS-COALESCE-NVL-IFNULL)将 IFNULL 列为其他数据库的类似能力，并说明 COALESCE 并非普通函数，本块据此不把它作为开放 PG IFNULL 别名的依据。timestamp 的 OID/长度由官方 **REL_18_0** [pg_type.dat](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_type.dat)核对；[附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html)支持 42883/0A000 分类。另一入口隐藏名称是 AsterSQL 的目标边界，不能只从 PG 手册推断 MySQL 支持集合。18 大版本手册和固定 18.0 源码均未提供本轮 18.6 运行差分证据。

#### 重载与隐式转换选择

继承首块模板及[显式隐式转换契约](#显式隐式转换契约)，只定义普通函数签名选择。先由前块过滤方言、名称环境与可见候选，再根据实参数量和声明输入类型选择；返回类型、期望 wire OID、参数值及注册顺序均不用于打破歧义。函数式类型转换的单参数特殊语法归第 2/3 章，不把它当普通重载。

候选阶段消费第 3 章 I 资格：精确签名优先；没有精确签名时剔除不能隐式转换的候选，依次比较精确位置数、需转换位置的类别优选类型。unknown 不构成精确匹配；仍有多个候选时按各 unknown 位置的 string 类偏好、统一类别及类别优选类型筛选；已知参数全为同型时再尝试用该型约束 unknown，仍不唯一则拒绝。不能把 UNION/CASE 共同类型推导直接套在函数候选上，不能以全局“最低 cast 成本”代替官方顺序。domain、多态和用户类型未开放时保留能力门禁，不从基础类型规则推导已支持。

unknown 字符串、未定型 NULL 和未指定 OID 的参数分别保留来源。typed NULL 带已知类型参加同一筛选，合法 strict 签名才传播同型 NULL，非法类型不因空值放行。参数在 Parse/Prepare 分析时定型，Bind 只解码既定类型；换值、NULL、文本/二进制格式不得重新选签名。候选选择之后才插入合法 coercion、执行输入解析和域检查；例如已选数值签名后文本内容非法是转换错误，不能失败后试另一个重载。折叠可提前触发值错误，不承诺都延后 Execute。

可变参数先按调用实参数量将声明数组参数展开为元素签名；显式 VARIADIC 保留数组调用、不做该展开。同 schema 展开后同输入签名的非可变函数优先，跨 schema 同签名按路径；不同输入签名不能只因路径靠前获胜。默认参数允许省略可默认位置，但同 schema 同剩余输入签名仍可能歧义。这些是候选规则，不授权本轮实现 UDF、默认参数、任意 schema 或多态；缺所需载体时标实现缺口，不能按名称和 min/max 参数数目冒充完成。

本块五组实例统一引用“三层身份与契约模板”，字段补充如下：

| 模板字段 | 本块实例差异、来源及缺口 |
| --- | --- |
| 关联、分类、调用模型、实现 ID | FUNC-RES 五组均为标量选择契约；ABS 复用首块 `scalar.abs.signed.checked/v1` 候选；SUBSTR 仅作 unknown 选择代表，实际规范实现 ID 待函数族取证；AMBIG 的两签名仅为待新增解析夹具，非生产函数或支持声明。 |
| 名称/别名/可见性、重载、cast | PG 使用 pg_catalog 内建，AMBIG 使用受控候选集；无新增别名。MySQL 走自身 ABS/SUBSTR，不应用 PG OID/候选优选。候选顺序依据 PG18 §10.3；当前双方言解析、工厂与完整参数定型未验证。 |
| NULL、返回类型、wire、错误 | 见验收表；ABS strict 属性继承首块来源。成功一列 v，PG 表/属性号 0、typmod -1，NULL 长度 -1，逻辑可空性另记。拒绝项无成功值、NULL 或结果载荷。PG 42883 为无匹配，42725 为歧义，参数不能定型/冲突沿第 3 章，不能把合法未实现统一伪装成参考端无匹配；P/完整消息及具体失败阶段待采集。 |
| collation、时区、会话、确定性 | 固定 UTF-8、PG 内建可见且无同名遮蔽；整数 ABS 不涉及 collation/时区，SUBSTR ASCII 输入排除 locale 差异但其完整排序规则未取证。ABS immutable 继承首块；SUBSTR 与夹具的 volatility/折叠批准不由本块推定，夹具仅解析不求值。MySQL 记录自身 sql_mode/charset/collation，运行隔离未验证。 |
| 下推、副作用、证据 | 本块不批准下推或新增折叠优化；成功 SELECT 无数据副作用，夹具解析无生产写入。源码与测试只读，PG/AsterSQL/MySQL 运行及性能均未验证。完整 wire 字段、错误字段继承首块，MySQL 精确 flags/charset/length/decimals 和诊断需补参考记录，不留作默认等价。 |

官方依据（2026-10-23 查阅，PostgreSQL **18**）：[§10.3 Functions](https://www.postgresql.org/docs/18/typeconv-func.html) 给出上述签名选择顺序、variadic/default 规则及 substr unknown 例；[§9.3 数学函数](https://www.postgresql.org/docs/18/functions-math.html)支持 ABS 输入/返回同族；[PREPARE](https://www.postgresql.org/docs/18/sql-prepare.html)支持参数由上下文定型；[§54.2.3 扩展查询](https://www.postgresql.org/docs/18/protocol-flow.html#PROTOCOL-FLOW-EXT-QUERY)与[§54.7 消息格式](https://www.postgresql.org/docs/18/protocol-message-formats.html)支持分析、绑定、描述与执行分责；[附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html)核对 42883/42725/42P18/42P08 分类。上述文档规则和受控候选推导是目标依据；未采集 18.6 参考端，不能记为补丁版本差分通过。

#### 等价共享与别名门槛

COALESCE 与 NULLIF 都只是**等价共享候选**，本块无批准项。批准单位是已解析参数类型向量、结果身份及适用输入域，不是函数名；COALESCE 还记录参数数目、顺序和共同类型，NULLIF 记录选中 `=` 的左右签名及其转换。即使复用算法，也分别保留方言名称、类型选择和 wire 映射。PG COALESCE 是特殊条件表达式，不按普通函数重载查找，不能将前块普通函数排序或 `pg_catalog.coalesce(...)` 当作等价入口，也不接受显式 VARIADIC 数组；NULLIF 的比较资格按运算符规则消费，不能硬套 COALESCE 的共同类型。

逐签名批准必须具备：双方言自身参考端和 AsterSQL 正式 SQL 入口的值/NULL、共同类型或比较提升、返回宽度/精度、求值顺序与错误时点、collation、wire、折叠/缓存及适用下推证据；记录完整版本、会话环境和命令。字段缺证保持“目标/待证”，不等价则保留方言分流。批准某个 int4/int4 域不授权 int8、numeric、text、unknown、其他参数数目或别名；IFNULL 的两个参数与 COALESCE 的相似算法不足以在 PG 公开 IFNULL。

以下实例引用“三层身份与契约模板”的全部字段，不适用和缺口明确如下。

| 模板字段 | 本块约束与状态 |
| --- | --- |
| 能力/用例、分类、调用模型、规范实现 ID | FUNC-EQUIV 五族，分类为第一类候选；标量结果条件表达式。候选语义 ID `conditional.first-non-null/v1`、`conditional.null-if-equal/v1` 为文档身份，非已注册符号。共享必须保留惰性子表达式边界；不能以预先计算全部参数的普通调用 ABI 代替。 |
| 名称、别名、方言可见性、重载、cast | 两入口目标开放自身 COALESCE/NULLIF 拼写，无新增别名；PG COALESCE 按共同类型、NULLIF 按 `=` 及其第一操作数提升，MySQL 保持自身转换/返回规则，不能套 PG OID。unknown、typed NULL 和参数分别消费第 3 章，不按运行值选型。MySQL 精确混型签名/元数据待参考端核对。 |
| NULL、求值顺序、返回身份 | COALESCE 左到右至首个非 NULL，全 NULL 返回分析固定类型的 NULL；非 strict，不能遇任意 NULL 就返回。NULLIF 相等返回 NULL，否则返回隐含比较后的第一参数；NULLIF(1,2.2) 的 PG 结果是 numeric，不能固定原始 int4。任一比较参数 NULL 时不构成 true；NULLIF(NULL,1)→NULL、NULLIF(1,NULL)→1，均 int4。具体输入见验收表。 |
| collation、时区、会话 | 数值例无时区/collation 运算；text 例显式固定 PG UTF-8/C 与 MySQL utf8mb4_bin，并采集 provider/版本及结果 collation。COALESCE 不比较文本仍须推导输出 collation，NULLIF 比较须消费所选 `=` 的 collation；不能以首个返回字符串决定规则。时间类型不在本块域内。名称/方言隔离沿前块。 |
| 确定性、折叠、副作用、下推 | 条件结构不一概标 immutable/strict，属性继承实际子表达式及比较操作符。验收 SELECT 无数据写入；以行变量避免把运行期短路测试退化成常量折叠，另采规划期拒绝。没有批准重排、重复求值、跨方言下推或新增折叠优化；volatile/副作用子表达式的完整开放仍须独立证据。 |
| wire、错误与证据 | PG 成功一列 v，表/属性号 0，示例 typmod -1；int4 为 23/4/-1、text 为 25/-1/-1、numeric 为 1700/-1/-1，NULL 长度 -1。格式/Describe 沿首块；逻辑可空性另记。错误记录 S/V/C/M 与适用 D/H/P、分析/规划/执行时点，完整消息/P 待采；MySQL code/flags/charset/length/decimals、错误/警告均待自身参考采集。无运行或性能证据。 |

官方依据（2026-10-23 查阅，PostgreSQL **18**）：[§9.18 条件表达式](https://www.postgresql.org/docs/18/functions-conditional.html)给出 COALESCE 短路、共同结果类型及 NULLIF 的隐含比较/返回提升；[§10.5 共同类型](https://www.postgresql.org/docs/18/typeconv-union-case.html)用于 COALESCE 的 unknown 与已定型输入选择；[§23.2 collation](https://www.postgresql.org/docs/18/collation.html)规定显式冲突、输出派生及 C 排序。常量折叠与运行顺序边界另据 [§4.2.14 表达式求值规则](https://www.postgresql.org/docs/18/sql-expressions.html#SYNTAX-EXPRESS-EVAL)，错误分类沿本章[附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html)来源。表中推导均为待验收目标；未采集 18.6 参考端，不声称补丁版本差分通过。

#### 同名异义分流

同名拼写不是共享签名。CONCAT 的 NULL 策略、类型到文本的转换、返回身份须按连接方言分别选择；GREATEST/LEAST 还要分开参数下限、共同类型及比较规则。可复用已定型非 NULL 字符串缓冲拼接、或已获等价证明的比较/极值步骤；NULL 策略、coercion、返回类型和错误必须留在各自方言契约边界，不能全局修改现有 MySQL kernel 来满足 PG。

PG GREATEST/LEAST 是特殊表达式，不是普通 pg_catalog 函数重载，不接受显式 VARIADIC 数组；普通函数候选排序不直接适用。PG concat 是普通可变参数函数，不能与同样跳过 NULL 的 MySQL CONCAT_WS 当别名，后者另有分隔符契约。以下实例引用首块全部模板字段，不批准任何新别名或下推。

| 模板字段 | 本块方言目标、来源与当前缺口 |
| --- | --- |
| 关联、分类、调用模型、规范实现 ID | 第二类“同名异义”，关联 FUNC-SPLIT 四族。文档候选 `string.concat.skip-null/pg/v1` 与 `string.concat.propagate-null/mysql/v1`、`extremum.ignore-null/pg/v1` 与 `extremum.propagate-null/mysql/v1` 分离；最大/最小和参数类型向量另记。标量结果，PG 极值为特殊表达式；这些 ID 不是已注册 Rust API。 |
| 名称、别名、可见性、重载、隐式 cast | 两入口均目标可见自身 concat/greatest/least；PG concat 可用 pg_catalog 限定名，极值用非限定特殊语法。PG concat 使用各参数类型的文本输出，不要求参数共同类型；极值依 §10.5 在分析期确定共同类型并只用第 3 章 I 资格。MySQL 保持 ETString/聚合类型及自身转换。unknown/typed NULL/参数不按运行值选型；完整可变数组边界另验，无新增别名。当前正式接线未验证。 |
| NULL、求值条件、返回身份和值域 | PG concat 忽略 NULL，全 NULL 的普通调用返回非 NULL 空串/text；MySQL 任一 NULL 返回 NULL/自身字符串类型。PG 极值忽略 NULL，全 NULL 返回共同类型的 SQL NULL；MySQL 任一 NULL 传播。PG 比较需要所有非 NULL 候选，不承诺沿用 MySQL 遇 NULL 停止；错误顺序不从局部 Datum 哨兵推导 SQL 行为。PG 极值返回共同类型，不根据胜出值重新定型。依据官方规则及局部源码；全链未验证。 |
| collation、时区、会话 | 拼接不排序，但结果字符属性须推导；极值按输入派生 collation 比较。文本域固定 PG UTF-8/C、MySQL utf8mb4_bin，并记录 provider/版本。数值/文本代表无时间运算，时区对本组不适用；不推广到其他类型的输出函数，PG concat 的任意类型输出可能依赖会话。MySQL packet/sql_mode/字符设置另采，不能套给 PG。 |
| 确定性、折叠、副作用 | REL_18_0 的 concat 为非 strict、stable，不能按纯缓冲算法标为 immutable；极值结构继承子表达式/比较的属性，无独立 pg_proc volatility。验收 SELECT 无写入；不批准重排、重复求值或新增折叠规则。MySQL 保留原优化属性，完整 volatile/错误阶段需后续取证。 |
| 返回修饰符、wire、错误 | 本组 PG text 为 25/-1/-1、int4 为 23/4/-1、numeric 为 1700/-1/-1（OID/typlen/typmod），列 v、表/属性号 0，NULL 长度 -1；逻辑可空性独立记录。MySQL 数值目标 BIGINT/DECIMAL，字符串保持自身列定义；code/flags/charset/length/decimals 与精确 precision/scale 待参考采集，不由 PG 类型倒推。错误采 S/V/C/M 和适用 D/H/P、实际阶段；成功无错误字段。目标码见验收，完整消息/P 未采集。 |
| 下推、验收、证据状态 | 只允许未来逐签名/输入域证明等价后共享非 NULL 算法；TiPB 名称不能授权 PG NULL/coercion 下推。四族逐行核对两入口，当前只有静态源码、断言意图和官方目标，无参考端/AsterSQL 实测，无性能结论。 |

官方依据（2026-10-23 查阅，版本 PostgreSQL **18**）：[§9.4 字符串函数](https://www.postgresql.org/docs/18/functions-string.html)定义 concat 文本输出和忽略 NULL；[§9.18.4 条件表达式](https://www.postgresql.org/docs/18/functions-conditional.html#FUNCTIONS-GREATEST-LEAST)定义极值的特殊调用、NULL 与共同类型；[§10.5 类型选择](https://www.postgresql.org/docs/18/typeconv-union-case.html)支撑混型/全 unknown 推导；[§23.2 collation](https://www.postgresql.org/docs/18/collation.html)支撑 C 排序与派生。固定 **REL_18_0** 的 [pg_proc.dat](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_proc.dat)核对 concat 的 any 可变参数、text、非 strict/stable；[gram.y](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/parser/gram.y)的 GREATEST/LEAST expr_list 产生 MinMaxExpr，支持单参数和空列表语法拒绝的目标推导。错误分类沿首块附录 A；这些均非 18.6 补丁版本实测。

#### 时间截断与提取

本块引用首块统一模板；PG 核心专属的是 date_trunc/date_part 及 PG 的提取语义，EXTRACT 语法在 MySQL 入口也可见但必须分流。EXTRACT 的 AST/字段语法交第 2 章，timestamp/timestamptz/interval 身份及 codec 交第 3 章，本块只规定已定型表达式的语义交接，不批准把 PG EXTRACT 别名到现有整数工厂。

| 模板字段 | 方言目标、依据与当前缺口 |
| --- | --- |
| 关联、分类、模型、实现 ID | 待新增能力族 FUNC-TEMP、标量。文档规范候选 `scalar.pg.temporal.truncate/{timestamp,timestamptz,interval}/v1` 与 `scalar.pg.temporal.field/{timestamp,timestamptz,interval}/{numeric,float8}/v1` 分开保留返回策略；不是现有 API。date_part/EXTRACT 可候选共享字段算法，精度和身份仍独立，未批准共享实现。 |
| 名称/别名、可见性 | PG 普通调用 date_trunc/date_part 支持 pg_catalog 限定及第 2 章名称环境；EXTRACT(field FROM source) 使用语法交接。MySQL 不开放 date_trunc/date_part，保留自身 EXTRACT；隔离为 AsterSQL 设计门禁，MySQL 参考端名称拒绝细节未采。无新增跨方言别名。 |
| 重载、cast | date_trunc(text,timestamp)→timestamp、(text,timestamptz)→timestamptz、(text,timestamptz,text zone)→timestamptz、(text,interval)→interval；三参显式区域属于 timestamptz 签名，timestamp/interval 本身没有带 zone 的截断重载。date_part(text,T)→float8、EXTRACT 对应已定型 T→numeric，T 为本块三种类型。参数先按前块及第 3 章 I 资格选择，不靠值猜类型；显式 timestamp→timestamptz 转换的时区依赖必须保留。date/time 自动转换遵循官方规则但不是本块代表验收域，不扩展到 timetz。 |
| 单位、值域与精度 | timestamp 截断支持 microseconds、milliseconds、second、minute、hour、day、week、month、quarter、year、decade、century、millennium，低位清零，日/月必要时置 1，week 从 ISO 周一开始；interval 按分量截断，不支持 week。SECOND 提取包含小数，MICROSECONDS 包含完整秒乘一百万；interval 的 month 是年内余月、day 是日分量，不能将月固定折秒后再提字段。date_part 浮点可能损精度，EXTRACT 以 numeric 保留微秒；截断不是四舍五入。非有限值沿第 3 章保留：合法截断保留 infinity；提取单调字段返回对应 infinity，其他合法字段为 NULL，不能误作 strict 导致的 NULL；具体扩展域需后续聚焦补证。 |
| NULL 与求值 | 本块各签名 strict，任一已定型实参 NULL→其固定返回类型的 SQL NULL；三参 zone=NULL 也为 NULL，不调用非 NULL 内核。NULL 不绕过签名/语法分析；不保证其他实参的分析或折叠错误被跳过。依据 pg_proc.dat 与 pg_proc.h 默认属性，当前全链未验证。 |
| collation、时区、会话 | 不做用户文本排序，collation 对时间计算不适用，字段名按 PG 单位解码而非 locale 比较；固定 UTF-8、DateStyle=ISO,YMD、IntervalStyle=postgres，记录 TimeZone、tzdata 与输入 p。timestamp/interval 按原字段，timestamptz 默认用会话区域；三参截断用指定区域求瞬时，结果显示仍用会话区域。提取 timestamptz 的 HOUR/timezone 等依会话区域，epoch 是绝对瞬时秒数；timestamp epoch 是忽略时区/DST 的名义秒数，二者不能互换。 |
| 确定性、折叠、副作用 | REL_18_0：两参 timestamptz 截断、timestamptz 提取为 stable；显式 zone 的三参截断、timestamp/interval 截断及提取为 immutable。无数据副作用；折叠与计划缓存须保持这些属性及区域配置边界，不从 kernel 无状态推断全部 immutable。当前缓存/折叠隔离未验证。 |
| 返回、wire 与修饰符 | 按签名固定结果，未额外 cast 的本块表达式目标 OID/typlen/typmod：timestamp 1114/8/-1、timestamptz 1184/8/-1、interval 1186/16/-1、date_part float8 701/8/-1、EXTRACT numeric 1700/-1/-1。不能由输入 p 或截断单位重造输出 typmod，numeric 的显示 scale 不等于 typmod。列名 AS v、来源表/属性号 0、格式与 NULL 长度 -1 沿首块；MySQL EXTRACT 走自身整数/复合单位和列定义，精确 flags/length/decimals 待参考采集。所有本块函数 wire 仍缺运行证据。 |
| 下推、错误与证据 | 默认不批准 PG 下推；现有 ExtractDatetime/Duration 的 TiPB 分支不证明 PG 精度/时区等价。未知字段目标 22023，已识别但类型不支持如 interval 的 week 截断、timestamp 的 timezone 提取目标 0A000；无匹配签名 42883，时区错误与输入/范围错误分开采集。错误 S/V/C/M、适用 D/H/P、阶段按首块，精确消息/位置和本地映射未验证。下方五族均为目标/静态意图，无功能或性能通过证据。 |

官方来源（2026-10-23 查阅，PostgreSQL **18**）：[§9.9.1 EXTRACT/date_part](https://www.postgresql.org/docs/18/functions-datetime.html#FUNCTIONS-DATETIME-EXTRACT)分别明确 numeric 与 double precision 返回、字段及精度区别；[§9.9.2 date_trunc](https://www.postgresql.org/docs/18/functions-datetime.html#FUNCTIONS-DATETIME-TRUNC)定义类型、单位和区域行为。精确重载、返回和 volatility 核对官方固定 **REL_18_0** [pg_proc.dat](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_proc.dat)，strict/immutable 缺省核对 [pg_proc.h](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_proc.h)；单位适用性、错误及字段算法核对 [timestamp.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/timestamp.c) 的 timestamp_trunc、timestamptz_trunc_internal、interval_trunc、timestamp_part_common、timestamptz_part_common。OID/长度核对 [pg_type.dat](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_type.dat)，typmod/格式沿第 3 章。以下 DST 数值为按区域规则推导的验收目标，尚无参考端采样；18.6 仍仅是来源指定目标，18 手册及 18.0 静态源码不能记作该补丁版本运行证据。

#### 格式化函数契约

引用首块统一模板，to_char 是 PG 核心专属标量，DATE_FORMAT 是 MySQL 专属；共享日历/数字组件只能作为候选，不能开放跨方言别名。下列实现 ID 仅为文档身份，不预设新 API。

| 模板字段 | 方言目标、依据及当前缺口 |
| --- | --- |
| 关联、分类、调用模型、实现 ID | 待新增 FUNC-FMT 五族；标量候选 `scalar.pg.format.temporal/v1` 与 `scalar.pg.format.numeric/v1`，按已选源类型保留各自语义；MySQL `scalar.mysql.date_format/v1` 独立。当前仅源码/断言意图与官方规则，无运行证据。 |
| 名称/别名与可见性 | PG to_char/pg_catalog.to_char 遵循第 2 章名称环境，MySQL 隐藏；DATE_FORMAT 在 MySQL 保留，在 PG 隐藏。该隔离为 AsterSQL 设计门禁，不推断参考端自定义函数不存在；无新增别名。 |
| 重载与隐式 cast | 精确签名 `(T,text)→text`，T 为 timestamp、timestamptz、interval、int4、int8、numeric、float4、float8。无直接 text 源重载；date/int2 等候选必须经前块 resolver 和第 3 章 I 资格，不能把 MySQL 字符串日期宽松转换搬过来。unknown/NULL/参数 OID 0 不由值猜重载；代表用例均显式定型。 |
| 模板与修饰符 | 时间与数值模板各自解析，不能互换；代表 YYYY/MM/DD、HH24/MI/SS/US、ISO IYYY/IW、数值 9/0/小数/符号、字面量、FM/TH/TM 见下方验收。FM 对时间只修饰下一项；数值 FM 是数值格式模式，不能套时间规则。FX 的输入解析约束不属于本块，SP 未实现不得承诺。 |
| NULL 与求值 | 所列重载均 strict，已定型任一参数 NULL→NULL/text；NULL 不绕过签名分析或保证其他实参不折叠。时间源空模板及非有限时间值返回 NULL 的额外边界来自 formatting.c，不能把此行为推广为所有数值空模板策略。 |
| collation、locale/时区 | UTF-8；TM 月/日名称依 lc_time，D/G/S 依 lc_numeric，L 依 lc_monetary；固定标点模板 . 和 , 不随 locale 换字符。记录输入/输出 collation，第 3 章派生规则保留，不把排序 collation 等同 locale 设置。timestamptz 先按会话 TimeZone 映射墙钟和偏移，timestamp/interval 保留自身字段；记录 tzdata、OS locale 库与可用名称。 |
| 确定性、折叠、副作用 | 官方目录将上述全部 to_char 重载标 stable，不能因某个纯数字模板看似固定便整体标 immutable。无数据副作用；准备语句跨执行的 locale/TimeZone 改变不得被旧结果缓存掩盖。折叠/缓存隔离未验证。 |
| 返回、wire、修饰符 | 所有成功重载均 text，PG OID/typlen/typmod=25/-1/-1，不是 varchar(n)，模板长度/输入精度不生成 typmod。列名 AS v、表/属性号 0，text/binary 均为编码后的文本字节；空格/尾零不得 trim。NULL 长度 -1、零行描述和 statement/portal 格式沿首块，可空性单记；MySQL 字符串使用自身 code/flags/charset/length/decimals，精确字段待采。 |
| 下推、错误与状态 | 无下推批准；日期百分号/FORMAT locale 内核不能证明 PG 等价。非法数值模板 V 与小数点组合→42601（formatting.c::NUMDesc_prepare），无匹配签名→42883；普通未知文本按字面量处理，不能一概报非法模板。错误采 S/V/C/M、适用 D/H/P 与分析/折叠/执行阶段；确切消息/位置、本地映射及 MySQL 自身警告/错误待采。 |
| 验收/证据状态 | 下方五族分别记录两入口输入、值/NULL/类型/错误及可见性；全部待新增、未运行，缺参考端 18.6 实测，成功项 SQLSTATE 不适用。不得据文档结构或名称表升级支持。 |

官方依据（2026-10-23 查阅，PostgreSQL **18**）：[§9.8 格式化函数及表 9.26–9.31](https://www.postgresql.org/docs/18/functions-formatting.html)给出模板和示例；[§19.11.2 locale 设置](https://www.postgresql.org/docs/18/runtime-config-client.html#RUNTIME-CONFIG-CLIENT-FORMAT)定义配置前提。精确签名/stable 核对固定 **REL_18_0** [pg_proc.dat](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_proc.dat)，strict 缺省核对 [pg_proc.h](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_proc.h)。[formatting.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/formatting.c)的 timestamp_to_char、timestamptz_to_char、interval_to_char、NUMDesc_prepare 支持时间边界和错误；结果身份/协议沿已取证的第 3 章 text 契约。18.6 仍仅为来源指定目标，18 手册和 18.0 静态源码不构成该补丁版本差分记录。

#### 数组函数契约

以 array_length、cardinality、array_append 为 PG 核心专属的**普通标量函数**目标，不承诺全量数组库；三层模板沿首块，规范实现候选分别为 `scalar.array.dimension-length/v1`、`scalar.array.cardinality/v1`、`scalar.array.append-element/v1`，均为文档身份，尚无获准共享项。值表示、元素族/typmod、矩形 shape、下界、空数组与两层 NULL 继承第 3 章基础数组契约。维数不改变数组类型 OID；不将多维值建模为“数组的数组”。

| 模板字段 | 本块方言目标、来源与缺口 |
| --- | --- |
| 关联/分类/调用模型、名称/别名/可见性 | FUNC-ARR 五组关联上述候选；PG 可见非限定名与 pg_catalog 限定名，MySQL 隐藏三个 PG 名称；无新增别名。普通标量一输入行对应一个结果，unnest 不适用本模型。依据官方签名和前块名称规则；本地正式接线未验证。 |
| 重载、隐式 cast、元素定型 | `array_length(anyarray,int4)→int4`、`cardinality(anyarray)→int4`；`array_append(anycompatiblearray,anycompatible)→anycompatiblearray`。前两者消费已知数组元素身份；后者在分析时将数组元素与追加元素按第 3 章 I 资格定共同元素类型，再转换整数组及元素，返回该元素对应数组。多态伪类型不是 wire OID；unknown/typed NULL 不按运行值猜型。本块只实例化 int4[]，并用 int4[]+int8 检查提升，不补全多态系统；不兼容的定型类型对不可因 NULL 绕过。 |
| NULL | array_length/cardinality strict：数组 SQL NULL 得 SQL NULL；array_length 的维号 NULL 也返回 NULL。array_append 非 strict：NULL 数组作为空数组，NULL 元素作为一个真实空位追加；空数组、数组 NULL、元素 NULL 不互换。依据固定 tag 目录/实现；当前未实测。 |
| collation、时区、会话 | 对已定型数组的计数/形状和本块 int4/int8 追加不适用 collation/时区（不比较文本、不处理时刻）；元素输入、输出及 cast 仍沿第 3 章元素族设置。名称解析沿连接上下文，不能由纯算法推导全链无会话依赖；跨连接隔离未验证。 |
| 确定性、折叠、副作用 | 三者目标 immutable，无 SQL 数据副作用，常量可规划期折叠；不能把内部原地优化理解为修改输入 SQL 值。本地折叠、缓存及错误阶段未验证。 |
| 返回类型、值域与 wire | 两计数结果 int4，23/4/-1；append 本组 int4[] 为 1007/-1/-1，提升 int8[] 为 1016/-1/-1，分别依赖元素 23/20 与数组 OID 配对。AS v、来源表/属性号 0；NULL 或零行不改身份。shape/lower 属值而非 OID/typmod；binary 数组头的元素 OID、维长/下界、元素长度/NULL 及两格式输出沿第 3 章。RowDescription 无可空性位；逻辑可空性另记。当前返回身份/codec 贯通未知，MySQL 不暴露这些 PG 数组 OID。 |
| 下推/执行位置 | 默认无下推或跨方言别名批准，须逐签名证明形状、NULL、cast、错误及元数据等价；现有 JSON append、目录 codec 均不足以批准。性能不适用本轮文档检查，无运行结论。 |
| 错误、验收与证据 | 五组成功无错误；无效维号返回 NULL 是成功。多维 append 目标 22000，下界追加索引溢出 22003；不匹配名称/签名沿 resolver，精确混型诊断另采，不统一套某个错误码。错误采首块 S/V/C/M 与适用 D/H/P、原 SQL 位置和分析/折叠/执行阶段，无伪成功行。FUNC-ARR 均待新增、未运行；官方目标与源码/断言意图不能代替双方言当前记录。 |

官方来源（2026-10-23 查阅）：PostgreSQL **18** [§9.19 数组函数](https://www.postgresql.org/docs/18/functions-array.html)定义代表函数，[§8.15 数组](https://www.postgresql.org/docs/18/arrays.html)支持形状/下界与 NULL，[§36.2.5 多态](https://www.postgresql.org/docs/18/extend-type-system.html#EXTEND-TYPES-POLYMORPHIC)支持分析期共同元素类型。固定 **REL_18_0** 的 [pg_proc.dat](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_proc.dat)与[pg_proc.h](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_proc.h)核对签名、strict/immutable；[arrayfuncs.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/arrayfuncs.c)::array_length/array_cardinality 核对维号与计数，[array_userfuncs.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/array_userfuncs.c)::fetch_array_arg_replace_nulls/array_append 核对 NULL、下界和错误分支。数值例为上述规则推导的待验收目标；18.6 仍为来源指定基线，未在该补丁版本实测。

#### 范围函数契约

沿首块统一模板，本块仅实例化 PG 核心专属普通标量 lower/upper/isempty 与 int4range 构造，以 numrange 作连续域对照；实现 ID 候选 `scalar.pg.range.bound/v1`、`scalar.pg.range.empty/v1`、`scalar.pg.range.construct/v1` 仅为文档身份。range 是单个 SQL 值，不是行集合或两个独立标量；multirange 不在本块。

| 模板字段 | 本块目标、依据及缺口 |
| --- | --- |
| 关联/分类/名称/别名/可见性 | 待新增 FUNC-RNG 四组；PG 非限定名及 pg_catalog 限定名沿第 2 章名称环境可见。MySQL 隐藏 range 重载、isempty/int4range/numrange，保留 lower/upper 自身字符串调用；不得按名称全局屏蔽或把字符串大小写函数当边界函数。无共享别名批准，当前正式接线未验证。 |
| 重载/隐式 cast/值域 | `lower(anyrange)→anyelement`、`upper(anyrange)→anyelement`、`isempty(anyrange)→boolean`；多态返回由分析时已知 range 的 subtype 决定，非 wire 伪类型。本组构造 `(int4,int4[,text])→int4range`、`(numeric,numeric[,text])→numrange` 是两个/三个参数的独立签名；两参默认 [)。unknown/参数 OID 0 及隐式转换沿 resolver 和第 3 章 I 资格，不根据值/NULL 猜 subtype。其他 range 族不在代表验收域内。 |
| 表示与边界 | 必须分别保存 range/subtype 身份、整体 SQL NULL、empty、端点是否无界及开闭标志；端点值不是可存 SQL NULL 的成员槽。有限端点大小顺序必须合法。离散 int4range 规范化后再观察端点；连续 numrange 保留开闭方式，不能套整数步长。第 3 章没有该表示/codec 承诺，需先单独补齐窄类型契约才可实施函数。 |
| NULL/求值 | lower/upper/isempty strict，range SQL NULL→相应已定型结果的 SQL NULL。empty 的 lower/upper 为 NULL、isempty 为 true；无界仅对应端点提取为 NULL，isempty 为 false。构造器非 strict，任一端点 NULL 表示该侧无界，两端 NULL 是全域；三参 flags=NULL 则错误，不是返回 NULL。NULL 不绕过签名分析或保证其他实参不折叠。 |
| collation/时区/会话 | 本组 int4/numeric 不做文本排序或时刻换算，collation/TimeZone 对值运算不适用；固定 UTF-8，数值输入输出沿第 3 章。名称与类型选择仍依连接上下文；不推广到文本自定义 range 或 timestamptz 的会话显示。 |
| 确定性/折叠/副作用 | 所列函数/构造签名按固定目录默认属性为 immutable，无 SQL 数据副作用；合法常量可以折叠，错误可在分析/折叠/执行出现，不强制延迟到 Execute。本地缓存、折叠与隔离未验证。 |
| 返回/wire/修饰符 | 本组 lower/upper：int4 为 23/4/-1，numeric 为 1700/-1/-1；isempty 为 16/1/-1；构造为 int4range 3904/-1/-1 或 numrange 3906/-1/-1。依签名固定 OID/typlen/typmod，空/NULL/零行不改身份；AS v、表/属性号 0、NULL 长度 -1、Describe 格式沿首块，逻辑可空性另记。range 文本应输出规范边界或 empty；binary 为标志字节及存在的端点长度/子类型二进制载荷，不发送内部 varlena/OID 结构，也不透传文本字节冒充 binary。参数/结果 codec、身份连续性是未具备的依赖。 |
| 下推/错误/证据 | 默认无下推批准；构造下界大于上界、NULL flags 目标 22000，非法 flags 目标 42601，int4 离散规范化端点溢出目标 22003；名称/签名无匹配沿前块 42883。采 S/V/C/M、适用 D/H/P 和实际阶段，无伪成功行；精确本地映射、消息/位置及 MySQL 错误待采。工程缺口不能冒充参考 PG 的语义错误；所有场景未运行。 |

官方来源（2026-10-23 查阅，PostgreSQL **18**）：[§9.20 range 函数](https://www.postgresql.org/docs/18/functions-range.html)核对提取/empty，[§8.17.3–7 range 类型](https://www.postgresql.org/docs/18/rangetypes.html)核对构造、无界和离散规范化。固定 **REL_18_0** [pg_proc.dat](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_proc.dat)及[pg_proc.h](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_proc.h)核对精确签名、strict 和 immutable；[pg_type.dat](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_type.dat)核对 OID/长度；[rangetypes.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/rangetypes.c)的 range_constructor2/3、range_lower/upper、range_parse_flags、range_serialize、int4range_canonical、range_send 核对 NULL、错误、规范化与 binary 布局。数值例是规则推导目标；18.6 仍为来源指定版本，未采该补丁版本参考端证据。

#### 必需系统函数契约

沿首块统一模板，本块定义五个普通标量候选，规范实现候选 ID 为 `scalar.pg.system.database/v1`、`schema/v1`、`version/v1`、`backend-pid/v1`、`typeof/v1`（后四项沿同一 `scalar.pg.system.` 前缀）；不是已注册 ID。优先级依据上表客户端探测静态证据，先数据库/schema，再版本、连接身份与类型内省；后面三项的具体客户端版本、调用频率和需求紧迫度未采，不能仅因候选名存在便升级支持。

| 模板字段 | 本块目标、事实提供者及缺证边界 |
| --- | --- |
| 关联/分类/调用模型/名称/可见性 | 关联下方 FUNC-SYS 五组；五个 PG 签名和 pg_catalog 限定名沿第 2 章可见，current_schema 无括号形式由语法层承接。version 同名按方言分流；其余不向 MySQL 开 PG 别名，MySQL DATABASE()/VERSION()/CONNECTION_ID() 保留原契约。不批准跨方言共享别名。当前通用标量链未验证。 |
| 重载/隐式 cast | current_database()→name、current_schema()→name、version()→text、pg_backend_pid()→int4；pg_typeof("any")→regtype，"any" 是接受实参类型的伪类型而非 wire 类型，不把输入都转 text，也不按物化值猜类型。显式 typed NULL 和 Parse 参数类型优先，unknown/参数 OID 0 沿前块解析规则单列；本组不承诺未取证的任意域/用户类型解析。 |
| 值/NULL/事实责任 | 数据库身份由工作流 5/7 的所属连接提供；schema 由工作流 5 的有效显式路径与工作流 6 的对象存在/USAGE 事实共同决定，无有效项返回 SQL NULL。版本取真实构建/兼容性声明提供者，工作流 7 保持启动参数一致，不伪造参考 PG 构建串。PID 取工作流 7 的 backend 身份分配与取消注册表，不能强转 MySQL u64 或共享进程 PID。pg_typeof 取第 3 章分析后的参数类型身份，类型名/OID 映射归工作流 6；typed NULL 仍返回类型的非 NULL regtype，不走 strict NULL 传播。其余四项零参数，参数 strict 快捷不适用；正常连接数据库/版本/PID 非 NULL。 |
| 权限/会话/collation/时区 | 固定 UTF-8；值不做时区换算或文本比较，collation 对本组值运算不适用，name/text 自身类型与字符属性仍保留。schema 必须忽略不存在或无 USAGE 的路径项，不能把路径字符串当可见对象事实；隐式 catalog 不自动成为 current_schema。角色/权限变化的有效路径刷新由 5/6 提供，不新增权限查询函数。PG 数据库在连接内固定，改库须重连，不借 MySQL USE 实现 PG 身份切换。当前仅 public/catalog 的路径模型不证明完整权限过滤。 |
| 确定性/折叠/副作用/下推 | 固定 PG 目录五项均为 stable，无 SQL 数据写入；pg_typeof 非 strict，其余无实参。结果依连接、类型或服务器环境，不能按 immutable 跨会话缓存；准备语句不能冻结 current_schema 的后续 SET。内省不保证消除实参正常求值/错误；不得以 pg_typeof 重写吞掉表达式副作用。下推默认不批准，事实须在所属 SQL 会话读取，无性能结论。 |
| 返回类型/wire/修饰符 | current_database/schema 为 name（OID 19，标准 NAMEDATALEN=64 的 typlen 64）；version 为 text（25/-1）；PID 为 int4（23/4）；pg_typeof 结果列为 regtype（2206/4），其值可为 int4 的 OID 23，不能把结果列 OID 写成 23。本组 typmod 目标 -1；name/regtype 身份与 text/oid 不可混同，第 3 章未承诺部分先补窄类型/codec 依赖。AS v、表达式表/属性号 0、NULL 长度 -1、描述格式与零行规则沿首块；name 二进制不发内部定长填充，regtype 二进制为 OID 载荷。MySQL 精确 code/flags/charset/length/decimals 待参考采集，不能从 PG OID 推导。 |
| 错误/阶段/证据 | 合法调用无错误，错误字段不适用；如 current_database(1)、pg_typeof() 无匹配签名，PG 目标 42883，分析阶段拒绝且无成功行。无参数类型可确定时另采 resolver 错误，不猜 SQLSTATE；基础设施/工厂/codec 缺失记工程缺口，当前目录 0A000 不冒充 PG 语义。诊断沿首块记录 S/V/C/M、适用 D/H/P 和原 SQL 字符位置，精确消息/P/本地映射待采。全部运行未验证。 |

官方依据（2026-10-23 查阅，PostgreSQL **18**）：[§9.27 系统信息](https://www.postgresql.org/docs/18/functions-info.html)定义五项返回语义；[§19.11.1 search_path](https://www.postgresql.org/docs/18/runtime-config-client.html#GUC-SEARCH-PATH)定义有效路径与权限过滤。固定 **REL_18_0** 的 [pg_proc.dat](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_proc.dat)核对签名、stable 及 pg_typeof 非 strict，[misc.c::pg_typeof](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/misc.c)核对从表达式类型取得 OID，[pg_type.dat](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_type.dat)核对 name/int4/text/regtype 身份及长度；[§54.7 消息格式](https://www.postgresql.org/docs/18/protocol-message-formats.html)核对描述与 BackendKeyData。标准 name 长度及 binary 布局另核对同 tag 的 [pg_config_manual.h](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/pg_config_manual.h)、[name.c::namesend](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/name.c) 和 [regproc.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/regproc.c)；本地 codec 未验证。18.6 仅为来源指定版本，没有该补丁版本差分结果。

#### MySQL专属函数隔离

沿首块统一模板，四个标量契约关联下方 FUNC-MY 四组，规范实现候选 ID 分别为 `scalar.mysql.ifnull/v1`、`scalar.mysql.date-format/v1`、`scalar.mysql.json-extract/v1`、`scalar.tidb.build-info/v1`；仅为文档身份，不声称已注册。MySQL/TiDB 原语义以本仓库 Go 工厂与求值为保留基线；PG 相似能力不构成改写依据。

| 模板字段 | 四类目标、来源与缺证边界 |
| --- | --- |
| 名称/别名/可见性 | MySQL 入口保留 IFNULL、DATE_FORMAT、JSON_EXTRACT、TIDB_VERSION；PG 入口不公开这些内建名称，包括 pg_catalog 限定名。按第 2 章方言与名称环境先过滤候选，再选择工厂，NULL 或常量折叠不能绕过门禁。此为 AsterSQL 设计边界，不声称参考 PG 无法创建同名用户函数。无跨方言别名；开放例外只能逐签名满足“等价共享与别名门槛”（任务 4），不能直接挂 COALESCE、to_char、json_extract_path 或 version。 |
| 重载/隐式 cast | IFNULL 固定两参，继承 Go 控制类型聚合、按结果求值类型转换及宽度/flags；不套 PG COALESCE 类型规则。DATE_FORMAT 两参按 MySQL datetime/string 转换。JSON_EXTRACT 至少两参，首参 JSON、其余字符串路径，遵循 Go 类型资格检查与转换。TIDB_VERSION 零参，不适用参数 cast。unknown/typed NULL 的 PG 调用也先拒绝名称，不能调用 MySQL cast 选出签名。 |
| NULL/求值/错误 | IFNULL 首参非 NULL 不求值备用分支，首参 NULL 才取后值，全 NULL 为推导结果类型的 SQL NULL；已到达参数的错误正常传播。DATE_FORMAT 任一参数 NULL→SQL NULL，非法/零日期保持 Go sql_mode 下警告或错误，空模板是非 NULL 空串。JSON_EXTRACT 的 SQL NULL 文档或路径→SQL NULL、缺失→SQL NULL，存在 JSON null 是非 SQL NULL 的 JSON 值；非法 JSON/路径仍报自身错误。TIDB_VERSION 无参数 NULL 场景（零参），正常结果非 NULL，额外 NULL 参数是参数数目错误。 |
| collation/时区/会话 | IFNULL 保留 MySQL 字符集、collation 派生与数值/时间修饰符；DATE_FORMAT 保留 MySQL 日期转换、会话时区及实际 locale 规则，不能换用 PG 模板或 lc_*。JSON 路径按自身语法，不借 PG jsonpath 或文本 collation 改义。TIDB_VERSION 读本服务构建信息，不依参数 collation/时区，返回字符串仍携自身字符属性。固定并记录 sql_mode、charset/collation、time_zone 和构建基线；精确派生与非默认环境运行证据未采。 |
| 确定性/折叠/副作用 | 前三项仅在输入转换及所属会话上下文相同时才讨论复用/折叠，IFNULL 保留短路与错误时点；不从局部纯函数推断 SQL immutable。TIDB_VERSION 无数据写入，值属于服务构建上下文，不能用 PG version 值替换或跨不同构建缓存。精确 Go/Rust 折叠及缓存策略未完整取证，保持原策略，后续须补优化证据。 |
| 返回类型/值域/修饰符/wire | IFNULL 按两参聚合结果，本文 CHAR 用例为 MySQL 字符串；DATE_FORMAT 为 MySQL 字符串及 Go 模板长度推导；JSON_EXTRACT 为 MySQL JSON（TypeJSON/code 245），不是 PG json/jsonb OID；TIDB_VERSION 为 MySQL 字符串、Go flen 为 GetTiDBInfo 字节长度。其余 code/flags/charset/length/decimals 精确值待参考采集，不能从 Rust String 猜协议类型。NULL、零行与重绑值不能改列类型。PG 名称拒绝无成功结果类型/RowDescription，不分配伪造 text/JSON OID。 |
| 下推/错误字段/证据 | 不新增下推批准，不改变既有 MySQL 下推策略；已存 TiPB 签名不批准 PG 下推。MySQL 参数数目、转换、路径错误按自身 code/SQLSTATE/消息/警告和实际阶段采集，精确字段尚未采；PG 隐藏名称目标分析阶段 42883，无成功行，记录 S/V/C/M 与适用 D/H/P，精确位置/消息未采。缺工厂是工程缺口，不能当作门禁成功。成功错误字段不适用；当前全链有效运行用例为 0。 |

官方依据（2026-10-23 查阅，PostgreSQL **18**）：[§9.18 条件表达式](https://www.postgresql.org/docs/18/functions-conditional.html)仅把 IFNULL 作为其他数据库的相似能力，并说明 COALESCE 不是普通函数；[§9.16 JSON](https://www.postgresql.org/docs/18/functions-json.html)区分 SQL NULL/JSON null，列出的 json_extract_path 与 jsonpath 不授权 MySQL JSON_EXTRACT 别名。[§10.3 函数解析](https://www.postgresql.org/docs/18/typeconv-func.html)支持先筛可见候选，[附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html)定义 42883 为 undefined_function。这里的 MySQL 语义来自本地 Go，PG 隔离来自设计契约；18 大版本手册不能代替 18.6 实测。

#### 运算符调用契约

继承“三层身份与契约模板”，调用模型独立记录为运算符/比较谓词：规范算法身份、方言运算符签名和 wire 结果身份仍分三层。PG 候选身份至少含 namespace、符号、一元前缀或二元中缀及左右输入类型；一元缺左操作数是结构信息，不是传入 SQL NULL。结果类型来自已选签名，不参与反向选重载；内部 div/intdiv/nulleq/concat 名称不能自动成为 PG SQL 别名。`IS [NOT] DISTINCT FROM` 是特殊比较谓词，需按底层 `=` 的比较资格绑定并专门处理 NULL，不伪装成用户可查找的同名普通函数或运算符对象。

PG 运算符候选按第 2 章可见环境筛选：限定 `OPERATOR(pg_catalog./)` 只取指定 schema，未限定符号取可见同名/同元数候选；同输入签名按路径去重，不同输入签名平等考虑。先精确匹配；二元一侧 unknown 时在此步骤先假设其与已知侧同型，一元 unknown 无此捷径。其后按第 3 章 I 转换资格、精确位置数、类别优选、unknown 的 string 类偏好和已知同型约束逐步筛选，不套普通函数的 variadic/default 规则或 UNION 共同类型规则。无匹配目标 42883、歧义 42725；domain/完整多态等未开放载体保留能力门禁，不能借本块构建完整类型系统。Parse/Prepare 定型后，Bind 换值、NULL 或格式不得重选运算符。

| 首块模板字段 | 本块实例填写及证据缺口 |
| --- | --- |
| 关联、分类、模型、规范实现 ID | 关联下方 OP-CALL 四族。文档候选 `operator.integral.divide.checked/v1`、`operator.numeric.divide/v1`、`operator.text.concat.strict/v1`、`predicate.int.null-safe-equal/v1` 为待新增实现身份；二元比较的有限 int4 值域可作共享候选，不批准跨方言别名。MySQL `/` 与 PG 整数 `/` 为同符号异义；三个代表行为均未有两入口运行证据。 |
| 名称/可见性、元数、输入、cast | `/`、文本 `\|\|` 为二元，`-` 为一元前缀；签名和限定名见正文/验收表，均限定内建无同名用户/扩展遮蔽。算术只取 int4/int4、numeric/numeric 两签名；文本取 text/text，不将 anynonarray 混合拼接泛化为本块支持；NULL 安全比较限定 int4 对。无批准别名，unknown 与 typed NULL 按上述规则参加分析，不由空值绕过非法类型。 |
| NULL、求值条件 | PG 两个除法重载与 text/text 拼接 strict，合法 typed NULL 返回同型 NULL；NULL::int4 / 0::int4 不进入非空除零算法。NULL 安全谓词非 strict，结果永不为 NULL；普通 `=` 为对照。strict 不承诺子表达式求值顺序或避免规划期常量错误，不以 `(1/0)` 包在其他 NULL 表达式下作必然免错断言。 |
| collation、时区、会话 | 固定 UTF-8、内建可见、PG search_path 和 MySQL sql_mode 逐次记录；数字运算及 int4 比较不涉及 collation/时区。text/text 只拼接原字符，不按排序规则改变字符，结果 collation 派生消费第 3 章，字符集/collation 冲突须另取证，不能宣称任意 collation 已兼容。MySQL `\|\|` 分别固定 PIPES_AS_CONCAT 关/开，不能让该配置影响 PG。 |
| 确定性、折叠、副作用、下推 | 已定型内建 int4 除法与 textcat 按官方 REL_18_0 默认属性为 immutable/strict；其余实例仅限定固定输入身份的纯值目标，不批准新增优化属性。SELECT 无数据副作用；可折叠常量的值错误可发生于规划，参数错误亦须记录真实阶段。不新增下推许可，类型、NULL、错误及会话差异未证明等价前不下推；无性能结论。 |
| 返回类型、值域、wire | 下表列值/NULL/类型；int4 域与溢出不扩宽沿第 3 章，一元负最小值及最小值除 -1 目标 22003。PG 结果 OID/typlen：int4 23/4、numeric 1700/-1、text 25/-1、bool 16/1，表达式 typmod -1，列名 v、表/属性号 0；text/binary 按第 3/7 章，不由返回行推型。MySQL 沿自身 DECIMAL/BIGINT/字符串及布尔整数表达式元数据，精确 code/flags/charset/length/decimals 待参考采集；不输出 PG OID。 |
| 错误、验收、状态 | 除零 22012、域溢出 22003，无匹配/歧义见上；成功 SQLSTATE/错误字段不适用，拒绝无成功载荷。沿首块记录 S/V/C/M 与适用 D/H/P，精确消息/P 和分析/折叠/执行阶段未采集；MySQL 警告/错误按自身参考，不能把合法 PG 未实现伪装成 PG 无匹配。只完成静态取证和契约，双方言运行有效用例数为 0。 |

官方依据（2026-10-23 查阅，PostgreSQL **18**）：[§10.2 运算符解析](https://www.postgresql.org/docs/18/typeconv-oper.html)与[pg_operator](https://www.postgresql.org/docs/18/catalog-pg-operator.html)支持候选/元数及输入、结果、实现函数身份分离；[§9.3 数学运算](https://www.postgresql.org/docs/18/functions-math.html)定义整数向零截断及非整数除法，[§9.4 字符串运算](https://www.postgresql.org/docs/18/functions-string.html)定义 text 拼接，[§9.2 比较谓词](https://www.postgresql.org/docs/18/functions-comparison.html)定义 NULL 安全比较。固定 **REL_18_0** [pg_proc.dat](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_proc.dat) 的 int4div/textcat 与 [pg_proc.h](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_proc.h) 的 strict/immutable 默认值支持对应属性；[int.c::int4div](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/int.c)支持除零/最小值除 -1，[附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html)支持错误分类。18 大版本手册与 18.0 静态源码不等于 18.6 差分证据，本轮没有参考端运行结果。

#### 普通聚合调用契约

继承“三层身份与契约模板”，普通 aggregate 独立于 scalar/window/ordered-set/SRF：分析绑定可见聚合签名与固定结果身份，执行器为每个分组创建状态、逐行 transition、最后 finalize；server 只编码最终值。规范实现候选 `aggregate.count.nonnull/v1`、`aggregate.sum.typed/v1`、`aggregate.avg.sum-count/v1` 是文档身份，不是新增 API 或共享批准。输入、过渡状态和返回类型必须分别记录；下面的状态是所需逻辑信息，不要求复制 PG 内部布局。

| PG 代表输入签名 | 逻辑过渡状态与最终结果目标 |
| --- | --- |
| count(*)、count(单个已定型标量) | 非负计数，初值 0；前者计输入行，后者仅计非 NULL。返回 int8，即使无行仍为非 NULL 0。星号是调用形式，不是 NULL 参数；多表达式 COUNT 不借 MySQL 多参规则开放。 |
| sum(int2/int4)、sum(int8/numeric) | 保存是否见非 NULL 及精确和；前组返回 int8，后组返回 numeric。状态须足够保存目标结果及溢出信息，不按输入整数宽度截断；无有效输入返回同型 SQL NULL。 |
| avg(int2/int4/int8/numeric) | 保存非 NULL 计数及足够精度的和，最终一次相除返回 numeric；不能平均各局部平均数，也不能继承 MySQL div_precision_increment 作为 PG 精度规则。无有效输入返回 numeric NULL，不进行除零。 |
| sum(float4/float8)、avg(float4/float8) | 保存浮点累计所需信息和有效计数；SUM 分别返回 float4/float8，AVG 均返回 float8。不能因共享 f64 容器改写 SUM(float4) 身份；相加顺序、舍入及非有限值策略须另取运行证据，不承诺任意重排等价。 |

FILTER 是每个聚合自己的输入门禁，仅 true 的行进入该聚合的参数/去重/排序和状态更新，false/NULL 均排除；不等于整条查询 WHERE，不能删掉其他聚合或已有分组。DISTINCT 按已定型输入的相等/排序资格去重，不能把编码字节相等直接当 PG 等价；本组 count/sum/avg 忽略 NULL。聚合内 ORDER BY 负责 transition 输入次序，独立于最终结果行 ORDER BY；DISTINCT 与排序并用时，排序表达式只能引用 DISTINCT 参数列表。普通聚合可无排序，WITHIN GROUP 及 OVER 另块处理。空全局聚合仍产一行，空分组输入没有组/没有结果行；已形成的组即使 FILTER 全排除也须保留其 0/NULL 结果。

| 首块模板字段 | 本块填写与缺口 |
| --- | --- |
| 关联/分类/名称/别名/可见性 | 关联 AGG-CALL 五组与上述规范候选；PG count/sum/avg 及 pg_catalog 限定名按第 2 章可见，MySQL COUNT/SUM/AVG 保留自身聚合调用。相同名称不批准跨方言别名、状态或最终化共享。两入口正式 PG 签名接线未验证。 |
| 重载/隐式 cast | 只覆盖表列数值和 count 的单标量/星号；unknown、typed NULL 与参数 OID 0 沿前块 resolver 和第 3 章 I 资格，Bind 换值不选新签名。显式 text 不擅自转 numeric 以通过 sum；interval/money 等其他重载不在本代表验收域。 |
| NULL/求值条件 | 按上述行级 NULL/无行/组规则；最终 aggregate 不能套标量 strict(NULL) 直接返回 NULL，count 必须返回 0。FILTER 排除行不执行该行 transition；常量子表达式仍可能提前折叠报错，不承诺 SQL 从左到右求值或规避所有规划期错误。 |
| collation/时区/会话 | 本组数值和整数分组不涉及文本 collation 或时区算术，输入转换、名称解析仍消费连接环境；count 的通用文本 DISTINCT/分组须另证明 collation 等价，本轮不泛化。记录编码、MySQL sql_mode/div_precision_increment，PG 不继承后者。 |
| 确定性/折叠/副作用 | 固定输入多重集和合法次序下的纯聚合目标，无数据写入；依赖行集合，不能因参数为常量当成单次标量折叠。浮点重排可能改变结果，不新增 immutable/parallel-safe、跨连接缓存或消除排序许可；具体优化属性未取证。 |
| 返回/值域/修饰符/wire | 按上表固定身份：PG int8 20/8/-1、numeric 1700/-1/-1、float4 700/4/-1、float8 701/8/-1（OID/typlen/typmod，沿第 3 章）。输入 numeric(p,s) 不强行继承结果 typmod；NULL/无行不改描述，过渡状态不发给客户端。MySQL COUNT 为自身 BIGINT、精确数 SUM/AVG 为 DECIMAL、近似数为 DOUBLE；精确 flags/length/decimals/charset 待参考采集。 |
| 局部/最终/共享/下推 | 默认不新增许可；分别证明状态初值、跨组隔离、合并与空局部、精度/溢出/错误时点及最终类型。AVG 合并 sum/count 后一次最终化；跨分区重复值不能只相加局部 DISTINCT 结果，ORDER BY/FILTER 不能丢失。PG 手册支持 partial mode 不证明 AsterSQL 的 Split、typed 路径或 TiPB 下推已等价。 |
| 错误字段/阶段/证据 | 合法项成功无错误；无匹配 sum(text) 目标 42883。非法 DISTINCT/排序组合须拒绝，无成功行，精确 SQLSTATE/消息/位置待参考采集；计数及有界整数结果溢出不能 wrapping 或变 NULL，PG 数值越界沿第 3 章 22003。记录实际分析/折叠/transition/final 阶段及 S/V/C/M、适用 D/H/P；MySQL 保持原 code/SQLSTATE/警告，具体映射未采。有效运行用例 0。 |

官方依据（2026-10-23 查阅，PostgreSQL **18**）：[§9.21 聚合函数](https://www.postgresql.org/docs/18/functions-aggregate.html)定义三种代表的输入、返回、NULL 及 partial-mode 能力；[§4.2.7 聚合表达式](https://www.postgresql.org/docs/18/sql-expressions.html#SYNTAX-AGGREGATES)定义星号、DISTINCT、FILTER 与聚合内排序；[§36.12 状态/最终化与 partial aggregation](https://www.postgresql.org/docs/18/xaggr.html)区分输入/状态/结果类型及 combine 责任。协议、类型和错误字段沿本章模板及第 3 章。以上不是 18.6 的运行证据，精确补丁版本与参考端仍未核实。

### 范围外

#### 三层身份与契约模板

本块不实现生产函数、注册接口、Rust 类型、类型系统、协议 codec、下推或测试运行器，不运行 Cargo、Go 测试、数据库或差分环境；不盘点函数库。四类函数的完整实例、别名开放与重载排序、运算符及 aggregate/window/ordered-set aggregate/SRF 的独立调用模型留给本章后续文档块，不用标量注册器代替其他调用模型。系统事实归工作流 6，会话/事务与消息生命周期归工作流 5/7；不重写第 1–3 章、不补建扩展加载生态。

#### 注册工厂与方言可见性

本块不实现名称解析、方言注册 API、扩展管理或工厂补齐，不补齐 ABS 工厂，也不把共享字符串表改造成完整 pg_proc。PG/MySQL 函数族的全部参数语义、重载优先级、多态/variadic/default 参数、目录可见对象和协议状态机分别交后续块及工作流 6/7；不启动数据库、不运行 Cargo/Go 或新增测试平台。

#### 重载与隐式转换选择

本块不实现候选排序器、转换矩阵、参数推断、UDF、默认/多态函数或完整类型系统，不补工厂、不改共享 NULL 推导，也不生成生产代码/测试。只交接第 3 章既有资格的消费方式；运算符的专门候选规则、各函数族值语义及协议状态机归后续块和工作流 7。受控歧义夹具只要求未来解析器测试注入候选，不要求新增 SQL CREATE FUNCTION 生态。

#### 等价共享与别名门槛

只定义两种条件表达式的逐签名批准规则和五组用例，不实现工厂、NULLIF 重写、类型/collation 解析器、惰性 ABI、协议或下推，不补齐当前缺工厂，不盘点其他函数。域/数组/时间/用户自定义 `=` 的完整支持及 volatile 子表达式生态不在本块代表域内，不能因未覆盖而扩大本轮实现。不会运行 Cargo、数据库或差分平台。

#### 同名异义分流

本块不实现函数、不修补缺工厂、不补建 PG 特殊表达式/类型系统/协议或测试平台，不穷举可变数组、时态、JSON、locale/输出函数；不变更 MySQL 原 NULL、混型、参数和 packet 行为。这些域需分别补证，不从当前代表用例批准全函数共享。

#### 时间截断与提取

只定义三种时间源的截断/字段提取代表契约，不实现工厂、字段解码、时间身份、tzdata 服务、会话生命周期、AST、codec、下推或差分平台；不扩写 date_bin、格式化、日历运算及完整 date/time/timetz 函数库。第 2/3 章及注册/重载前块既有缺口分别交接，不因本块需要而补建；不运行 Cargo、数据库或构建/代码 lint。

#### 格式化函数契约

不实现 to_date/to_timestamp/to_number 解析器，不扩展完整模板函数库、locale 安装器或时区系统；不修复已读 MySQL 工厂/测试缺口、不建立新的求值器或测试平台。不批准 DATE_FORMAT/FORMAT 别名、PG 下推或全域性能结论；范围外缺口留给各自聚焦实现。

#### 数组函数契约

不实现数组表示、函数工厂、多态解析、cast/codec 或测试平台，不扩大为完整数组库。unnest 归集合返回模型，数组运算符归运算符任务；下标/切片、数组聚合、索引、自定义/复合/domain 元素不在本块。第 3 章拥有类型/形状与 codec，工作流 2/5/6/7/8 分别拥有语法、会话、目录、协议与客户端证据；不改 Go/MySQL JSON_ARRAY_APPEND 的原契约，不把缺口改写成 PG 语义拒绝。

#### 范围函数契约

本块不实现 range 表示、构造/求值、cast、codec 或测试设施，不修改第 3 章，不将基础类型缺口扩大为子系统建设。multirange、自定义 range、其他 range 族的完整验收、范围运算符/索引/聚合及扩展生态均在本块范围外；两入口已有 lower/upper 字符串契约另行保留。本轮不运行 Cargo/Go、数据库或差分平台。

#### 必需系统函数契约

不实现代码、工厂、OID 分配、任意目录行或权限系统；不补建完整系统函数库、角色生态、统计视图、版本伪装或取消协议。注册/求值只消费事实接口，目录对象/OID/权限事实归工作流 6，会话路径/角色归 5，连接启动/身份/取消归 7，类型身份归第 3 章。现有探测解析与 SQL 适配不是通用实现方案，禁止用 SQL 文本替换、固定 public/库名/进程号或参考 PG 版本串让探测通过；不把目录当前占位 NULL 当权限事实。

#### MySQL专属函数隔离

不盘点全部 MySQL/TiDB 管理函数，不修复 DATE_FORMAT/TIDB_VERSION 工厂或测试缺口，不改 Go 语义，不补建 JSON、时区、权限或管理子系统。不新增 COALESCE/to_char/PG JSON 别名，不把其他函数或扩展生态纳入隔离实现；本块不修改代码、协议或下推。

#### 运算符调用契约

不实现 PG operator resolver、表达式工厂、cast、codec、目录或测试设施，不改 Go/MySQL 算法及 sql_mode。只取整数/非整数除法、文本拼接和 int4 NULL 安全比较；数组/range 运算符登记为后续独立族，不能借 `||` 同符号或 lower/upper 已注册推定支持。其他数值/文本混合重载、完整算术库、行/复合比较、用户自定义运算符及扩展生态不在本块。语法优先级和谓词节点归第 2 章，算术值域/规范类型归第 3 章，系统目录/协议生命周期归工作流 6/7；不运行 Cargo、数据库或差分平台。

#### 普通聚合调用契约

不实现 FILTER 载体、排序器、PG numeric 或新的聚合执行器，不修复两条现有聚合路径的精度/溢出差异，不扩展完整聚合库、分布式状态序列化或测试平台；窗口 frame/逆转移、WITHIN GROUP 的 direct arguments、有序集合及 SRF 留给独立调用模型。现有 MySQL/Go 行为继续保留，不用 PG 规则重写；共享、局部/最终和下推缺证分别交后续聚焦工作，不阻塞本块文档交付。

### 验收

#### 三层身份与契约模板

文档验收要求三层身份可从一个标量候选追踪、模板全部字段有来源或明确缺口、五个全局小节齐全。下表为**待新增**用例族和候选能力记录，沿第 1 章格式落地，本轮没有新增/执行测试。每行 PG、MySQL 分别核对 AsterSQL 入口与自身参考端；成功均一列 v、无副作用、SQLSTATE/错误位置不适用，NULL 项按逻辑可空性记录；拒绝项无成功值/NULL/结果载荷，不能作为函数成功用例。

| 场景 ID 与区分目的 | PG 输入与目标 | MySQL 自身输入/可见性与目标 | 来源、缺口与证据等级 |
| --- | --- | --- | --- |
| FUNC-ID-ABS-SIGNATURE：同名不同签名 | 分别 `SELECT abs(CAST(-1 AS int4)) AS v`、`SELECT abs(CAST(-1 AS int8)) AS v`，均一行非 NULL 的 1；可见内建 abs，分别 23/4/-1、20/8/-1，不合并为同一签名。另查 pg_catalog.abs 的对应限定调用，目标同上。 | `SELECT ABS(CAST(-1 AS SIGNED)) AS v`，函数可见、返回非 NULL 的 1/BIGINT；PG int4/int8 语法不直接发送 MySQL。其自身 flags/length/decimals 按模板补证。 | PG 官方签名/类型目录；MySQL Go 工厂及 TestAbs。仅目标与静态来源，双方言名称/执行/元数据均未验证。 |
| FUNC-ID-ABS-SAME-VALUE：同值不同返回类型 | 沿用上行两条 SQL，数值均 1 仍须分别保持 int4/int8；不得从值或 payload 文本“1”合并 OID。 | 独立比较上行 SIGNED 与 `SELECT ABS(CAST(-1 AS DECIMAL(8,0))) AS v`，均精确值 1，返回 BIGINT 与 DECIMAL 身份不同；返回精度/scale/wire 字段另采证，不要求与 PG OID 相等。 | PG pg_proc.dat、MySQL Go Int/Decimal 分支；Rust 数学测试只证局部意图，实际两入口未验证。 |
| FUNC-ID-ABS-TYPED-NULL：NULL 不是未知身份 | `SELECT abs(CAST(NULL AS int4)) AS v` 与 int8 变体，均一行 SQL NULL，分别 23/4/-1、20/8/-1；扩展路径 `SELECT abs($1::int4) AS v`，Parse 参数 OID 23，绑定 -1/NULL/1 的三个新 portal，值为 1/NULL/1，结果描述始终 23/4/-1。 | `SELECT ABS(CAST(NULL AS SIGNED)) AS v`，函数可见、SQL NULL/BIGINT；MySQL Prepare `SELECT ABS(CAST(? AS SIGNED)) AS v` 独立重绑同组值，保持自身列定义。未定型 NULL、字符串 NULL 与空载荷不替代 typed NULL。 | strict 默认属性、Go nil 断言及第 3 章 META 契约；通用类型回退 text 不符合目标。两入口 NULL/准备/协议连续性均未验证。 |
| FUNC-ID-ABS-ERROR：签名与域检查分开 | `SELECT abs('-2147483648'::int4) AS v` 目标 22003，无临时 int8 提升；`SELECT abs() AS v` 目标 42883，无匹配签名，不能因 abs 名称存在成功。错误字段沿模板，确切阶段/P 待取证。 | `SELECT ABS(CAST(-2147483648 AS SIGNED)) AS v` 在 BIGINT 域内目标 2147483648，不能套 PG int4 溢出；`SELECT ABS() AS v` 拒绝错误参数数目，MySQL 精确码/位置待参考采集，无成功结果。 | PG int4abs/签名及错误分类；MySQL Go Int 工厂和 i64 局部算法支持域内目标，精确诊断仍缺证。未做参考端实测，不将共享内核错误字符串充当 PG SQLSTATE。 |

以上各成功场景 PG 同时定义简单 Q 和扩展 Parse/statement Describe/Bind/portal Describe/Execute 两路径；结果格式覆盖 text/binary，statement 格式 0、portal 按 Bind，OID/typlen/typmod 与 NULL 不随格式、重绑值或首行变化，Execute 只比对已描述类型与载荷，不要求它再发 RowDescription。另给首行 NULL 后接 1 的夹具 `SELECT abs(v) AS v FROM (VALUES (1,NULL::int4),(2,1::int4)) AS t(ord,v) ORDER BY ord`，目标两行 NULL/1；全 NULL 使用上表单行 typed NULL，零行使用 `SELECT abs(CAST(NULL AS int4)) AS v WHERE false`。三者描述仍 23/4/-1，行数分别为二/一/零。MySQL 使用自身 Query/Prepare、结果列定义和参数格式做对应检查，PG SQL/格式不直接套用。函数别名、两个 PG 会话及同时存在的 MySQL 连接需交错观测可见性与类型隔离；没有默认批准任何跨方言别名。此处均是未采集的验收目标，第 3 章类型/codec 与工作流 7 生命周期负责落实。

人工交接检查须将值/NULL、类型、元数据、错误、可见性和证据等级逐格对应；存在待定义字段时，文档可交接但相应行为验收不可记通过。结构、路径与 diff 检查只能证明文档完整性，不证明 SQL 正确、兼容或性能。

#### 注册工厂与方言可见性

以下四族为待新增验收目标。双方言使用独立连接，参考端保持相同无用户/扩展同名函数的前提；每行分别比较 AsterSQL 与所属方言参考端。错误项无成功值、NULL、结果类型或元数据，不能以一行 NULL 充当拒绝。

| 场景 ID | PG 入口输入、可见性与预期 | MySQL 入口输入、可见性与预期 | 来源与当前证据等级 |
| --- | --- | --- | --- |
| FUNC-REG-PG-ONLY | `SELECT pg_catalog.date_trunc('hour'::text, TIMESTAMP '2001-02-16 20:38:40') AS v`；可见，非 NULL 的 2001-02-16 20:00:00 / timestamp without time zone，1114/8/-1。非限定 date_trunc 同样可见。成功 SQLSTATE 不适用。 | `SELECT date_trunc('hour', CAST('2001-02-16 20:38:40' AS DATETIME)) AS v`；本目标集合不可见，在名称/签名解析拒绝，MySQL 精确错误码/SQLSTATE 待参考采集。用自身合法参数语法，避免 PG cast 语法错误掩盖函数门禁。 | PG 官方 §9.9/类型目录给语义目标；MySQL 隐藏为方言设计边界，参考端仍待核实；当前没有 PG 工厂或两入口通过证据。 |
| FUNC-REG-MYSQL-ONLY | `SELECT ifnull(NULL::text, 'fallback'::text) AS v`；PG 目标不可见，42883；不得自动改写成 COALESCE 或泄露 MySQL 工厂。 | `SELECT IFNULL(CAST(NULL AS CHAR), CAST('fallback' AS CHAR)) AS v`；可见，非 NULL fallback/字符串；另以第二参数 `CAST(NULL AS CHAR)` 得 SQL NULL/字符串，类型描述不得由首行或空值改变；wire 精确字段待采集，成功 SQLSTATE 不适用。 | IFNULL 核心工厂条目及独立 `migrated_string_control_and_like_factories_match_go_runtime` 给局部构造/值断言意图；typed CHAR、全 NULL、PG 拒绝与两入口元数据未实测。 |
| FUNC-REG-UNKNOWN | `SELECT astersql_contract_missing_fn_20261023(1::int4) AS v`；确认名称不在内建/用户/扩展候选，目标 42883，名称解析拒绝。 | `SELECT astersql_contract_missing_fn_20261023(CAST(1 AS SIGNED)) AS v`；同样不可见并拒绝；固定已选库，错误码/SQLSTATE/消息待参考采集。 | `newFunctionImpl` 的缺名分支及注册表测试负例是静态证据，不证明 PG SQLSTATE 已映射；两入口拒绝未实测。 |
| FUNC-REG-UNLINKED | 受控夹具保留可见 `abs(int4)` 类并确保工厂缺失，`SELECT abs(1::int4) AS v`；名称/参数已选中仍拒绝，目标 0A000（AsterSQL 实现缺口设计），不得误报 42883 或返回 NULL。正常 PG 参考端应为非 NULL 1/int4，23/4/-1；拒绝与成功不能算差分等价。 | 独立夹具保留 ABS 类/缺工厂，`SELECT ABS(CAST(1 AS SIGNED)) AS v`；构造拒绝，MySQL 实现缺口码/SQLSTATE 待定义；正常参考端目标非 NULL 1/BIGINT，元数据沿首块补证。 | `invoke_factory` 缺工厂错误及 abs 规格/核心表差异是夹具依据；动态注册须在后续测试中隔离并恢复，不据当前表断言生产 abs 必失败。此为工程拒绝用例，不得登记为功能支持或普通 PG 等价通过。 |

执行交接：PG 成功项覆盖 Q 与 Parse/Describe/Bind/Execute 的 text/binary 格式，返回描述依第 3 章固定；错误项分别采集名称/类型分析或构造失败阶段与协议 ErrorResponse，不臆定现有阶段。MySQL 覆盖 Query/Prepare 及其自身列定义。交错两个 PG 会话和一个 MySQL 连接，重用名称并重复准备/执行，验证可见性及缓存不串方言；进程全局注册变动不得用作切换连接方言。缺工厂夹具须隔离全局表，不能污染其他会话或测试。文档逐字段审阅通过只表示目标及缺口可交接，缺失诊断、typed NULL 或 wire 证据的行为仍未验收。

#### 重载与隐式转换选择

下表为**待新增** FUNC-RES 用例族，全部运行未验证。PG 同时采集 simple Q 与 Parse/statement Describe/Bind/portal Describe/Execute，双方言各对自身参考端；成功核对一行值/NULL、类型与可见性，失败核对 SQLSTATE 及首块错误字段。MySQL 旁路是隔离断言，不构造 PG 参数语法或虚构 MySQL 重载选择；其精确元数据缺口必须补证后才能升级证据。

| 五组目标用例 | PG 输入、选择、值/NULL/类型/错误 | MySQL 入口与证据边界 |
| --- | --- | --- |
| FUNC-RES-EXACT：精确签名优先 | `SELECT pg_catalog.abs((-1)::int4) AS v` 与 int8 版本分别精确绑定对应输入，均非 NULL 的 1，结果 23/4/-1 与 20/8/-1。增加 `SELECT pg_catalog.round(4::int4,4::int4) AS v`：I 资格转 numeric，结果 4.0000、1700/-1/-1，证明无精确签名时合法隐式转换；成功无错误。 | `SELECT ABS(CAST(-1 AS SIGNED)) AS v` 目标非 NULL 的 1/BIGINT，沿首块元数据取证；MySQL 不接受 PG 的 int4/int8 签名作为自身断言。PG round 例直接来自 §10.3，双方言全链未验证。 |
| FUNC-RES-UNKNOWN：unknown 定型 | `SELECT pg_catalog.substr('1234',3::int4) AS v` 将 unknown 按 string 类选择 text 输入，结果非 NULL 的 '34'，25/-1/-1。扩展正例 `SELECT pg_catalog.abs($1::int4) AS v` 用省略/0 OID 推定参数 23；绑定 -1、2、NULL 分别得 1、2、NULL，结果始终 23/4/-1，不能由 Bind 猜测重载。 | `SELECT SUBSTR('1234',3) AS v` 目标非 NULL 的 '34'/字符串；其返回类型码和字符元数据待自身参考采集，不把 PG unknown 规则搬入 MySQL。当前 PG 普通省略 OID 测试断言 0A000，目标推断未实现的部分须单列缺口。 |
| FUNC-RES-NULL：NULL 参数 | `SELECT pg_catalog.abs(NULL::int4) AS v`、int8 版本各得一行 SQL NULL、分别 23/4/-1 与 20/8/-1；NULL 不改签名。未定型 NULL 的歧义见 AMBIG，不能一律先定 text；`abs(NULL::text)` 目标 42883，没有 NULL 成功结果。 | `SELECT ABS(CAST(NULL AS SIGNED)) AS v` 目标 SQL NULL/BIGINT，沿 MySQL NULL 契约；不能把两入口的物理空值表示当作类型相同证据。PG typed NULL 全链未验证。 |
| FUNC-RES-NO-MATCH：无匹配 | `SELECT pg_catalog.abs('1'::text) AS v` 目标 42883，text 已定型且无到数值的 I 资格，不尝试读值后选择；`SELECT pg_catalog.abs() AS v` 同码但因为参数数目无匹配。两者无成功结果；与已选签名后的非法输入错误分开记录。 | `SELECT ABS() AS v` 拒绝参数数目，MySQL 精确码/消息待采集；合法旁路 `ABS(CAST(-1 AS SIGNED))` 仍为 1/BIGINT。不能把 PG text 拒绝目标推广为 MySQL 文本数值转换禁令。 |
| FUNC-RES-AMBIG：歧义 | 待新增解析夹具只暴露同 schema 的 `f(int4)→int4`、`f(int8)→int8`，无默认/可变/其他候选、两型都非 numeric 类优选。对 `f('1')`、`f(NULL)` 无精确或唯一优选目标 42725；显式 `f(1::int4)` 唯一绑定 int4，分析结果 23/4/-1（夹具不执行，无值/NULL 断言）。前两项错误而非任取首候选，按 §10.3 推导、待参考解析取证。 | 夹具仅用于 PG 解析，无 MySQL 可见函数 f，不能在 MySQL 注入同名生产函数。旁路 `SELECT ABS(CAST(-1 AS SIGNED)) AS v` 仍为 1/BIGINT；验证 PG 夹具和连接不改变 MySQL 可见集合。该旁路及 PG 歧义均未运行。 |

成功项的 Prepare/Describe 与 Execute 类型必须一致：显式参数 OID 与上下文兼容时保留其源身份，未知参数由分析固定；statement Describe 返回 ParameterDescription 与结果类型，portal Describe 只改变格式字段，Execute 按固定身份编码且不额外发送 RowDescription。新 Bind 换值/NULL、text/binary 不改变签名或结果描述，失败项不能发送伪成功描述/数据；binary 缺 codec 须如实登记能力缺口。MySQL Prepare/Execute 按其参数约束与元数据独立核对，不能套 PG OID。夹具 AMBIG 只验解析选择/拒绝，不宣称执行成功；可变参数与默认参数的未来聚焦解析验收还须覆盖展开/显式数组、同签名优先及默认歧义，不能以五组内建例证明这些能力已实现。

#### 等价共享与别名门槛

以下 FUNC-EQUIV 五组均为**待新增、未运行**的对照；每条分别比较 PG/AsterSQL PG 与 MySQL/AsterSQL MySQL，不要求两种参考端类型相等。两入口的 COALESCE/NULLIF 均要求在自身语法中可见，PG IFNULL 仍按前块拒绝。成功均单列 v、无错误，NULL 与类型独立检查；错误项无成功行或 NULL 载荷。

| 场景族 | PG 输入及目标值/NULL/类型/错误 | MySQL 自身输入及目标/缺口 |
| --- | --- | --- |
| FUNC-EQUIV-VALUE：普通值 | `SELECT COALESCE(NULL::int4,7::int4,9::int4) AS v`→7；`SELECT NULLIF(7::int4,7::int4) AS v`→NULL；第二参数换 9→7，均 int4。另 `NULLIF(7::int4,NULL::int4)`→7，`NULLIF(NULL::int4,7::int4)`→NULL，均 int4。 | 对应使用 `CAST(... AS SIGNED)`（NULL 同样显式 cast），目标值依次 7/NULL/7/7/NULL，目标 BIGINT；精确 flags、可空性/元数据由参考端补证。不能直接发送 PG ::int4。 |
| FUNC-EQUIV-ALL-NULL：全 NULL | `SELECT COALESCE(NULL::int4,NULL::int4) AS v` 与 `SELECT NULLIF(NULL::int4,NULL::int4) AS v`→SQL NULL/int4；另 `SELECT COALESCE(NULL,NULL) AS v`→SQL NULL/text，不能按本地 Null/binary 默认值回退。 | 两个 typed SIGNED 版本→SQL NULL/BIGINT 目标；未定型 `SELECT COALESCE(NULL,NULL) AS v` 保留其自身 NULL 类型/列定义，确切 MySQL wire 待采，不能强定 PG text。 |
| FUNC-EQUIV-MIXED：混合类型 | `SELECT COALESCE(NULL::int4,2.2::numeric) AS v`→2.2/numeric；`SELECT NULLIF(1::int4,2.2::numeric) AS v`→1/numeric，体现比较后首参数提升。`SELECT COALESCE(1::int4,'x'::text) AS v` 即使首项非 NULL 也应在分析拒绝，目标 42804，无值。 | `SELECT COALESCE(CAST(NULL AS SIGNED),CAST(2.2 AS DECIMAL(2,1))) AS v`→2.2/DECIMAL 目标；`SELECT NULLIF(CAST(1 AS SIGNED),CAST(2.2 AS DECIMAL(2,1))) AS v`→1，第一参数整数返回身份作为待参考核实目标；精度/scale/wire 缺证。`COALESCE(CAST(1 AS SIGNED),CAST('x' AS CHAR))` 保留 MySQL 自身混型规则，预计取首值但类型待采，不套 PG 42804；本组不批准共享。 |
| FUNC-EQUIV-UNREACHED-ERROR：未选分支与必需比较 | 待新增普通关系夹具 `contract_equiv_input(a int4,z int4)` 含一行 (7,0)，无将 z 固化为常量的约束：`SELECT COALESCE(a,10/z) AS v FROM contract_equiv_input`→7/int4，无除零错误；改 a 为 NULL 后应为 22012。`SELECT NULLIF(a,10/z) AS v FROM contract_equiv_input` 在 a=7 时也须计算比较所需右参，目标 22012。分析错误仍不能被短路；常量 `1/0` 不作为运行期跳过的唯一证据，实际失败阶段须采集。 | 自身普通关系夹具采用 BIGINT 列，COALESCE(a,10/z) 在 (7,0) 应跳过后参并取 7，结果类型由自身 `/` 与聚合规则决定，非自动 BIGINT。a=NULL 及 NULLIF 右参的除零值/警告/错误须固定 sql_mode 按参考端采集，不承诺与 PG 22012 同义。另沿已读局部 Datum::Error 夹具覆盖未达/必达错误，但局部通过不能代替 SQL 证据。 |
| FUNC-EQUIV-COLLATION：字符串比较与结果属性 | `SELECT COALESCE(NULL::text,'A'::text COLLATE "C") AS v`→非 NULL 'A'/text，输出 C；`SELECT NULLIF('A'::text COLLATE "C",'a'::text COLLATE "C") AS v`→非 NULL 'A'/text；把第二值换 'A'→NULL/text。负例 `SELECT COALESCE('A'::text COLLATE "C",'a'::text COLLATE "POSIX") AS v` 显式冲突目标 42P21，不能因为首项非 NULL 忽略分析错误。 | `SELECT COALESCE(CAST(NULL AS CHAR CHARACTER SET utf8mb4),_utf8mb4'A' COLLATE utf8mb4_bin) AS v`→'A'/字符串、utf8mb4_bin；`SELECT NULLIF(_utf8mb4'A' COLLATE utf8mb4_bin,_utf8mb4'a' COLLATE utf8mb4_bin) AS v`→'A'，第二值换 'A'→NULL。不把 PG C/POSIX 名称发给 MySQL；本地比较上下文快照断言不能证明上述 SQL/输出 collation，精确 wire 和冲突诊断仍待采。 |

每个成功项覆盖 PG Q 与 Parse/statement Describe/Bind/portal Describe/Execute、text/binary；以带显式 cast 的参数重绑值/NULL，类型与 collation 必须由分析固定，不能随选中分支变化。全 NULL 仍有固定描述；结果逻辑可空性单独登记。MySQL 覆盖 Query/Prepare 及自身列定义，采集当前 sql_mode/字符设置和警告；两个 PG 连接与 MySQL 连接交错验证名称、类型和缓存隔离。所有行均只有官方规则或局部静态断言意图，没有两入口运行证据，因此本轮**批准共享/别名数量为 0**。结构检查不能升级任一场景为功能通过。

#### 同名异义分流

下表 FUNC-SPLIT 四族均为**待新增、未运行**目标；每条独立比较参考 PG/AsterSQL PG 和参考 MySQL/AsterSQL MySQL。两入口名称均目标可见，参数/类型负例仍须在正确阶段拒绝；成功均单列 v、无错误，非 NULL 空串与 SQL NULL 分开断言，wire 使用本块模板。MySQL 的“字符串”指自身结果类型，精确元数据待补，不能当作 PG text。

| 场景族 | PG 输入与值/NULL/类型/错误目标 | MySQL 自身输入与目标、证据边界 |
| --- | --- | --- |
| FUNC-SPLIT-NULL | `SELECT concat('a'::text,NULL::text,'b'::text) AS v`→非 NULL 'ab'/text；`concat(NULL::text,NULL::text)`→非 NULL 空串/text。`SELECT GREATEST(NULL::int4,7::int4) AS v` 与 LEAST 版本→7/int4；全换 NULL::int4→NULL/int4；另 `GREATEST(NULL,NULL)`→NULL/text。 | `SELECT CONCAT(CAST('a' AS CHAR),CAST(NULL AS CHAR),CAST('b' AS CHAR)) AS v`→NULL/字符串；全 NULL CHAR 同样 NULL。`SELECT GREATEST(CAST(NULL AS SIGNED),CAST(7 AS SIGNED)) AS v` 及 LEAST→NULL/BIGINT 目标，全 NULL SIGNED 同样 NULL。Rust 局部 NULL 断言只支持 MySQL 算法意图，不证明 SQL/typed NULL/wire。 |
| FUNC-SPLIT-MIXED | `SELECT concat('v='::text,2::int4) AS v`→非 NULL 'v=2'/text；`SELECT GREATEST(2::int4,10.5::numeric) AS v`→10.5/numeric，LEAST→2/numeric。`SELECT GREATEST(2::int4,'10'::text) AS v` 及 LEAST→分析拒绝 42804，无值；typed text 不能被当作 unknown 数字串。 | `SELECT CONCAT(CAST('v=' AS CHAR),CAST(2 AS SIGNED)) AS v`→'v=2'/字符串；`SELECT GREATEST(CAST(2 AS SIGNED),CAST(10.5 AS DECIMAL(3,1))) AS v`→10.5/DECIMAL，LEAST→2/DECIMAL 目标。`SELECT GREATEST(CAST(2 AS SIGNED),CAST('10' AS CHAR)) AS v` 及 LEAST 的目标按自身字符串比较为 '2'/'10'，返回字符串；混型精确类型/precision/scale/wire 待采，Go 工厂/混型测试只有静态意图，不套 PG 拒绝。 |
| FUNC-SPLIT-COLLATION | `SELECT GREATEST('Z'::text COLLATE "C",'a'::text COLLATE "C") AS v`→'a'/text，LEAST→'Z'/text，结果 collation C。`SELECT concat('Z'::text COLLATE "C",'a'::text COLLATE "C") AS v`→'Za'/text、C，证明拼接不排序。极值第二参数显式换 COLLATE "POSIX"→冲突拒绝目标 42P21，无值。 | `SELECT GREATEST(_utf8mb4'Z' COLLATE utf8mb4_bin,_utf8mb4'a' COLLATE utf8mb4_bin) AS v`→'a'/字符串，LEAST→'Z'；对应 CONCAT→'Za'，三者结果 collation 目标 utf8mb4_bin。固定 ASCII/相同显式规则，此域值可相同但不能由此批准全域等价；不把 C/POSIX 名称发到 MySQL。冲突诊断/精确 wire 由自身参考端补证。 |
| FUNC-SPLIT-ARITY | `SELECT concat('x'::text) AS v`→'x'/text；`SELECT concat() AS v`→无匹配函数目标 42883。`SELECT GREATEST(7::int4) AS v` 与 LEAST→7/int4；`SELECT GREATEST() AS v` 与 LEAST→语法拒绝目标 42601，无值，不能套用普通函数 42883。 | `SELECT CONCAT(CAST('x' AS CHAR)) AS v`→'x'/字符串，`SELECT CONCAT() AS v` 拒绝参数个数；`SELECT GREATEST(CAST(7 AS SIGNED)) AS v` 和 LEAST 均拒绝，不能因局部 helper 接受单值而开放；两 SIGNED 参数 (7,9) 正例→9/7、BIGINT。零参数极值同样拒绝。MySQL 错误码/SQLSTATE/消息待参考采集，不复用 PG 42601/42883。 |

成功项覆盖 PG simple Q 与 Parse/statement Describe/Bind/portal Describe/Execute 的 text/binary，并使用显式 cast 的参数重绑普通值/NULL，参数与结果身份在分析固定；零行或全 NULL 仍有类型描述。MySQL 覆盖 Query/Prepare/Execute 及自身列定义、警告/会话设置。交错两个 PG 连接和一个 MySQL 连接，以相同名称、不同 NULL/参数策略验证缓存与可见性隔离；不能修改进程全局注册模拟连接方言。拒绝项采实际阶段和完整错误字段，无伪成功行；缺工厂沿注册块单列工程缺口，不能冒充参考端语义错误。四组仅有官方目标/静态证据，未批准任何共享别名、下推或功能支持。

#### 时间截断与提取

以下 FUNC-TEMP 五组全部为**待新增、未运行**用例。PG 名称/签名目标可见，合法项成功无错误、单列 AS v；除明确 NULL 项外均非 NULL，结果类型/wire 使用本块模板。每条独立比较参考 PG 与 AsterSQL PG，MySQL 旁路对其自身参考端，不要求跨方言同型。固定构建、编码、p、时区与 tzdata，不能把这些文档预期记成差分结果。

| 五组场景 | PG 输入与目标值/NULL/类型/错误 | MySQL 入口与证据等级 |
| --- | --- | --- |
| FUNC-TEMP-DAY：跨日与精度 | `SELECT date_trunc('day',TIMESTAMP '2024-01-02 00:00:00.123456') AS v`→2024-01-02 00:00:00/timestamp；换前日 23:59:59.999999→2024-01-01 00:00:00；`date_trunc('week',TIMESTAMP '2021-01-01 12:00:00')`→2020-12-28 00:00:00。`date_trunc('milliseconds',TIMESTAMP '2024-01-02 00:00:40.123456')`→00:00:40.123（日期不变）。对该原输入 `EXTRACT(SECOND FROM ...)`→40.123456/numeric，`date_part('second',...)`→40.123456/float8（按 float8 正确舍入及编码核对）；`EXTRACT(MICROSECONDS FROM ...)`→40123456/numeric。 | `SELECT EXTRACT(SECOND FROM CAST('2024-01-02 00:00:40.123456' AS DATETIME(6))) AS v` 保留自身整数 40 目标；MICROSECOND 版本目标 123456，不能套 PG 整秒累计单位。MySQL 精确列定义及两入口未采；静态正式工厂为整数分支，不能证明上述 SQL 通过。 |
| FUNC-TEMP-ZONE：时区变化 | `SELECT date_trunc('day',TIMESTAMPTZ '2024-01-02 01:30:00+00') AS v` 在 UTC→2024-01-02 00:00:00+00，在 America/New_York→2024-01-01 00:00:00-05（05:00Z），均 timestamptz。固定 UTC 显示，三参 zone='America/New_York'→2024-01-01 05:00:00+00。同一输入 `EXTRACT(HOUR FROM ...)` 在两会话分别 1/20、numeric，date_part('hour',...) 同值/float8；timestamp '2024-01-02 01:30:00' 的 day 截断在两区域均 2024-01-02 00:00:00/timestamp，不能添造瞬时。 | PG 两名称在 MySQL 隐藏；以 `SELECT date_trunc('day',CAST('2024-01-02 01:30:00' AS DATETIME)) AS v`、同参数 date_part 验证名称拒绝（错误码/消息待采）。合法旁路 `EXTRACT(HOUR FROM CAST('2024-01-02 01:30:00' AS DATETIME))`→1/自身整数目标，不复制 PG timestamptz。区域转换独立断言仅静态意图。 |
| FUNC-TEMP-DST：DST 边界 | TimeZone=UTC，`SELECT date_trunc('day',TIMESTAMPTZ '2024-03-10 12:00:00+00','America/New_York') AS v`→2024-03-10 05:00:00+00，次日同时间输入→2024-03-11 04:00:00+00，相邻桶相差 23h；秋季 2024-11-03 / 11-04 12:00+00 分别→11-03 04:00Z / 11-04 05:00Z，相差 25h，均 timestamptz。TimeZone=America/New_York，显式瞬时 2024-11-03 05:30Z / 06:30Z 的 HOUR 提取均 1/numeric，timezone 分别 -14400/-18000；date_part 同字段同数值/float8。避免模糊墙钟输入掩盖偏移差异。 | 继续拒绝 PG 专属名称；MySQL 自身 `EXTRACT(HOUR FROM CAST('2024-11-03 01:30:00' AS DATETIME))`→1/整数目标，不声称其保存两个瞬时。已有 Amsterdam 缺口/重叠转换断言不等于 PG 截断。DST 数值为规则推导，参考端、tzdata 与双方言运行证据均待采。 |
| FUNC-TEMP-INTERVAL：interval 字段 | `SELECT date_trunc('hour',INTERVAL '2 years 3 months 4 days 05:06:07.123456') AS v`→月分量 27、日 4、时间 05:00:00/interval。对原输入依次 `EXTRACT(YEAR/MONTH/DAY/HOUR/SECOND FROM ...)`（每字段单独一条 SQL）→2/3/4/5/7.123456、numeric；date_part 对应文本字段→同数值/float8。`EXTRACT(MINUTE FROM INTERVAL '80 minutes')`→20/numeric；`date_trunc('hour',INTERVAL '-1 day -02:03:04')`→日 -1、时间 -02:00:00/interval，不能按总秒数向负无穷取整或把月折 30 天。 | 不发送 PG interval 语法；`SELECT EXTRACT(HOUR FROM CAST('27:06:07' AS TIME)) AS v`→27/自身整数目标，保留 MySQL 长 duration，不能充当 interval 日/月字段证据。正式工厂 Duration 分支只有局部意图，SQL/元数据待采。 |
| FUNC-TEMP-NULL-INVALID：NULL/非法单位 | 对每个 T=timestamp/timestamptz/interval，`SELECT date_trunc('day',NULL::T) AS v`→NULL/T；`date_part('day',NULL::T)`→NULL/float8；`EXTRACT(DAY FROM NULL::T)`→NULL/numeric。`date_trunc(NULL::text,TIMESTAMP '2024-01-01')`→NULL/timestamp，`date_trunc('day',TIMESTAMPTZ '2024-01-01 00:00:00+00',NULL::text)`→NULL/timestamptz。非 NULL 有效源上 date_trunc/date_part 的字段换 'aster_bad_unit'→22023；`EXTRACT('aster_bad_unit' FROM TIMESTAMP '2024-01-01')`→22023；`date_trunc('week',INTERVAL '1 month')`、`EXTRACT(TIMEZONE FROM TIMESTAMP '2024-01-01')` 及 date_part('timezone',同 timestamp)→0A000。错误无成功行/NULL 载荷；NULL 与非法字段混合不作为错误阶段的唯一证据。 | `SELECT EXTRACT(DAY FROM CAST(NULL AS DATETIME)) AS v`→NULL/自身整数目标，不能变成 PG numeric。非法单位在 MySQL 依自身语法/签名拒绝，错误阶段、码/SQLSTATE 与警告待采，不能照搬 PG 22023。NULL 正式求值分支已静态读到，typed NULL 与双方言协议未验证。 |

所有 PG 成功项覆盖 simple Q 与 Parse/statement Describe/Bind/portal Describe/Execute、text/binary 参数到 text/binary 结果；源参数使用显式 cast 的 $1::T 重绑值/NULL；date_trunc/date_part 的字段参数可用 $2::text、三参 zone 参数可用 $3::text，EXTRACT 的字段保留在语法树中而不以 Bind 替换，分析固定 T 和返回身份，Bind 不换重载。statement Describe 和 portal Describe 的格式差异沿首块，Execute 不重发 RowDescription；NULL 或零行仍有固定类型描述。时区变化用两个 PG 连接交错，并在同一已准备语句执行前改变 TimeZone，区分 stable 执行上下文与三参指定区域；显式区域结果瞬时不变但文本显示随会话变化。MySQL Query/Prepare/Execute 独立采 code/flags/charset/length/decimals、sql_mode 与时区，验证 PG 名称隐藏及 EXTRACT 不被全局注册切换污染。错误采分析/折叠/执行实际阶段和完整错误字段；缺 codec/工厂记工程缺口，不冒充参考端语义拒绝。五组及官方/源码证据不批准别名、下推或功能支持。

#### 格式化函数契约

以下 FUNC-FMT 五组均为**待新增、未运行**目标，逐条分别对照参考 PG/AsterSQL PG 与参考 MySQL/AsterSQL MySQL。PG 成功项统一单列 AS v、text/25/-1/-1、无错误，除明确 NULL 项外非 NULL；结果空格和编码字节精确比较，不用 trim 或近似数值比较掩盖格式差异。固定 UTF-8，基线 lc_time/lc_numeric/lc_monetary=C、TimeZone=UTC。

| 五组场景 | PG 输入与预期值/NULL/类型/错误 | MySQL 入口、自身旁路与证据限制 |
| --- | --- | --- |
| FUNC-FMT-TIME：时间格式 | `SELECT to_char(TIMESTAMP '2024-01-02 17:03:04.123456','YYYY-MM-DD HH24:MI:SS.US') AS v`→'2024-01-02 17:03:04.123456'；`to_char(TIMESTAMP '2021-01-01','IYYY-IW')`→'2020-53'；`to_char(INTERVAL '36 hours','HH24:MI:SS')`→'36:00:00'，不能按日内时间取模。 | to_char 在 MySQL 拒绝；合法旁路 `SELECT DATE_FORMAT(CAST('2024-01-02 17:03:04.123456' AS DATETIME(6)),'%Y-%m-%d %H:%i:%s.%f') AS v`→相同可见文本/自身字符串目标；不发送 PG interval 语法，不由本例批准别名，精确列定义未采。 |
| FUNC-FMT-NUM：数值格式 | `SELECT to_char(148.5::numeric,'FM999.990') AS v`→'148.500'；`to_char(-485::int4,'999MI')`→'485-'；`to_char(12::int8,'FM000')`→'012'；分别以 `125.5::float4`、`125.5::float8` 和 'FM999.0'→'125.5'，不把浮点输入先转任意精度数值而抹去源身份。非法 `to_char(12::numeric,'99.9V99')`→42601，无成功行，阶段待采。 | to_char 均隐藏；合法独立数值旁路 `SELECT FORMAT(148.5,3,'en_US') AS v`→'148.500'/自身字符串目标。FORMAT 的 decimals/locale 参数不等同 PG 模板；PG V/小数点负例不发送给 FORMAT，MySQL 精确元数据/警告未采。 |
| FUNC-FMT-TEMPLATE：字面量/修饰符 | `SELECT to_char(TIMESTAMP '2024-01-02','"Year="YYYY FMDD DDTH') AS v`→'Year=2024 2 02ND'，证明引号字面量、FM 仅下一时间项、TH；同源模板 '%Y'→'%4'，不能按 DATE_FORMAT %Y 输出 2024。`to_char(TIMESTAMP '2024-01-02','"YYYY"')`→'YYYY'。 | `SELECT DATE_FORMAT(CAST('2024-01-02' AS DATETIME),'%Y') AS v`→'2024'/自身字符串目标；同源模板 'YYYY'→'YYYY'。to_char 名称仍拒绝；反向 PG 调用 `DATE_FORMAT(TIMESTAMP '2024-01-02','%Y')` 目标 42883，禁止用模板翻译自动放开名称。 |
| FUNC-FMT-NULL：NULL/额外时间边界 | 对每个已列 T，`SELECT to_char(NULL::T,'YYYY') AS v`→NULL/text；`to_char(12::numeric,NULL::text)`、`to_char(TIMESTAMP '2024-01-02',NULL::text)`→NULL/text。`to_char(TIMESTAMP '2024-01-02','')` 与 `to_char(TIMESTAMP 'infinity','YYYY')`→NULL/text，和非 NULL 空串区分。`to_char('abc'::text,'YYYY'::text)`→42883，无值。 | `SELECT DATE_FORMAT(CAST(NULL AS DATETIME),'%Y') AS v` 及 `DATE_FORMAT(CAST('2024-01-02' AS DATETIME),CAST(NULL AS CHAR))`→NULL/自身字符串目标；同有效日期空模板→非 NULL 空串目标。MySQL 不接受 PG infinity 字面量作同义输入；to_char 仍隐藏，错误字段与 wire 待采。 |
| FUNC-FMT-CONTEXT：locale/时区 | `SELECT to_char(TIMESTAMPTZ '2024-01-02 01:30:00+00','YYYY-MM-DD HH24:MI TZH:TZM') AS v` 在 UTC→'2024-01-02 01:30 +00:00'，America/New_York→'2024-01-01 20:30 -05:00'；timestamp 同墙钟输入及同模板去掉 TZH:TZM 后两会话均 '2024-01-02 01:30'。C 下 `to_char(TIMESTAMP '2024-01-02','TMMonth')`→'January'，`to_char(1234.5::numeric,'FM9G999D0')`→'1,234.5'。另在参考端确认存在的德语 locale 下采同两条，目标为该 locale 的一月译名与分组/小数字符，精确 locale 名及字节待采；. / , 固定标点不随配置变。 | `SELECT DATE_FORMAT(CAST('2024-01-02 01:30:00' AS DATETIME),'%Y-%m-%d %H:%i') AS v`→'2024-01-02 01:30'/自身字符串目标，不能充当 timestamptz 瞬时/偏移证据；独立 `FORMAT(1234.5,1,'de_DE')`→'1.234,5' 目标，局部测试只证明算法意图。MySQL 自身字符/locale/时区配置与精确元数据另采，不把 PG lc_* 设置发给 MySQL。 |

每族另以合法 MySQL DATE_FORMAT 调用在 PG 入口验证名称拒绝 42883，以合法 PG to_char 调用在 MySQL 验证名称拒绝（自身错误码/SQLSTATE/消息待采），此为两入口可见性隔离目标。PG 成功项覆盖 Q 与 Parse/statement Describe/Bind/portal Describe/Execute，显式 `$1::T,$2::text` 绑定重绑值/NULL、text/binary 参数和结果，分析固定 T/text，Bind 不改重载；两个 Describe 的格式差异沿首块，Execute 不重发描述。零行/全 NULL 仍固定 text 身份。MySQL Query/Prepare/Execute 独立采列定义、警告、sql_mode 与字符设置。两 PG 连接和一 MySQL 连接交错执行；同一已准备语句在执行前变更 TimeZone/允许修改的 locale 配置，验证 stable 上下文及缓存隔离，并恢复配置。非 C locale 不可用须记录环境缺口，不能静默跳过后宣称全组通过。错误采实际阶段与完整字段，缺工厂/codec 单列工程缺口。当前有效运行用例数为 0，五族只是官方目标/静态意图，没有功能通过、别名或下推批准。

#### 数组函数契约

以下 FUNC-ARR 五组全部为**待新增、未运行**。表中 a 是输入替换记号，每个表达式独立使用 `SELECT <表达式> AS v`，不是新建变量/测试设施。除显式 SQL NULL 外结果非 NULL；合法项无错误，计数/追加类型与 wire 沿本块模板。MySQL 名称隔离使用其合法语法，避免 PG 数组语法先报错掩盖名称门禁。

| 五组场景 | PG 输入及预期值/NULL/类型/错误 | MySQL 入口与证据限制 |
| --- | --- | --- |
| FUNC-ARR-EMPTY：空数组 | a=`ARRAY[]::int4[]`；`array_length(a,1)`→SQL NULL/int4；`cardinality(a)`→0/int4；`array_append(a,7::int4)`→{7}/int4[]、shape=[1]、lower=[1]。 | 每组三函数名均以 `SELECT array_length(JSON_ARRAY(),1) AS v`、`SELECT cardinality(JSON_ARRAY()) AS v`、`SELECT array_append(JSON_ARRAY(),7) AS v` 验证隐藏/拒绝，自身错误码/SQLSTATE/消息待采；不能用 JSON 空数组代表 PG 空数组。 |
| FUNC-ARR-NULL-ARRAY：数组 SQL NULL | a=`NULL::int4[]`；`array_length(a,1)`、`cardinality(a)`→SQL NULL/int4；`array_length(ARRAY[1]::int4[],NULL::int4)`→SQL NULL/int4；`array_append(a,7::int4)`→{7}/int4[]；`array_append(a,NULL::int4)`→{NULL}/int4[]，后两者 shape=[1]、lower=[1]，不是外层 NULL。 | 三名称用同一合法参数骨架，将 JSON_ARRAY() 换成 NULL，仍拒绝，不能因 NULL 常量折叠绕过可见性；错误与元数据未采。 |
| FUNC-ARR-NULL-ELEMENT：NULL 元素与定型 | a=`ARRAY[1,NULL]::int4[]`；`array_length(a,1)`→2/int4，`cardinality(a)`→2/int4；`array_append(a,NULL::int4)`→{1,NULL,NULL}/int4[]、shape=[3]/lower=[1]。`array_append(ARRAY[1]::int4[],2::int8)`→{1,2}/int8[]、元素 OID 20、结果 OID 1016，不能沿用输入 1007。 | 合法骨架改用 JSON_ARRAY(1,NULL)，仍拒绝 PG 名称；JSON null 与 SQL 元素 NULL 无等价批准，提升例不要求 MySQL 提供 PG 类型语法或 OID。 |
| FUNC-ARR-MULTIDIM：多维长度 | a=`ARRAY[[1,2,3],[4,5,6]]::int4[]`；`array_length(a,1)`→2、维号 2→3、3/0/-1→SQL NULL，均 int4；`cardinality(a)`→6/int4；`array_append(a,7::int4)`→22000、无结果。形状 [2,3] 不把数组 OID 1007 改成另一“二维 OID”。 | 合法骨架改用 JSON_ARRAY(JSON_ARRAY(1,2,3),JSON_ARRAY(4,5,6))，三名仍拒绝；MySQL JSON 嵌套数组不作矩形 shape 或 PG 多维追加证据。 |
| FUNC-ARR-LOWER：非默认下界 | a=`'[0:1]={10,20}'::int4[]`；`array_length(a,1)`→2/int4，`cardinality(a)`→2/int4，`array_append(a,30::int4)`→[0:2]={10,20,30}/int4[]、shape=[3]、lower=[0]。下界换 -2 的 `a='[-2:-1]={10,20}'::int4[]` 追加后→[-2:0]={10,20,30}。`array_append('[2147483647:2147483647]={1}'::int4[],2::int4)`→22003、无结果，不能环绕索引。 | 沿 EMPTY 的合法骨架验证名称隐藏；MySQL 无本块 PG 下界载体，不伪造等价输入/返回 OID。拒绝实际阶段与字段未采，不将未实现形状误判为参考端成功。 |

每组分别对照 PG 参考端/AsterSQL PG；成功项覆盖 Q 与 Parse/statement Describe/Bind/portal Describe/Execute，显式 `$1::int4[]`、维号 `$2::int4` 或元素 `$2::int4`，提升例用 `$2::int8`。参数 text/binary × 结果 text/binary 四组合验证同一规范值、维度/下界/元素 NULL；重复 Bind 换值、空数组、数组 NULL 或 NULL 元素均不换已选签名/返回 OID。NULL 结果与零行仍有固定描述，两个 Describe 的格式差异沿首块，Execute 不重发描述；错误采实际阶段，不强制延迟到 Execute。MySQL 覆盖 Query/Prepare/Execute 的名称拒绝，并以既有合法 `SELECT 1 AS v` 作成功旁路，单独采自身列定义、警告和错误码/SQLSTATE，不引入新 JSON 函数验收。两个 PG 连接和一个 MySQL 连接交错验证可见性/缓存隔离。缺数组表示、codec、定型或工厂单列工程缺口，不当作参考 PG 目标错误；当前有效运行用例为 0，无功能通过证据。

#### 范围函数契约

以下四组均为**待新增、未运行**目标，每个表达式独立执行 `SELECT <表达式> AS v`。r 为表内输入替换记号，不是新增变量设施。PG 合法项可见且成功无错误，除显式 NULL 外为非 NULL；返回身份按本块模板，成功 SQLSTATE 不适用。

| 场景 | PG 输入与值/NULL/类型/错误目标 | MySQL 入口及证据限制 |
| --- | --- | --- |
| FUNC-RNG-EMPTY-NULL：空与 NULL | r=`'empty'::int4range`：lower(r)/upper(r)→SQL NULL/int4，isempty(r)→true/bool；r=`NULL::int4range` 三者均 SQL NULL，类型分别 int4/int4/bool。`int4range(4,4)`→empty/int4range；`int4range(4,4,'[]')`→[4,5)，非空。 | 合法语法 `isempty(NULL)`、`int4range(4,4)` 目标名称拒绝，MySQL 自身错误码/SQLSTATE 待采；不能把 PG cast 语法错误当名称门禁证据。两侧无当前运行记录。 |
| FUNC-RNG-UNBOUNDED：缺端点 | `int4range(NULL::int4,5)`→(,5)，lower→NULL/int4、upper→5/int4、isempty→false/bool；`int4range(2,NULL::int4)`→[2,)，lower→2、upper→NULL、isempty→false；`int4range(NULL::int4,NULL::int4,'[]')`→(,)，两提取 NULL、isempty=false。均为非 NULL range，缺侧自动排除边界，不是 subtype 的显式 infinity。 | `int4range(NULL,5)`/`numrange(NULL,5)` 仍隐藏；PG 无界域不映射为 MySQL 字符串 NULL 语义。成功旁路 `SELECT 1 AS v`→1/自身整数，精确列定义待采。 |
| FUNC-RNG-BOUNDS：开闭与非法构造 | 对 flags 四值 [)、(]、[]、()，`numrange(1::numeric,3::numeric,flags)` 输出相应边界，lower=1、upper=3/numeric、isempty=false；`numrange(4::numeric,4::numeric,'[]')` 为单点非空，其余三种同端点为空。`int4range(5,2)`→22000；`int4range(1,3,NULL::text)`→22000；flags='bad'→42601，均无成功行。 | `lower('AbC')`→'abc'、`upper('AbC')`→'ABC'/自身字符串为保留目标；`numrange(1,3,'[]')` 隐藏。字符串大小写结果不证明 PG range 提取；MySQL 自身元数据/错误字段未采。 |
| FUNC-RNG-CANON：离散规范化 | `int4range(1,4,'(]')`→[2,5)，lower=2、upper=5/int4；`int4range(2,5)` 得同一规范 range；`int4range(1,2,'()')`→empty。连续 `numrange(1::numeric,4::numeric,'(]')` 仍 (1,4]、提取 1/4/numeric。`int4range(1,2147483647,'[]')`→22003，不把溢出自动变无界或换 int8range。 | range 构造名称仍隐藏，lower/upper 的字符串签名保持可见；PG 专属输入不跨发到 MySQL 作同值对比。全部只具官方目标/静态入口证据。 |

每组独立对照参考 PG/AsterSQL PG，成功项覆盖 Q 与 Parse/statement Describe/Bind/portal Describe/Execute；用 `$1::int4range`、`$1::numrange` 或定型端点/flags 参数，text/binary 参数×结果四组合，重复 Bind 普通值、empty、无界、SQL NULL 不换已选签名和 OID。全 NULL/零行仍有固定描述，Execute 不重发描述；构造 NULL 端点不按 strict 提前吞掉。MySQL Query/Prepare/Execute 用合法骨架检查隐藏名称及保留字符串签名，采自身 code/flags/charset/length/decimals、警告和错误；两个 PG 连接与一个 MySQL 连接交错验证缓存/可见性隔离。codec、表示或工厂缺失单列工程缺口，不能将其记作目标拒绝通过；当前有效运行用例为 0。

#### 必需系统函数契约

以下五组为**待新增、未运行**目标；每项表达式独立 `SELECT <表达式> AS v`，类型及 wire 按本块模板。固定无同名用户函数、有效数据库 db_a/db_b、UTF-8；成功项除显式 schema NULL 外非 NULL，成功 SQLSTATE 不适用。

| 场景 | 输入及 PG 值/NULL/类型/错误目标 | 事实提供者、两入口与缺证边界 |
| --- | --- | --- |
| FUNC-SYS-SESSION：会话变化 | A/B 分别连接 db_a/db_b，current_database()→各库名/name；A 的 search_path 从 public 改 pg_catalog 再改空，current_schema() 依次 public、pg_catalog、SQL NULL/name，B 不变；RESET 回到夹具记录的默认值。Parse current_schema() 后 SET 再 Bind/Execute 应观察新值。 | 5/7 提供所属连接数据库与路径，6 提供有效 schema。MySQL DATABASE() 只反映自身库，PG SET 不改变它；同值不批准别名。现有局部测试只有 test 与 public/catalog/空路径，不证明双库、完整元数据或两入口隔离。 |
| FUNC-SYS-VISIBILITY：权限与可见性 | 参考夹具建存在的 s_hidden（无 USAGE）、s_ok（有 USAGE），SET search_path TO missing,s_hidden,s_ok；current_schema()→s_ok/name。路径只剩 missing,s_hidden→SQL NULL/name；隐式 pg_catalog 仍可解析限定系统函数。授予/撤销 USAGE 后在下一语句重新验证有效结果。 | 6 提供真实存在/授权事实，5 按当前角色构造有效路径；不能从函数名或路径文本伪造。当前 AsterSQL 有界路径拒绝任意 schema 是工程缺口，非 PG 目标；GRANT/角色基础设施属外部依赖。MySQL 不公开 current_schema() PG 签名，错误码/SQLSTATE 待采，不能拿 PG SET 语法错误作名称隔离证据。 |
| FUNC-SYS-TYPEOF：类型内省 | pg_typeof(1::int4)、pg_typeof(NULL::int4) 都返回 regtype 值 23，text 显示 integer；pg_typeof(NULL::text)→regtype 值 25，text 显示 text；均非 NULL，结果列 OID 恒为 2206。定型参数 `$1::int4` 重复 Bind 1/NULL 保持 integer；pg_typeof()→42883 无成功行。 | 第 3 章提供分析类型，6 提供真实类型目录与 regtype 显示，server 编码。MySQL 合法骨架 pg_typeof(1) 目标名称拒绝，自身诊断待采；不把内部 MySQL BIGINT 直接报告为 PG int4。typed NULL、参数、regtype codec 和完整工厂均无当前运行证据。 |
| FUNC-SYS-IDENTITY：连接身份 | 同时活动的 PG A/B 重复 pg_backend_pid() 各得稳定、非 NULL、正 int4 且两者不同；各值须等于本连接 BackendKeyData 的 backend ID，独立取消只作用于对应连接。断开后不要求 ID 永不重用。version() 返回真实服务版本说明/text，与所声明能力和启动参数一致，不逐字等同参考 PG 的机器/编译器串。 | 7 提供 backend ID/生命周期/取消关联，构建信息提供者和 7 提供版本策略；第 4 章只定义求值。MySQL CONNECTION_ID()/VERSION() 保持自身值和类型，不能截断 u64 到 int4 或互相覆盖版本缓存。现有启动 server_version 测试不是 SQL version/PID 证据；本块只定义交接，不运行取消平台。 |
| FUNC-SYS-ISOLATION：两入口隔离 | 交错 A/B 的 current_database/current_schema/pg_typeof/pg_backend_pid 与 MySQL M 的 DATABASE()/VERSION()/CONNECTION_ID()；PG version() 与 MySQL VERSION() 同名各选自身签名及返回元数据，重复准备/执行不得泄漏 A/B/M 状态。PG current_database(1)→42883，无成功行。 | 第 4 章选择签名与工厂，5/7 供连接事实，6 供目录；MySQL 中 current_database()/current_schema()/pg_backend_pid()/pg_typeof(1) 隐藏 PG 名称，分别用合法 MySQL 函数调用采自身拒绝，错误码/SQLSTATE 未采。PG DATABASE()/CONNECTION_ID() 不开放跨方言别名，名称无匹配目标 42883。没有双入口全链运行证据。 |

每组成功项分别在参考 PG 和 AsterSQL PG 采 Q、Parse/statement Describe、Bind/portal Describe、Execute，text/binary 结果及适用的参数格式；无参函数不虚构参数载荷，typeof 参数覆盖 text/binary × 结果格式。schema=NULL 和零行仍保留 OID/typlen/typmod；Execute 不重发描述。name/regtype codec 或工厂缺失单列依赖，不能退回 text 冒充通过。MySQL Query/Prepare/Execute 单独记录值/NULL、自身列定义、错误和警告，两个 PG 连接与一个 MySQL 连接交错采样。现有源码及断言意图与官方目标分别记录，有效运行用例为 0。

#### MySQL专属函数隔离

以下四组为**待新增、未运行**的验收目标，各表达式独立 `SELECT <表达式> AS v`。MySQL 指 AsterSQL MySQL 入口及相应参考端；TIDB_VERSION 参考使用记录完整版本的 TiDB/本仓库 Go，普通 MySQL 无此专属函数不作成功参考。固定无同名用户/扩展函数、UTF-8、记录 MySQL 会话环境与构建信息；PG 用内建查找环境，不能用语法错误替代名称拒绝。

| 场景 | MySQL 成功、NULL、类型与错误保持目标 | PG 可见性拒绝目标 |
| --- | --- | --- |
| FUNC-MY-IFNULL | `IFNULL(CAST(NULL AS CHAR),CAST('fallback' AS CHAR))`→非 NULL 'fallback'/自身字符串；两参均 CAST(NULL AS CHAR)→SQL NULL/同类字符串。`IFNULL(1,2)`→1/自身整数；一参数 `IFNULL(1)` 保留参数数目错误，无成功行。另以可控报错表达式作备用分支验证首参非空不求值，局部 Error 断言不替代 SQL 证据。精确 flags/宽度/错误字段待采。 | `ifnull(NULL::text,'fallback'::text)` 及全 typed NULL 版本→42883，无成功行；不得改写 COALESCE。 |
| FUNC-MY-DATE | `DATE_FORMAT(CAST('2024-01-02 17:03:04' AS DATETIME),'%Y-%m-%d %H:%i:%s')`→非 NULL '2024-01-02 17:03:04'/自身字符串；日期或模板单独 typed NULL→SQL NULL/字符串；同有效日期空模板→非 NULL 空串。一参数 DATE_FORMAT('2024-01-02') 保留参数错误；零日期警告/错误按固定 sql_mode 对照 Go，精确诊断待采，不能因 PG 兼容改写。 | `date_format(TIMESTAMP '2024-01-02 17:03:04','%Y-%m-%d %H:%i:%s')` 及日期/模板 typed NULL→42883；不翻译为 to_char。 |
| FUNC-MY-JSON | `JSON_EXTRACT('{"a":1,"b":2}','$.a','$.b')`→非 NULL JSON [1,2]；`JSON_EXTRACT('{"a":null}','$.a')`→JSON null（IS NULL 为 false）；同文档 '$.missing'、SQL NULL 文档或路径→SQL NULL，结果类型仍 JSON/code 245。`JSON_EXTRACT('{"a":1}','$InvalidPath')`→路径错误、无成功行；非法 JSON 文档另采自身转换错误，不吞为缺失/NULL，精确诊断待采。 | `json_extract('{"a":1}'::json,'$.a'::text)`、文档/路径 typed NULL 和非法路径版本均→名称无匹配 42883；不能先运行 MySQL 路径解析或返回 PG JSON 值。 |
| FUNC-MY-ADMIN | `TIDB_VERSION()`→非 NULL 构建信息/自身字符串，内容等于同一服务构建的 GetTiDBInfo，不能固定参考 TiDB 品牌串冒充本构建；零参不适用 NULL 输入。`TIDB_VERSION(NULL)`→参数数目错误，无成功行；结果类型/flen 不由第一行推断，精确列定义/错误字段待采。 | `tidb_version()` 及 `tidb_version(NULL::text)`→42883，无成功行；不重写 PG version()，不泄露 MySQL 工厂。 |

四组分别重复 PG 未限定名与 pg_catalog 限定名；PG Q 和扩展 Parse 均须在分析阶段拒绝且无成功 RowDescription/DataRow，记录错误实际字段/阶段，不能拿缺工厂错误充作 42883。MySQL 对成功/NULL 采 Query、Prepare/Execute、参数值/NULL 重绑、零行列定义，逐列核对 code/flags/charset/length/decimals、载荷与警告。两个 PG 连接与一个 MySQL 连接交错重复调用，验证名称/工厂缓存不串方言；MySQL 原错误与 Go/TiDB 参考逐条核对。PG 未限定名仅在夹具排除同名用户函数时要求拒绝，不扩大为禁止用户定义函数。全部四组有效运行用例数为 0，当前仅完成契约证据与场景覆盖。

#### 运算符调用契约

以下四组为待新增、未运行目标，各表达式独立 `SELECT <表达式> AS v`；PG 使用显式类型，MySQL 使用自身 CAST，禁止给 MySQL 输入 `::` 后将语法拒绝当语义对照。两入口各自与对应参考端核对，不要求同符号得到同值。无同名用户/扩展遮蔽，固定 UTF-8，记录 MySQL sql_mode（除零组固定普通 SELECT、非严格模式及 ERROR_FOR_DIVISION_BY_ZERO 开启）、div_precision_increment=4 和字符环境。

| 场景 | PG 入口目标：输入、值/NULL、类型、错误 | MySQL 入口目标：可见性及自身行为 |
| --- | --- | --- |
| OP-CALL-DIV：整数/非整数 | `5::int4 / 2::int4`→2，`(-5)::int4 / 2::int4`→-2，均非 NULL/int4；`5::numeric / 2::numeric`→精确 2.5/numeric，不固定文本尾零；`NULL::int4 / 2::int4`→SQL NULL/int4。补一元 `-(5::int4)`→-5/int4、`-('-2147483648'::int4)`→22003，区分一元/二元。`('-2147483648'::int4) / (-1)::int4`→22003，无成功值。 | `CAST(5 AS SIGNED) / CAST(2 AS SIGNED)`→2.5000/自身 DECIMAL，负数版本→-2.5000；`CAST(5 AS SIGNED) DIV CAST(2 AS SIGNED)`→2/BIGINT，负数版本→-2；`CAST(5 AS DECIMAL(2,0)) / CAST(2 AS DECIMAL(2,0))`→2.5000/DECIMAL。typed NULL 除以 2→SQL NULL/相应类型；一元 `-CAST(5 AS SIGNED)`→-5/自身 signed 整数。以上均可见，成功无错误，不把 PG int4 结果宽度写入 MySQL。 |
| OP-CALL-ZERO：除零 | `1::int4 / 0::int4` 与 `1::numeric / 0::numeric`→22012，无成功值/NULL；`NULL::int4 / 0::int4`→SQL NULL/int4。常量折叠可提前失败；参数版 `$1::int4 / $2::int4` 用 1/0 验证执行错误，用 NULL/0 验证同型 NULL。 | 普通 SELECT 的 `CAST(1 AS SIGNED)/CAST(0 AS SIGNED)` 和 `CAST(1 AS SIGNED) DIV CAST(0 AS SIGNED)`→SQL NULL，分别保持 DECIMAL/BIGINT，收集除零警告；NULL 左参→NULL。警告精确码/SQLSTATE/消息及正式链传播仍待 Go/参考端采集，不拿局部 error_ctx 的错误证明 SQL 模式已接通；DML 严格模式不在本组。 |
| OP-CALL-CONCAT：文本拼接与 NULL | `'a'::text \|\| 'b'::text`→非 NULL 'ab'/text；`'a'::text \|\| NULL::text` 和 `NULL::text \|\| 'b'::text`→SQL NULL/text；`''::text \|\| 'b'::text`→'b'。合法 text/text 无错误，不能用 concat 函数的 NULL 行为代替 `\|\|`。 | PIPES_AS_CONCAT 关闭时 `1 \|\| 0`→1、`NULL \|\| 0`→SQL NULL/自身整数；开启时 `CAST('a' AS CHAR) \|\| CAST('b' AS CHAR)`→'ab'/自身字符串，单侧 CAST(NULL AS CHAR)→SQL NULL/字符串。结果/警告按模式分别核对，PG 不继承 MySQL 开关；不能把关闭模式下的逻辑 OR 误作文本拼接支持。 |
| OP-CALL-NULLSAFE：NULL 安全比较 | int4 对分别 `NULL IS NOT DISTINCT FROM NULL`→true，`NULL IS NOT DISTINCT FROM 1`→false，`1 IS NOT DISTINCT FROM 1`→true，`1 IS NOT DISTINCT FROM 2`→false（两侧均加 ::int4）；对应 IS DISTINCT FROM 逐项取反，均非 NULL/bool。对照 `NULL::int4 = 1::int4`→SQL NULL/bool；合法输入无错误。 | 使用同值的 CAST(... AS SIGNED) 与 `<=>`，两空/一空/等值/异值→1/0/1/0，均非 NULL/自身布尔整数表达式；普通 `=` 一空→SQL NULL。PG 不批准 `<=>` 内建别名，`1::int4 <=> 1::int4` 在无用户定义夹具下目标 42883，无成功行；不能把 nulleq 名称注册作为 PG 谓词接线证明。 |

PG `/` 和文本 `||` 同时核对未限定写法与 `OPERATOR(pg_catalog./)` / `OPERATOR(pg_catalog.||)`，NULL 安全谓词无 OPERATOR 同名形式，不适用该检查。四组成功/NULL 在 Q 与扩展 Parse/Bind/Describe/Execute 核对值、OID/typlen/typmod、文本/二进制格式，参数重绑与零行描述不改型；MySQL 核对 Query、Prepare/Execute、参数 NULL 重绑及零行列定义。错误可能在分析、折叠或执行阶段，分别保留原字段，不能强制所有错误都在 Parse；PG 参数版补执行与恢复证据。交错 PG/MySQL 连接与 MySQL 两种 pipes 模式，确认缓存保留方言/会话签名。元数据及诊断尚未采集，四组有效运行用例数均为 0，文档结构通过不能升级为功能通过。

#### 普通聚合调用契约

以下 AGG-CALL 五组全部为**待新增、未运行**目标。每组分别比较 AsterSQL PG/参考 PG 与 AsterSQL MySQL/自身参考端，名称均目标可见；PG 另重复 pg_catalog 限定名。固定无同名用户/扩展遮蔽、UTF-8、MySQL div_precision_increment=4，合法项无错误，除显式 SQL NULL 外非 NULL。PG t 用表列出的定型 VALUES；MySQL 用等值 `SELECT CAST(... AS SIGNED/DECIMAL/DOUBLE) ... UNION ALL ...` 派生表，不发送 PG 的 ::/VALUES/FILTER 语法来冒充比较。

| 场景 | PG 输入、值/NULL/类型及错误目标 | MySQL 自身入口目标与限制 |
| --- | --- | --- |
| AGG-CALL-EMPTY：无行全局 | `SELECT count(*) AS n,count(x) AS c,sum(x) AS s,avg(x) AS a FROM (SELECT 1::int4 AS x WHERE false) t`→一行 (0,0,NULL,NULL)，类型 (int8,int8,int8,numeric)。补 `GROUP BY x`→零行，四列类型仍固定。 | 等值空表 `(SELECT CAST(1 AS SIGNED) AS x WHERE 1=0) t` 运行同投影→一行 (0,0,NULL,NULL)，BIGINT/BIGINT/DECIMAL/DECIMAL；加 GROUP BY x→零行，保留列定义。 |
| AGG-CALL-NULL：全 NULL | 对 `(VALUES(NULL::int4),(NULL::int4)) t(x)` 运行同四列投影→(2,0,NULL,NULL)，类型同 EMPTY；区别无行与两行全 NULL。 | `(SELECT CAST(NULL AS SIGNED) AS x UNION ALL SELECT CAST(NULL AS SIGNED)) t`→同值/NULL，自身 BIGINT/BIGINT/DECIMAL/DECIMAL；不把 0 行、NULL 和 decimal 零混同。 |
| AGG-CALL-GROUP：分组与状态隔离 | 对 `(VALUES(1,1::int4),(1,3::int4),(2,NULL::int4)) t(g,x)`，`SELECT g,count(*),count(x),sum(x),avg(x) ... GROUP BY g ORDER BY g`→(1,2,2,4,2)、(2,1,0,NULL,NULL)，g 为 int4，其余同 EMPTY。每组加 `FILTER (WHERE false)`→各自 (g,0,0,NULL,NULL)，两组仍存在；NULL 分组键另补两行 NULL 键应同组。 | 等值 SIGNED 派生表，同无 FILTER 查询→相同值/NULL，g 为自身整数，其余自身类型。保留 NULL 键同组目标；FILTER 版本不要求 MySQL 可见，不把删除所有行的 WHERE 改写作等价证据。 |
| AGG-CALL-DISTINCT-FILTER：重复/筛选/排序 | t=`(VALUES(1::int4,true),(1::int4,false),(2::int4,true),(NULL::int4,true),(9::int4,NULL::bool)) t(x,keep)`；普通四列投影→(5,4,13,3.25)。另独立查 `count(*) FILTER (WHERE keep),count(DISTINCT x) FILTER (WHERE keep),sum(DISTINCT x ORDER BY x DESC) FILTER (WHERE keep),avg(DISTINCT x) FILTER (WHERE keep)`→(3,2,3,1.5)，类型同 EMPTY。`sum(DISTINCT x ORDER BY keep)` 须拒绝，无成功行，精确诊断待采；普通数值结果相同不能单独证明排序器执行过，需局部有序输入轨迹证据。 | 等值 SIGNED x 与 1/0/NULL keep，普通四列→(5,4,13,3.25)/自身类型；不加 FILTER 的 `COUNT(DISTINCT x),SUM(DISTINCT x),AVG(DISTINCT x)`→(3,12,4)。用 `COUNT(CASE WHEN keep THEN 1 END),COUNT(DISTINCT CASE WHEN keep THEN x END),SUM(DISTINCT CASE WHEN keep THEN x END),AVG(DISTINCT CASE WHEN keep THEN x END)`→(3,2,3,1.5)，仅本夹具值对照，不批准通用 FILTER 改写或 MySQL 聚合内 ORDER BY。 |
| AGG-CALL-NUMERIC：结果身份与错误 | 各 T∈int2/int4/int8/numeric/float4/float8，分别 `SELECT sum(x) AS s,avg(x) AS a FROM (VALUES(1::T),(2::T)) t(x)`→(3,1.5)；SUM 类型依次 int8/int8/numeric/numeric/float4/float8，AVG 前四 numeric、后两 float8。追加 `sum('x'::text)`→42883；sum(x) 对两个合法 '9223372036854775807'::int8→18446744073709551614/numeric，不得先按 i64 溢出。 | 分别 SIGNED、DECIMAL(10,2)、DOUBLE 的 1/2 派生表，SUM/AVG→3/1.5，前两自身 DECIMAL、后一 DOUBLE，精确文本尾零与 metadata 待采；SIGNED 的两个 9223372036854775807 求和保持自身 DECIMAL 域。PG sum(text) 拒绝不强加 MySQL，字符串转换/警告依自身参考端另采。 |

每组 PG 成功/NULL/零行覆盖 Q 与 Parse/statement Describe/Bind/portal Describe/Execute；参数化输入用 `$1::int4` 等固定类型，text/binary 参数×结果四组合，非空/NULL 重绑不换 OID/typmod，Execute 不重发描述。count=0 与 SQL NULL 分开核对载荷，MySQL Query/Prepare/Execute 同样记录列定义和 NULL bitmap；错误独立记录真实阶段/字段，不能用缺工厂或 codec 失败当目标拒绝。两 PG 连接与 MySQL 交错调用，验证签名、状态及会话策略不串线。

局部/最终与下推另需计划/状态证据：将 1、1、2 分在不同局部分区（夹带空分区），核对全局 DISTINCT count=2、sum=3、avg=1.5；将 [1] 与 [2,3,4] 分开，AVG 最终=2.5，不能平均 1 与 3 得 2。比较单阶段及合法合并的值/NULL/类型/错误，验证 FILTER 各自独立及排序信息保留，浮点另外记录次序/舍入差异；未获此证据不批准共享或下推。当前五组及此门禁有效运行用例均为 0，结构检查不表示功能通过。

### 后续细分

#### 三层身份与契约模板

下一块先承接本模板，细化注册/重载的方言可见性、名称环境、候选排序、隐式 cast 与错误；依赖第 2 章身份/名称及第 3 章类型/转换/元数据，不递归修复缺失子系统。随后各函数族与独立调用模型只补自身实例和窄边界，引用本模板及对应官方来源；缺证项补证后再批准共享别名或下推。本块只给文档交接，不生成代码任务。

来源映射：蓝图任务 4 步骤 1 的三层契约由身份表与标量候选贯通，步骤 3 的所有字段由统一模板覆盖；编号任务的同名不同签名、同值不同类型、typed NULL 分别由 FUNC-ID-ABS-SIGNATURE/SAME-VALUE/TYPED-NULL 覆盖，ERROR 补充域/签名拒绝。与前序章节的连接方言、E/A/I 资格和固定元数据规则一致，无需修改第 1–3 章。其余来源步骤和完整章节验收由后续文档任务承接，不随本块完成升级。

#### 注册工厂与方言可见性

下一块只细化已过滤候选中的重载、隐式 cast 与歧义，不重建名称解析器；函数族块再补 date_trunc/IFNULL 的规范实现 ID、NULL/优化属性与 wire 细节。后续实现先为现有正式构造链补最小方言接线及聚焦门禁证据，缺工厂问题单独追踪，不递归补齐函数库；系统目录事实仍交工作流 6，协议诊断交 7。

来源映射：蓝图任务步骤 1 的方言选择层落在四阶段证据表；步骤 2 的 PG/MySQL 专属代表落在 FUNC-REG-PG-ONLY/MYSQL-ONLY；步骤 3 的可见性、别名及模板字段落在差异表与四族验收，UNKNOWN/UNLINKED 区分名称缺失和实现缺口。首块三层身份、NULL/元数据与第 2 章连接身份、第 3 章转换资格保持一致；本块未修改前序正文或升级任何运行状态。

#### 重载与隐式转换选择

后续实现只交接三个窄边界：接收第 2 章可见集合及第 3 章类型/I 资格的候选选择；已选签名到正式工厂及合法 coercion 的局部接线；参数/结果身份在 Prepare/Describe/Execute 的连续证据。缺工厂、完整类型系统、UDF/多态、目录和协议设施分别追踪，不借本块补齐。可变/default 的完整开放须先补载体与聚焦解析证据，本轮不生成实现任务。

来源映射：蓝图步骤 1 的方言签名层与步骤 3 的重载/cast/NULL/返回和 wire 字段，由本块目标段、模板差异表及 FUNC-RES 五组覆盖；精确签名、unknown、NULL、无匹配、歧义逐行有输入与类型/错误目标，其他模板字段明确不适用或缺口。前两批已交付的名称门禁、首块 strict/类型身份与第 3 章 E/A/I、unknown 参数及固定描述保持一致。结构、引用和差异审阅仅证明文档交付；正确性/兼容性目标未实测，没有性能结论，不升级任何功能支持状态。

#### 等价共享与别名门槛

后续只交接 COALESCE 共同类型到惰性正式调用、NULLIF 比较签名到返回身份、text 输出 collation 和两入口证据四个窄边界，依赖第 3 章及前块名称/转换资格。先补代表签名证据再讨论别名或下推；缺工厂、类型系统与协议设施分别追踪，不在本块补建，不提前生成代码任务。

来源映射：蓝图步骤 2 第一类由两候选和逐签名批准门槛覆盖；步骤 3 的名称/可见性、重载/cast、NULL、collation/时区、确定性、返回、wire、下推和错误均映射模板差异表及 FUNC-EQUIV 五组。普通值、全 NULL、混型、未达错误、collation 分别定位 VALUE/ALL-NULL/MIXED/UNREACHED-ERROR/COLLATION；每组含两入口输入与目标或明确缺口。人工审阅已确认与前块“不默认批准别名”、第 3 章定型和固定描述一致。本轮仅文档完成，正确性/兼容性仍为未实测目标，性能无结论。

#### 同名异义分流

后续仅交接 concat 的方言 NULL/输出策略到正式构造链、极值特殊表达式的共同类型/NULL/参数边界、已定型比较/拼接可复用域及两入口描述证据；依赖前块名称隔离、第 3 章类型/I 资格和 server 元数据，不扩展为函数库或执行器重建，不生成代码任务。

来源映射：蓝图步骤 2 第二类由 concat/greatest/least 分流理由和独立语义 ID 覆盖，步骤 3 全字段由本块模板差异表覆盖；NULL、混型、字符串排序、参数个数分别映射 FUNC-SPLIT-NULL/MIXED/COLLATION/ARITY，两入口各有输入、值/NULL/类型或拒绝目标及缺口。与前块“不因同名/局部实现批准共享”、PG 特殊表达式边界和第 3 章固定身份一致，前序正文未修改。文档正确性/兼容性目标尚未实测，性能无结论。

#### 时间截断与提取

后续仅交接已定型三种源到截断/字段内核、numeric/float8 返回策略到正式工厂、会话/显式区域上下文到日历计算、固定结果身份到 server 的四个窄边界；依赖第 2 章语法、前块注册/重载、第 3 章类型/codec 和工作流 5 的会话区域。先补本块五组及两入口证据，再考虑共享算法与下推；不生成代码任务，不递归补齐时间库或协议设施。

来源映射：蓝图步骤 2 的 PG 核心 date_trunc/date_part 由本块重载表及 FUNC-TEMP 覆盖，步骤 3 全部模板字段由本块差异表承接；跨日、区域、DST、interval、NULL/非法单位分别定位 DAY/ZONE/DST/INTERVAL/NULL-INVALID。date_part 与 EXTRACT 返回类型分别有 §9.9.1 和 pg_proc.dat 证据，精度/两入口可见性/错误及 wire 缺口均明确。人工审阅与第 2 章 EXTRACT 语法、第 3 章三种身份和前块“不凭同名批准别名”一致，未重写前序正文。correctness/兼容性仅为文档目标，运行行为未验证，性能无结论。

#### 格式化函数契约

后续仅交接已选源类型到时间/数值模板内核、locale/TimeZone 执行上下文、text 固定身份到正式工厂/server 三个窄边界；依赖前块 resolver、第 3 章类型/cast 和工作流 5 的会话生命周期。先补五族及双方言元数据证据，再讨论可复用组件；不递归补全 DATE_FORMAT、locale 或时间子系统，不生成代码任务。

来源映射：蓝图步骤 2 的 PG 核心 to_char 与 MySQL DATE_FORMAT 隔离由本块名称和签名表覆盖；步骤 3 全部模板字段由差异表承接。任务的时间格式、数值格式、字面量/修饰符、NULL、locale/时区分别定位 FUNC-FMT-TIME/NUM/TEMPLATE/NULL/CONTEXT，均含两入口输入、预期或明确缺口；返回文本元数据及错误阶段有公共验收规则。与前块不默认批准别名、第 3 章 text/时间/数值身份及第 2 章连接方言一致。correctness/兼容性仍是未实测目标，性能无结论；只完成文档契约，不修改第 1–3 章或其他契约块。

#### 数组函数契约

后续只交接第 3 章规范数组到维长/基数求值、共同元素类型到单元素追加、固定返回数组身份到正式工厂/server 三个窄边界；依赖前块名称/resolver 与第 3 章元素 I 资格、shape/lower/NULL/codec。先补五组与双入口证据，再讨论共享或下推；不生成代码任务，不借此补全数组或类型子系统。

来源映射：蓝图步骤 2 的 PG 核心数组代表由三签名及 FUNC-ARR 覆盖，步骤 3 的全部模板字段由本块差异表承接；空数组、数组 SQL NULL、NULL 元素、多维长度、非默认下界分别定位 EMPTY/NULL-ARRAY/NULL-ELEMENT/MULTIDIM/LOWER，含输入、值/NULL/类型/错误、两入口可见性和证据等级。元素提升及返回数组 OID 依赖落在 NULL-ELEMENT 与公共 wire 段。人工审阅与第 3 章基础数组两层 NULL/shape/lower、前块按分析固定身份和名称隔离一致，保留前序正文。correctness/兼容性仅为文档目标，运行未验证，性能无结论。

#### 范围函数契约

后续仅交接独立 range/subtype 身份及 empty/无界/开闭表示契约、已定型构造与规范化到正式标量链、固定 range/提取结果身份到 server codec 三个窄边界；第 3 章缺少通用 range 契约，须先补该前置文档与代表证据，不能由本块完成推断基础依赖已具备。先获得四组与两入口证据，再讨论共享/下推；本轮不生成代码任务。

来源映射：蓝图步骤 2 的 PG 核心 range 由 lower/upper/isempty/构造及 FUNC-RNG 四组承接，步骤 3 全部字段由统一模板差异表覆盖；空范围与 NULL、无界端点、开闭界、离散规范化分别定位 EMPTY-NULL/UNBOUNDED/BOUNDS/CANON，每组有输入、值/NULL/类型/错误与两入口可见性和证据等级。人工审阅与第 3 章未承诺 range、前块按分析固定身份及逐签名门禁一致，未改前序正文；correctness/兼容性仅为文档目标，运行未验证，性能无结论。

#### 必需系统函数契约

后续仅按首批客户端探测补数据库/schema 的正式标量接线，先向工作流 5/6/7 明确事实接口、读取时点与不可用诊断；再为版本、backend 身份和 pg_typeof 补具体客户端需求、窄 name/regtype 身份/codec 与独立验收。函数求值不拥有目录表或身份生命周期，不借本块扩大目录/权限/协议实现。范围、优先级与通用工厂接线仍须实施阶段重新核实，本轮不生成代码任务。

来源映射：来源步骤 2“PG 必需系统函数”由本块五候选和 FUNC-SYS 五组承接；步骤 4 的系统函数/目录责任由模块表、事实责任字段、范围外与上述接口交接承接。首块全部模板字段已逐项填写；权限过滤、类型内省、连接身份和两入口缺证均显式保留。文档完整性不表示 correctness/兼容性/性能已实测，未运行 AsterSQL/PG/MySQL 或 18.6 参考端。

#### MySQL专属函数隔离

后续仅交接现有候选过滤→四名称 MySQL 签名/工厂→原求值/返回元数据三个窄边界；依赖前块 resolver、任务 4 等价门槛、第 3 章类型和工作流 7 的双协议描述。实施前重新核实动态工厂及四组独立测试，缺工厂和精确列定义单独追踪；不递归补全时间/JSON/管理系统，本轮不生成代码任务。

来源映射：蓝图步骤 2 第四类由四个规范实现候选与 FUNC-MY-IFNULL/DATE/JSON/ADMIN 承接；步骤 3 的名称、可见性、重载/cast、NULL、collation/时区、确定性、返回/wire、下推、错误由本块统一模板各行覆盖。每类至少一组 MySQL 成功与 PG 拒绝、NULL 适用性/类型/错误保持均有明确目标或缺口；跨方言开放只引用任务 4 的证据门槛。correctness/兼容性为未实测目标，性能无结论；文档完成不表示函数支持通过。

#### 运算符调用契约

后续仅交接三个窄边界：整数/numeric `/` 的签名、值域与除零策略；text/text `||` 的 strict 拼接及模式隔离；int4 NULL 安全谓词的比较绑定与 bool 返回身份。各边界先核实 PG 语法节点 → 已选签名 → 正式工厂/求值链 → server 元数据，再决定能否复用局部算法，不另建一套求值器，不预先生成实现任务。

依赖第 2 章语法/名称环境、第 3 章类型及 cast、工作流 6 的运算符身份和 7/8 的协议/参考端证据；数组/range 运算符另族定义输入形状、重载与 NULL 后再验收。工厂或类型基础缺口登记为实现阶段依赖，不扩大本轮文档任务，也不阻塞已完成的文档交付；普通聚合沿下一块独立调用模型继续。

#### 普通聚合调用契约

后续仅交接三个窄边界：已选 aggregate 签名/固定返回身份到描述符；每聚合 FILTER、DISTINCT、排序到每组 transition/finalize；最终结果身份到 server 两协议描述。依赖第 2 章语法与名称、第 3 章类型/I 转换及前块 resolver，优先复用现有聚合链并先证明两条执行路径等价。局部/最终与下推须另过上述状态合并门禁；窗口、有序集合和 SRF 分别交后块，不提前生成代码任务。

来源映射：蓝图任务步骤 4 的 aggregate 独立调用模型由模块链及状态表覆盖；步骤 3 全部模板字段由本块差异表填写。无行全局、全 NULL、分组、重复/FILTER、数值返回分别映射 AGG-CALL-EMPTY/NULL/GROUP/DISTINCT-FILTER/NUMERIC，聚合内排序及局部/最终另有正反例和补证要求。人工审阅与首块三层身份、第 3 章固定类型及前块不默认共享的规则一致，未改第 1–3 章或其他正文。correctness/兼容性仅为文档目标，运行未验证，性能无结论。

## 5. SQL会话与事务语义

## 6. 系统目录与内省契约

## 7. 线协议认证与数据通道

## 8. 客户端与双方言共存验收
