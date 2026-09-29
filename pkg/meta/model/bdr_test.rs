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

// BDR Action 映射完整性测试：正向分组与反向映射必须一一对应、可逆。

use crate::group_1::*;
use crate::group_3::action_type_string;

#[test]
fn test_action_bdr_map() {
    // ActionMap 与 ActionBDRMap 应覆盖同一批 action，长度不一致意味着有 action 缺少反向 BDR 映射。
    let action_map_len = (0_u8..=94)
        .filter(|action| action_type_string(*action) != "none")
        .count();
    assert_eq!(action_map_len, ActionBDRMap.len());

    let mut total_actions = 0;
    for (bdr_type, actions) in BDRActionMap.iter() {
        for action in actions {
            // BDRActionMap 是按 BDR 类型分组的正向表；ActionBDRMap 必须能把每个 action 映射回同一类型。
            assert_eq!(
                Some(bdr_type),
                ActionBDRMap.get(action),
                "action {}",
                action
            );
        }
        total_actions += actions.len();
    }

    // 所有分组内 action 的总数必须等于反向映射大小，避免 ActionBDRMap 出现多余条目。
    assert_eq!(total_actions, ActionBDRMap.len());
}

#[test]
fn go_merge_12_materialized_view_action_categories() {
    let cases = [
        (ACTION_CREATE_MATERIALIZED_VIEW_LOG, SafeDDL),
        (ACTION_CREATE_MATERIALIZED_VIEW, SafeDDL),
        (ACTION_ALTER_MATERIALIZED_VIEW_REFRESH, SafeDDL),
        (ACTION_ALTER_MATERIALIZED_VIEW_ATTRIBUTES, SafeDDL),
        (ACTION_ALTER_MATERIALIZED_VIEW_LOG_PURGE, SafeDDL),
        (ACTION_DROP_MATERIALIZED_VIEW, UnsafeDDL),
        (ACTION_DROP_MATERIALIZED_VIEW_LOG, UnsafeDDL),
        (ACTION_DROP_MATERIALIZED_VIEW_SHADOW, UnsafeDDL),
        (ACTION_MVIEW_REFRESH_OUT_OF_PLACE_CUTOVER, UnsafeDDL),
        (ACTION_CREATE_MATERIALIZED_VIEW_SHADOW, UnsafeDDL),
    ];
    assert_eq!(ACTION_CREATE_MATERIALIZED_VIEW_LOG, 85);
    assert_eq!(ACTION_DROP_MATERIALIZED_VIEW_SHADOW, 94);
    for (action, expected) in cases {
        assert_eq!(ActionBDRMap.get(&action), Some(&expected));
        assert_ne!(action_type_string(action), "none");
    }
}
use crate::group_1::{ActionBDRMap, BDRActionMap};

/// 断言分组内动作总数等于反向映射大小，且每个动作能映回同一 BDR 类别。
#[test]
fn action_bdr_maps_are_complete_and_reversible() {
    // 正向表各分组长度之和应等于反向表条目数。
    let grouped = BDRActionMap.values().map(Vec::len).sum::<usize>();
    assert_eq!(grouped, ActionBDRMap.len());
    for (kind, actions) in BDRActionMap.iter() {
        for action in actions {
            assert_eq!(ActionBDRMap.get(action), Some(kind));
        }
    }
}
