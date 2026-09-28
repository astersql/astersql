// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// `metamanager` 单元测试：覆盖分组解析、非法 group ID 与 [`get_info`] 回退路径。

use super::{
    GC_MANAGEMENT_TYPE_KEY, GC_MANAGEMENT_TYPE_KEYSPACE_LEVEL, GC_MANAGEMENT_TYPE_UNIFIED,
    GLOBAL_GROUP_ID, GROUP_ADDRS_KEY, GROUP_ID_KEY, KeyspaceMeta, MetaServiceError, get_group,
    get_info,
};
use std::collections::HashMap;

/// 将字符串切片转为 `Vec<String>`，便于构造 PD/分组地址。
fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

/// 按名称与键值对构造 [`KeyspaceMeta`]。
fn keyspace_meta(name: &str, entries: &[(&str, &str)]) -> KeyspaceMeta {
    KeyspaceMeta {
        name: name.to_owned(),
        config: entries
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect::<HashMap<_, _>>(),
    }
}

/// 排序后比较地址列表，忽略配置中的顺序差异。
fn sorted(mut values: Vec<String>) -> Vec<String> {
    values.sort();
    values
}

/// 覆盖 `get_group`：空 meta、合法独立分组、统一 GC 拒绝、地址 trim、空地址、全局回退。
#[test]
fn test_get_group() {
    let pd_addrs = strings(&["127.0.0.1:2379"]);
    let expected_addrs = strings(&["127.0.0.1:2388", "127.0.0.1:2389"]);

    // keyspace meta 为空时应直接失败。
    let error = get_group(None, &pd_addrs).expect_err("nil keyspace meta must fail");
    assert!(matches!(error, MetaServiceError::NilKeyspaceMeta));

    // 独立分组 + keyspace 级 GC：应解析出 group1 与两个地址。
    let mut meta = keyspace_meta(
        "",
        &[
            (GROUP_ID_KEY, "group1"),
            (GROUP_ADDRS_KEY, "127.0.0.1:2388,127.0.0.1:2389"),
            (GC_MANAGEMENT_TYPE_KEY, GC_MANAGEMENT_TYPE_KEYSPACE_LEVEL),
        ],
    );
    let group = get_group(Some(&meta), &pd_addrs).unwrap();
    assert_eq!(group.group_id, "group1");
    assert_eq!(sorted(group.addrs), sorted(expected_addrs.clone()));

    // 配置了独立 group 但使用统一 GC：必须报 KeyspaceLevelGcRequired。
    meta = keyspace_meta(
        "ks-unified-gc",
        &[
            (GROUP_ID_KEY, "group1"),
            (GROUP_ADDRS_KEY, "127.0.0.1:2388,127.0.0.1:2389"),
            (GC_MANAGEMENT_TYPE_KEY, GC_MANAGEMENT_TYPE_UNIFIED),
        ],
    );
    let error = get_group(Some(&meta), &pd_addrs).expect_err("unified GC must fail");
    assert!(matches!(
        &error,
        MetaServiceError::KeyspaceLevelGcRequired { .. }
    ));
    assert!(error.to_string().contains("requires keyspace-level GC"));

    // 地址串含空白与空段时，trim 后应得到有效地址。
    meta = keyspace_meta(
        "",
        &[
            (GROUP_ID_KEY, "group1"),
            (GROUP_ADDRS_KEY, " 127.0.0.1:2388, ,127.0.0.1:2389,  "),
            (GC_MANAGEMENT_TYPE_KEY, GC_MANAGEMENT_TYPE_KEYSPACE_LEVEL),
        ],
    );
    let group = get_group(Some(&meta), &pd_addrs).unwrap();
    assert_eq!(group.group_id, "group1");
    assert_eq!(sorted(group.addrs), sorted(expected_addrs));

    // 仅空白分隔符：解析后地址为空，视为 GroupNotMatch。
    meta.config
        .insert(GROUP_ADDRS_KEY.to_owned(), " , \t,  ".to_owned());
    assert!(matches!(
        get_group(Some(&meta), &pd_addrs),
        Err(MetaServiceError::GroupNotMatch)
    ));

    // 缺少 GROUP_ADDRS_KEY：同样 GroupNotMatch。
    meta.config.remove(GROUP_ADDRS_KEY);
    assert!(matches!(
        get_group(Some(&meta), &pd_addrs),
        Err(MetaServiceError::GroupNotMatch)
    ));

    // 去掉 GROUP_ID_KEY：回退到全局分组与 PD 地址。
    meta.config.remove(GROUP_ID_KEY);
    let group = get_group(Some(&meta), &pd_addrs).unwrap();
    assert_eq!(group.group_id, GLOBAL_GROUP_ID);
    assert_eq!(sorted(group.addrs), sorted(pd_addrs.clone()));
}

/// 非法 group ID（纯数字、空格、点号、仅分隔符+数字）均应返回 InvalidGroupId。
#[test]
fn test_get_group_rejects_invalid_group_id() {
    let pd_addrs = strings(&["127.0.0.1:2379"]);
    for (name, group_id) in [
        ("numeric only", "1"),
        ("contains space", "group 1"),
        ("contains dot", "group.1"),
        ("contains only separators and digits", "1-2_3"),
    ] {
        let meta = keyspace_meta(
            "",
            &[
                (GROUP_ID_KEY, group_id),
                (GROUP_ADDRS_KEY, "127.0.0.1:2388,127.0.0.1:2389"),
                (GC_MANAGEMENT_TYPE_KEY, GC_MANAGEMENT_TYPE_KEYSPACE_LEVEL),
            ],
        );
        let error = match get_group(Some(&meta), &pd_addrs) {
            Ok(_) => panic!("case should fail: {name}"),
            Err(error) => error,
        };
        assert!(matches!(&error, MetaServiceError::InvalidGroupId), "{name}");
        assert!(
            error.to_string().contains("invalid meta service group id"),
            "{name}"
        );
    }
}

/// 覆盖 `get_info`：无 meta 时全局回退；有合法独立配置时保留 PD 地址并解析分组。
#[test]
fn test_get_info() {
    let pd_addrs = strings(&["127.0.0.1:2379"]);
    let info = get_info(None, &pd_addrs).unwrap();
    assert_eq!(info.group.group_id, GLOBAL_GROUP_ID);
    assert_eq!(info.group.addrs, pd_addrs);
    assert_eq!(info.pd_addrs, pd_addrs);

    let meta = keyspace_meta(
        "",
        &[
            (GROUP_ID_KEY, "group2"),
            (GROUP_ADDRS_KEY, "127.0.0.1:2388,127.0.0.1:2389"),
            (GC_MANAGEMENT_TYPE_KEY, GC_MANAGEMENT_TYPE_KEYSPACE_LEVEL),
        ],
    );
    let info = get_info(Some(&meta), &pd_addrs).unwrap();
    assert_eq!(info.group.group_id, "group2");
    assert_eq!(info.pd_addrs, pd_addrs);
    assert_eq!(
        sorted(info.group.addrs),
        sorted(strings(&["127.0.0.1:2388", "127.0.0.1:2389"]))
    );
}
