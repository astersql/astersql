// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

//! Go TestLoadDataInitParam through the real parser, planner and controller.

#[path = "../init_param_test_support.rs"]
mod init_param_test_support;

#[test]
fn test_load_data_init_param() {
    use init_param_test_support::build_controller;
    // Same four invalid configurations and two fixed-row rejections as Go.
    for (sql, expected) in [
        (
            "load data infile '' into table load_data_test",
            "load data path is empty",
        ),
        (
            "load data infile '/a' into table load_data_test fields defined null by 'a' optionally enclosed",
            "requires FIELDS ENCLOSED BY",
        ),
        (
            "load data infile '/a' into table load_data_test lines terminated by ''",
            "LINES TERMINATED BY is empty",
        ),
        (
            "load data infile '/a' into table load_data_test fields enclosed by 'a' terminated by 'a'",
            "must not prefix each other",
        ),
        (
            "load data local infile '/tmp/nonexistence.csv' into table load_data_test fields terminated by ''",
            "FIELDS TERMINATED BY is empty",
        ),
        (
            "load data local infile '/tmp/nonexistence.csv' into table load_data_test fields terminated by '' enclosed by ''",
            "FIELDS TERMINATED BY is empty",
        ),
    ] {
        let error = match build_controller(sql) {
            Ok(mut controller) => {
                controller.Close();
                panic!("expected rejection: {sql}");
            }
            Err(error) => error,
        };
        assert!(error.contains(expected), "{sql}: {error}");
    }
    for (clause, expected, optional) in [
        ("", vec![r"\N"], false),
        ("fields enclosed by ''", vec![r"\N"], false),
        ("fields defined null by 'a'", vec!["a", r"\N"], false),
        (
            "fields enclosed by 'b' defined null by 'a' optionally enclosed",
            vec!["a", r"\N"],
            true,
        ),
        ("fields enclosed by 'b'", vec!["NULL", r"\N"], false),
        ("fields enclosed by 'b' escaped by ''", vec!["NULL"], false),
    ] {
        let sql = format!("load data local infile '/a' into table load_data_test {clause}");
        let mut controller =
            build_controller(&sql).unwrap_or_else(|error| panic!("{sql}: {error}"));
        assert_eq!(controller.Plan.FieldNullDef, expected, "{sql}");
        assert_eq!(controller.Plan.NullValueOptEnclosed, optional, "{sql}");
        assert_eq!(controller.InsertColumns.len(), 2);
        controller.Close();
    }
    for sql in [
        "load data local infile '/a' format 'sql file' into table load_data_test",
        "load data local infile '/a' into table load_data_test fields terminated by 'a'",
        "load data local infile '/a' format 'delimited data' into table load_data_test fields terminated by 'a'",
    ] {
        let mut controller = build_controller(sql).unwrap_or_else(|error| panic!("{sql}: {error}"));
        controller.Close();
    }
}
