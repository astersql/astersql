// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// DDL 元数据构建上下文（metabuild）。
//
// 在解析 CREATE/ALTER 等 DDL 并生成表/索引元信息时，需要一组来自会话
// （session）与显式选项的配置：是否允许生成列含自增、是否强制主键、
// 聚簇索引（clustered index，主键与行存储绑定）模式、行 ID 分片位数、
// 预分裂 Region 数等。本模块将这些配置汇总为 `MetaBuildContext`。

/// 聚簇索引定义模式：控制新建表主键是否采用聚簇存储。
///
/// - `IntOnly`：仅整型主键可聚簇（默认）；
/// - `Off`：关闭聚簇主键；
/// - `On`：强制启用聚簇主键。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ClusteredIndexMode {
    #[default]
    /// 仅整型主键使用聚簇索引。
    IntOnly,
    /// 禁用聚簇索引。
    Off,
    /// 启用聚簇索引。
    On,
}

/// 会话侧与元数据构建相关的变量快照。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionVariables {
    /// 生成列（generated column）表达式中是否允许自增列。
    pub enable_auto_increment_in_generated: bool,
    /// 是否要求表必须有主键。
    pub primary_key_required: bool,
    /// 当前是否处于受限 SQL（如内部系统语句）上下文。
    pub in_restricted_sql: bool,
    /// 聚簇索引模式。
    pub clustered_index_mode: ClusteredIndexMode,
    /// 隐式行 ID（`_tidb_rowid`）的分片位数（shard bits）。
    pub shard_row_id_bits: u64,
    /// 建表时预分裂的 Region 数量。
    pub pre_split_regions: u64,
}

/// 从会话构造 metabuild 上下文时所需的会话侧输入。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionBuildContext {
    /// 表达式求值上下文标识（不可为空）。
    pub expression_context: String,
    /// 会话可见的最新 InfoSchema（信息模式，缓存的库表元数据视图）标识。
    pub latest_info_schema: String,
    /// 会话变量快照。
    pub variables: SessionVariables,
}

/// 覆盖或补充会话默认值的构建选项。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BuildOption {
    /// 覆盖“生成列是否允许自增”。
    EnableAutoIncrementInGenerated(bool),
    /// 覆盖“是否强制主键”。
    PrimaryKeyRequired(bool),
    /// 覆盖聚簇索引模式。
    ClusteredIndexMode(ClusteredIndexMode),
    /// 覆盖行 ID 分片位数。
    ShardRowIdBits(u64),
    /// 覆盖预分裂 Region 数。
    PreSplitRegions(u64),
    /// 覆盖 InfoSchema 标识。
    InfoSchema(String),
    /// 是否抑制“索引过长”错误（部分导入/兼容路径需要）。
    SuppressTooLongIndexError(bool),
}

/// 元数据构建过程使用的完整上下文。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MetaBuildContext {
    /// 表达式求值上下文。
    pub expression_context: String,
    /// 生成列是否允许引用自增列。
    pub enable_auto_increment_in_generated: bool,
    /// 是否强制要求主键。
    pub primary_key_required: bool,
    /// 聚簇索引模式。
    pub clustered_index_mode: ClusteredIndexMode,
    /// 行 ID 分片位数。
    pub shard_row_id_bits: u64,
    /// 预分裂 Region 数。
    pub pre_split_regions: u64,
    /// InfoSchema 标识。
    pub info_schema: String,
    /// 是否抑制索引过长错误。
    pub suppress_too_long_index_error: bool,
    /// 构建过程中累积的警告信息。
    pub warnings: Vec<String>,
}

/// 以会话配置为默认值，再叠加调用方传入的 `BuildOption`，构造 `MetaBuildContext`。
///
/// 受限 SQL 下即使会话开启“强制主键”，也会关闭该要求，避免内部语句被拦截。
pub fn new_meta_build_context_with_session(
    session: &SessionBuildContext,
    other_options: impl IntoIterator<Item = BuildOption>,
) -> MetaBuildContext {
    assert!(
        !session.expression_context.is_empty(),
        "session expression context must not be empty"
    );
    let variables = &session.variables;
    // 先从会话变量填充默认值；受限 SQL 时不强制主键。
    let mut context = MetaBuildContext {
        expression_context: session.expression_context.clone(),
        enable_auto_increment_in_generated: variables.enable_auto_increment_in_generated,
        primary_key_required: !variables.in_restricted_sql && variables.primary_key_required,
        clustered_index_mode: variables.clustered_index_mode,
        shard_row_id_bits: variables.shard_row_id_bits,
        pre_split_regions: variables.pre_split_regions,
        info_schema: session.latest_info_schema.clone(),
        suppress_too_long_index_error: false,
        warnings: Vec::new(),
    };
    // Go appends caller options after session-derived defaults, so later options override them.
    // 调用方选项后写覆盖会话默认值，与 Go 语义一致。
    for option in other_options {
        match option {
            BuildOption::EnableAutoIncrementInGenerated(value) => {
                context.enable_auto_increment_in_generated = value;
            }
            BuildOption::PrimaryKeyRequired(value) => context.primary_key_required = value,
            BuildOption::ClusteredIndexMode(value) => context.clustered_index_mode = value,
            BuildOption::ShardRowIdBits(value) => context.shard_row_id_bits = value,
            BuildOption::PreSplitRegions(value) => context.pre_split_regions = value,
            BuildOption::InfoSchema(value) => context.info_schema = value,
            BuildOption::SuppressTooLongIndexError(value) => {
                context.suppress_too_long_index_error = value;
            }
        }
    }
    context
}
