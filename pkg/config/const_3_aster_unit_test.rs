// Copyright 2026 AsterSQL.
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

// config 常量相关的单元测试（第 3 组）。
//
// 本文件验证从 Go(TiDB) 迁移到 Rust 的配置常量与配置结构行为是否与 Go 版本保持一致，
// 覆盖以下几个方面：
// - TiFlash 弹性伸缩（AutoScaler）类型常量与字符串到类型的映射；
// - 外部工作负载（external-workload）配置的规范化（去空白）与合法性校验；
// - 存储引擎类型 `StoreType`（如 tikv、unistore、mocktikv）的字符串保持与有效性判断；
// - Keyspace（键空间，TiDB 中用于多租户隔离的逻辑命名空间）可观测性字段的
//   校验、去重、解析与深拷贝语义。

use astersql_config::{
    AWSASStr, AWSASType, DefASStr, DefAWSAutoScalerAddr, DefRowsForSampleRate, GCPASStr, GCPASType,
    GetAutoScalerType, InvalidASType, IsValidAutoScalerConfig, KeyspaceObservability,
    KeyspaceObservabilityField, KeyspaceObservabilityValues, MockASStr, MockASType, RoleMaster,
    StoreType, StoreTypeList, TestASStr, TestASType, defaultExternalWorkload,
};
use std::collections::HashMap;

/// 验证常量取值与 TiFlash AutoScaler 类型映射与 Go 版本一致。
///
/// - `DefRowsForSampleRate`：统计信息采样率相关的默认行数阈值；
/// - `GetAutoScalerType`：把伸缩器名称字符串映射为枚举类型，未知名称返回 `InvalidASType`；
/// - `IsValidAutoScalerConfig`：判断名称是否为可用于生产配置的合法伸缩器（test 类型不合法）。
#[test]
fn constants_and_tiflash_mapping_match_go() {
    assert_eq!(DefRowsForSampleRate, 110_000);
    assert_eq!(DefASStr, AWSASStr);
    assert_eq!(
        DefAWSAutoScalerAddr,
        "tiflash-autoscale-lb.tiflash-autoscale.svc.cluster.local:8081"
    );
    assert_eq!(GetAutoScalerType(MockASStr), MockASType);
    assert_eq!(GetAutoScalerType(AWSASStr), AWSASType);
    assert_eq!(GetAutoScalerType(GCPASStr), GCPASType);
    assert_eq!(GetAutoScalerType(TestASStr), TestASType);
    assert_eq!(GetAutoScalerType("unknown"), InvalidASType);
    assert!(IsValidAutoScalerConfig(MockASStr));
    assert!(IsValidAutoScalerConfig(AWSASStr));
    assert!(IsValidAutoScalerConfig(GCPASStr));
    assert!(!IsValidAutoScalerConfig(TestASStr));
}

/// 验证外部工作负载配置的规范化与校验行为与 Go 版本一致。
///
/// 规则要点：
/// - 未启用（Enable=false）时不做校验也不修改字段；
/// - 启用后会去除字段首尾空白，Role 为空时回退为默认的 `RoleMaster`；
/// - Role 非法或 controller-addr 为空时返回对应错误。
#[test]
fn external_workload_normalizes_and_validates_like_go() {
    // 未启用时：即便 Role 非法也校验通过，且字段保持原样不被修剪。
    let mut disabled = defaultExternalWorkload();
    disabled.Role = " INVALID ".into();
    assert!(disabled.Valid().is_ok());
    assert_eq!(disabled.Role, " INVALID ");
    assert!(disabled.isConfigured());

    // 启用后：空白 Role 回退为 RoleMaster，地址与连接池名被去除首尾空白。
    let mut enabled = defaultExternalWorkload();
    enabled.Enable = true;
    enabled.Role = "  ".into();
    enabled.ControllerAddr = " controller:1234 \n".into();
    enabled.TidbPool = " vip-tidb-pool\t".into();
    enabled.Valid().unwrap();
    assert_eq!(enabled.Role, RoleMaster);
    assert_eq!(enabled.ControllerAddr, "controller:1234");
    assert_eq!(enabled.TidbPool, "vip-tidb-pool");

    // 非法角色与空 controller-addr 应分别返回明确的错误信息。
    enabled.Role = "bogus".into();
    assert_eq!(
        enabled.Valid().unwrap_err(),
        "invalid external-workload role \"bogus\""
    );
    enabled.Role = RoleMaster.into();
    enabled.ControllerAddr.clear();
    assert!(
        enabled
            .Valid()
            .unwrap_err()
            .contains("controller-addr must not be empty")
    );
}

/// 验证 `StoreType` 保留任意字符串值的 Go 语义。
///
/// Go 中 StoreType 本质是字符串类型别名，可承载任意值；
/// 只有内置的 tikv/unistore/mocktikv 三种被认为是合法存储引擎。
#[test]
fn store_type_preserves_arbitrary_go_string_values() {
    let values = StoreTypeList();
    assert_eq!(
        values.iter().map(StoreType::String).collect::<Vec<_>>(),
        ["tikv", "unistore", "mocktikv"]
    );
    assert!(values.iter().all(StoreType::Valid));

    // 自定义引擎名可以原样保存，但不会被判定为合法类型。
    let custom = StoreType::from("custom-engine".to_owned());
    assert_eq!(custom.String(), "custom-engine");
    assert!(!custom.Valid());
}

/// 测试辅助函数：快速构造一个 Keyspace 可观测性字段。
///
/// 参数依次为：元数据来源键、监控指标标签名、慢日志字段名、
/// 语句日志字段名、以及该元数据项是否必填。
fn field(
    source: &str,
    metric: &str,
    slow: &str,
    stmt: &str,
    required: bool,
) -> KeyspaceObservabilityField {
    KeyspaceObservabilityField {
        Source: source.into(),
        MetricLabel: metric.into(),
        SlowLogField: slow.into(),
        StmtLogField: stmt.into(),
        Required: required,
    }
}

/// 验证 Keyspace 可观测性配置的命名校验与大小写不敏感的去重逻辑。
///
/// 校验规则：source 不能为空；至少配置一个输出（指标标签/慢日志/语句日志）；
/// 指标标签与慢日志字段名需符合命名规范并以规定前缀（keyspace_meta_）开头；
/// 指标标签重复检查忽略大小写。
#[test]
fn keyspace_observability_validates_names_and_case_insensitive_duplicates() {
    // 完全合法的单字段配置应通过校验。
    let valid = KeyspaceObservability {
        Fields: vec![field(
            "meta_a",
            "keyspace_meta_label_a",
            "Keyspace_meta_slow_a",
            "stmt_a",
            true,
        )],
    };
    valid.Valid().unwrap();

    // 逐一验证各类非法字段配置会产生包含预期关键字的错误信息。
    for (candidate, expected) in [
        (
            field("", "keyspace_meta_a", "", "", false),
            "source cannot be empty",
        ),
        (
            field("meta", "", "", "", false),
            "at least one output must be set",
        ),
        (
            field("meta", "1_label", "", "", false),
            "invalid metric-label",
        ),
        (
            field("meta", "service_scope", "", "", false),
            "must start with",
        ),
        (
            field("meta", "", "Bad Field", "", false),
            "invalid slow-log-field",
        ),
        (
            field("meta", "", "keyspace_meta_slow", "", false),
            "must start with",
        ),
    ] {
        let error = KeyspaceObservability {
            Fields: vec![candidate],
        }
        .Valid()
        .unwrap_err();
        assert!(
            error.contains(expected),
            "{error:?} did not contain {expected:?}"
        );
    }

    // 指标标签重复检查忽略大小写：keyspace_meta_label 与 KEYSPACE_META_LABEL 视为重复。
    let duplicate = KeyspaceObservability {
        Fields: vec![
            field(
                "a",
                "keyspace_meta_label",
                "Keyspace_meta_slow",
                "stmt",
                false,
            ),
            field(
                "b",
                "KEYSPACE_META_LABEL",
                "Keyspace_meta_other",
                "other",
                false,
            ),
        ],
    };
    assert!(
        duplicate
            .Valid()
            .unwrap_err()
            .contains("duplicated metric-label")
    );
}

/// 验证 Keyspace 可观测性元数据的解析、排序与深拷贝语义。
///
/// - `ResolveKeyspaceObservability`：用给定的元数据键值表填充各输出通道，
///   缺失必填项时报错；
/// - 慢日志字段按名称排序输出；
/// - `Clone` 为深拷贝，修改副本不影响原配置。
#[test]
fn keyspace_observability_resolves_sorts_and_clones_deeply() {
    let observability = KeyspaceObservability {
        Fields: vec![
            field("meta_b", "keyspace_meta_b", "Keyspace_meta_z", "", false),
            field(
                "meta_a",
                "keyspace_meta_a",
                "Keyspace_meta_a",
                "stmt_a",
                true,
            ),
        ],
    };
    let mut config = astersql_config::Config {
        keyspace_observability: observability,
        keyspace_observability_values: KeyspaceObservabilityValues::default(),
        ..Default::default()
    };
    let values = HashMap::from([
        ("meta_a".into(), "value_a".into()),
        ("meta_b".into(), "value_b".into()),
    ]);
    // 解析成功后：指标标签、慢日志字段（按名称排序）、语句日志字段均被正确填充。
    config.ResolveKeyspaceObservability(values).unwrap();
    assert_eq!(
        config.GetKeyspaceObservabilityMetricLabels()["keyspace_meta_a"],
        "value_a"
    );
    assert_eq!(
        config
            .GetKeyspaceObservabilitySlowLogFields()
            .iter()
            .map(|v| v.Name.as_str())
            .collect::<Vec<_>>(),
        ["Keyspace_meta_a", "Keyspace_meta_z"]
    );
    assert_eq!(
        config.GetKeyspaceObservabilityStmtLogFields()["stmt_a"],
        "value_a"
    );

    // Clone 是深拷贝：向副本插入新键不会污染原始配置。
    let mut cloned = config.keyspace_observability_values.Clone();
    cloned.MetricLabels.insert("new".into(), "value".into());
    assert!(
        !config
            .keyspace_observability_values
            .MetricLabels
            .contains_key("new")
    );

    // 缺少必填元数据项（meta_a 标记为 Required）时解析应报错。
    let missing = HashMap::from([("meta_b".into(), "value_b".into())]);
    assert_eq!(
        config.ResolveKeyspaceObservability(missing).unwrap_err(),
        "missing required keyspace metadata entry \"meta_a\""
    );
}
