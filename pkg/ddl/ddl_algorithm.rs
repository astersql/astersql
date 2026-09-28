// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// DDL ALTER 算法（ALGORITHM）选择模块。
//
// DDL（Data Definition Language，数据定义语言）指 CREATE/ALTER/DROP 等修改
// 表结构的语句。MySQL 兼容语法允许在 `ALTER TABLE ... ALGORITHM=xxx` 中显式
// 指定变更算法，用于控制表结构变更的执行方式（例如是否复制整表数据）。
// 本模块负责根据语句指定的算法与当前操作实际支持的算法集合，选出最终
// 采用的算法，并在两者不一致时产生错误信息，与 MySQL 的行为保持一致。

/// ALTER 语句可指定的算法类型。
///
/// 各变体按 Go `ast.AlgorithmType` 的值顺序排列（枚举派生了 `Ord`，
/// 比较即依赖此声明顺序）：
/// - `Default`：未显式指定算法，由系统选择默认值；
/// - `Copy`：拷贝方式，创建新表并复制全部数据，开销最大。
/// - `Inplace`：原地变更，不复制整表数据（可能需要重建索引）；
/// - `Instant`：即时变更，仅修改元数据，不动存量数据。
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub enum AlgorithmType {
    #[default]
    Default,
    Copy,
    Inplace,
    Instant,
}
/// ALTER 操作的种类，用于决定默认算法。
///
/// - `AddConstraint`：添加约束（如 CHECK 约束），默认使用 `Inplace`；
/// - `Other`：其他类型的变更，默认使用 `Instant`。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AlterKind {
    AddConstraint,
    Other,
}
/// 描述某类 ALTER 操作支持的算法集合及其默认算法。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AlterAlgorithm {
    /// 该操作支持的算法列表，按优先（开销较小）到次优排列。
    pub supported: Vec<AlgorithmType>,
    /// 未显式指定算法时采用的默认算法。
    pub default_algorithm: AlgorithmType,
}
/// 算法不匹配错误：用户请求的算法与实际选中的算法不一致时产生。
///
/// 对应 MySQL 的 `ER_ALTER_OPERATION_NOT_SUPPORTED` 类错误场景。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AlgorithmError {
    /// 用户在语句中显式请求的算法。
    pub requested: AlgorithmType,
    /// 系统实际选中的算法。
    pub selected: AlgorithmType,
    /// 该操作的默认算法，用于错误提示。
    pub default_algorithm: AlgorithmType,
}
/// 在支持的算法集合中为用户指定的算法挑选一个合适的实际算法。
///
/// 规则：
/// - 若用户未显式指定（`Default`），直接返回默认算法，且无错误；
/// - 否则按 Go 的枚举值比较，在 `supported` 列表中找到第一个不低于请求值的
///   更优算法（例如指定 `Copy` 时可选 `Inplace` 或 `Instant`）；找不到则回退
///   为 `Default`；
/// - 若最终选中的算法与用户指定的不同，附带一个 `AlgorithmError`，
///   由上层决定是报错还是仅告警。
pub fn proper_algorithm(
    specified: AlgorithmType,
    algorithm: &AlterAlgorithm,
) -> (AlgorithmType, Option<AlgorithmError>) {
    // 未显式指定算法：直接采用默认算法，不产生错误。
    if specified == AlgorithmType::Default {
        return (algorithm.default_algorithm, None);
    }
    // 按 Go 枚举值顺序查找第一个满足 specified <= supported 的算法，
    // 即允许把用户请求升级为更优的可用算法。
    let selected = algorithm
        .supported
        .iter()
        .copied()
        .find(|supported| specified <= *supported)
        .unwrap_or(AlgorithmType::Default);
    // 选中算法与用户请求不一致时记录错误信息，交由调用方处理。
    let error = (specified != selected).then_some(AlgorithmError {
        requested: specified,
        selected,
        default_algorithm: algorithm.default_algorithm,
    });
    (selected, error)
}
/// 根据 ALTER 操作种类与用户指定的算法，解析出最终使用的算法。
///
/// 添加约束类操作默认（且仅支持）`Inplace`，其余操作默认（且仅支持）
/// `Instant`；随后交由 [`proper_algorithm`] 完成匹配与错误判定。
pub fn resolve_alter_algorithm(
    kind: AlterKind,
    specified: AlgorithmType,
) -> (AlgorithmType, Option<AlgorithmError>) {
    // 依据操作种类确定默认算法：添加约束需要校验存量数据，故用 Inplace。
    let default_algorithm = if kind == AlterKind::AddConstraint {
        AlgorithmType::Inplace
    } else {
        AlgorithmType::Instant
    };
    proper_algorithm(
        specified,
        &AlterAlgorithm {
            supported: vec![default_algorithm],
            default_algorithm,
        },
    )
}
