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

// 慢日志规则（slowlogrule）迁移期单元测试。
//
// 对照 Go 侧初始化与字段语义，验证会话规则构造、条件阈值类型保留，
// 以及全局规则按连接 ID（含 `-1` 表示全局）分桶的结构。

#[cfg(test)]
mod tests {
    use crate::{
        GlobalSlowLogRules, NewSessionSlowLogRules, SlowLogCondition, SlowLogRule, SlowLogRules,
    };
    use std::collections::{HashMap, HashSet};

    /// 验证 `NewSessionSlowLogRules` 与 Go 初始化一致：
    /// 保留原始规则串与字段集，有效字段为空，并标记需更新有效字段。
    #[test]
    fn new_session_slow_log_rules_matches_go_initialization() {
        let fields = HashSet::from(["Conn_ID".to_owned(), "Query_time".to_owned()]);
        let rules = Box::new(SlowLogRules {
            RawRules: "Conn_ID: 42, Query_time: 1.5".to_owned(),
            Fields: fields.clone(),
            Rules: Vec::new(),
        });
        let rules_ptr = &*rules as *const SlowLogRules;

        let session = NewSessionSlowLogRules(rules);

        assert_eq!(&*session.SlowLogRules as *const SlowLogRules, rules_ptr);
        assert_eq!(
            session.SlowLogRules.RawRules,
            "Conn_ID: 42, Query_time: 1.5"
        );
        assert_eq!(session.SlowLogRules.Fields, fields);
        assert!(session.EffectiveFields.is_empty());
        assert_eq!(session.GlobalRawRulesHash, 0);
        assert!(session.NeedUpdateEffectiveFields);
    }

    /// 验证单条规则条件保留 Go 的 `any` 阈值类型（浮点/布尔），
    /// 且同一规则内多条件为逻辑与（AND）分组。
    #[test]
    fn conditions_retain_go_any_thresholds_and_and_grouping() {
        let rule = SlowLogRule {
            Conditions: vec![
                SlowLogCondition {
                    Field: "Query_time".to_owned(),
                    Threshold: Box::new(1.5_f64),
                },
                SlowLogCondition {
                    Field: "Succ".to_owned(),
                    Threshold: Box::new(true),
                },
            ],
        };

        assert_eq!(rule.Conditions.len(), 2);
        assert_eq!(rule.Conditions[0].Field, "Query_time");
        assert_eq!(
            rule.Conditions[0].Threshold.downcast_ref::<f64>(),
            Some(&1.5)
        );
        assert_eq!(rule.Conditions[1].Field, "Succ");
        assert_eq!(
            rule.Conditions[1].Threshold.downcast_ref::<bool>(),
            Some(&true)
        );
    }

    /// 验证全局规则映射同时保留连接级条目与 key=`-1` 的全局条目。
    #[test]
    fn global_rules_preserve_connection_scoped_and_global_entries() {
        let global = SlowLogRules {
            RawRules: "Query_time: 1".to_owned(),
            Fields: HashSet::from(["Query_time".to_owned()]),
            Rules: Vec::new(),
        };
        let connection = SlowLogRules {
            RawRules: "Conn_ID: 42, Succ: true".to_owned(),
            Fields: HashSet::from(["Conn_ID".to_owned(), "Succ".to_owned()]),
            Rules: Vec::new(),
        };
        let mut rules_map = HashMap::new();
        rules_map.insert(-1, Box::new(global));
        rules_map.insert(42, Box::new(connection));

        let rules = GlobalSlowLogRules {
            RawRules: "Query_time: 1; Conn_ID: 42, Succ: true".to_owned(),
            RawRulesHash: 99,
            RulesMap: rules_map,
        };

        assert_eq!(rules.RawRulesHash, 99);
        assert_eq!(rules.RulesMap.len(), 2);
        assert_eq!(rules.RulesMap[&-1].RawRules, "Query_time: 1");
        assert_eq!(rules.RulesMap[&42].RawRules, "Conn_ID: 42, Succ: true");
    }
}
