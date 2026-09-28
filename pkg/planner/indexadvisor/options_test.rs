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

// 索引顾问配置项解析单测。
//
// 覆盖 `parse_duration` 对 Go 风格单位（s/ms）的解析，以及非法文本的拒绝，
// 确保 timeout 选项与持久化字符串语义一致。

/// 合法 duration 文本应按单位换算；非法值返回错误。
#[test]
fn option_duration_parser_matches_go_units_and_rejects_invalid_values() {
    use std::time::Duration;
    assert_eq!(
        crate::options::parse_duration("30s").unwrap(),
        Duration::from_secs(30)
    );
    assert_eq!(
        crate::options::parse_duration("1500ms").unwrap(),
        Duration::from_millis(1500)
    );
    assert_eq!(
        crate::options::parse_duration("0s").unwrap(),
        Duration::ZERO
    );
    // 非数字+单位形式（如 forever）应被拒绝。
    assert!(crate::options::parse_duration("forever").is_err());
    assert_eq!(
        crate::options::parse_duration("1.5s").unwrap(),
        Duration::from_millis(1500)
    );
    assert_eq!(
        crate::options::parse_duration("1h30m").unwrap(),
        Duration::from_secs(5400)
    );
    assert_eq!(crate::options::parse_duration("0").unwrap(), Duration::ZERO);
    assert_eq!(
        crate::options::parse_duration("+.5s").unwrap(),
        Duration::from_millis(500)
    );
    assert_eq!(
        crate::options::parse_duration("1µs").unwrap(),
        Duration::from_nanos(1_000)
    );
    assert_eq!(
        crate::options::parse_duration("0.0000000006s").unwrap(),
        Duration::ZERO
    );
}

#[test]
fn fill_options_ignores_invalid_persisted_integers_like_go() {
    use crate::options::{AdvisorOptions, OPT_MAX_NUM_INDEX, OptionStore, fill_options};
    use std::collections::BTreeMap;

    struct Store;
    impl OptionStore for Store {
        fn get(&self, _: &str, _: &[&str]) -> Result<BTreeMap<String, String>, String> {
            Ok(BTreeMap::from([(
                OPT_MAX_NUM_INDEX.to_string(),
                "not-an-integer".to_string(),
            )]))
        }
        fn set(&self, _: &str, _: &str, _: &str, _: &str) -> Result<(), String> {
            Ok(())
        }
    }

    let mut options = AdvisorOptions {
        max_num_indexes: 0,
        ..AdvisorOptions::default()
    };
    fill_options(&Store, &mut options, &[]).unwrap();
    assert_eq!(options.max_num_indexes, 0);
}

#[test]
fn option_store_reads_defaults_and_applies_overrides_in_go_order() {
    use crate::options::{
        AdvisorOptions, OPT_MAX_NUM_INDEX, OPT_TIMEOUT, OptionStore, OptionValue, fill_options,
    };
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    struct Store {
        values: RefCell<BTreeMap<String, String>>,
    }
    impl OptionStore for Store {
        fn get(&self, _: &str, _: &[&str]) -> Result<BTreeMap<String, String>, String> {
            Ok(self.values.borrow().clone())
        }
        fn set(&self, _: &str, name: &str, value: &str, _: &str) -> Result<(), String> {
            self.values
                .borrow_mut()
                .insert(name.to_string(), value.to_string());
            Ok(())
        }
    }

    let store = Store {
        values: RefCell::new(BTreeMap::from([
            (OPT_MAX_NUM_INDEX.to_string(), "7".to_string()),
            (OPT_TIMEOUT.to_string(), "2s".to_string()),
        ])),
    };
    let mut options = AdvisorOptions {
        max_num_indexes: 0,
        max_index_width: 0,
        max_num_query: 0,
        timeout: std::time::Duration::ZERO,
    };
    fill_options(
        &store,
        &mut options,
        &[(OPT_MAX_NUM_INDEX.to_string(), OptionValue::Integer(9))],
    )
    .unwrap();
    assert_eq!(options.max_num_indexes, 9);
    assert_eq!(options.max_index_width, 3);
    assert_eq!(options.max_num_query, 1000);
    assert_eq!(options.timeout, std::time::Duration::from_secs(2));
}

#[test]
fn option_validation_and_sequential_writes_match_go() {
    use crate::options::{
        OPT_MAX_NUM_INDEX, OPT_MAX_NUM_QUERY, OPT_TIMEOUT, OptionStore, OptionValue, set_option,
        set_options,
    };
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct Store(RefCell<Vec<(String, String)>>);
    impl OptionStore for Store {
        fn get(&self, _: &str, _: &[&str]) -> Result<BTreeMap<String, String>, String> {
            Ok(BTreeMap::new())
        }
        fn set(&self, _: &str, name: &str, value: &str, _: &str) -> Result<(), String> {
            self.0
                .borrow_mut()
                .push((name.to_string(), value.to_string()));
            Ok(())
        }
    }

    let store = Store::default();
    let error = set_options(
        &store,
        &[
            (OPT_MAX_NUM_QUERY.to_string(), OptionValue::Integer(111)),
            (OPT_TIMEOUT.to_string(), OptionValue::Text("-1s".into())),
            (OPT_MAX_NUM_INDEX.to_string(), OptionValue::Integer(7)),
        ],
    )
    .unwrap_err();
    assert!(error.contains("invalid duration"));
    assert_eq!(
        store.0.borrow().as_slice(),
        &[(OPT_MAX_NUM_QUERY.to_string(), "111".to_string())]
    );

    set_option(&store, OPT_TIMEOUT, OptionValue::Text("-0s".to_string())).unwrap();
    assert!(
        set_option(
            &store,
            OPT_MAX_NUM_INDEX,
            OptionValue::Unsigned(i64::MAX as u64 + 1),
        )
        .is_err()
    );
}
