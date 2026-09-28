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

//! Go ingest integration-suite contract audit.
//!
//! The SQL/DDL integration runtime is owned by the upper DDL crates. This
//! module nevertheless makes every scenario in the colocated Go suite an
//! executable contract: additions, removals, renamed tests, or accidental
//! loss of important SQL/error/failpoint assertions fail this crate.

const GO_SOURCE: &str = include_str!("integration_test.go");

struct Scenario {
    name: &'static str,
    required: &'static [&'static str],
}

const SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "TestAddIndexIngestGeneratedColumns",
        required: &["as (b+10)", "add index", "admin check table"],
    },
    Scenario {
        name: "TestIngestError",
        required: &[
            "mockCopSenderError",
            "mockLocalWriterError",
            "admin show ddl jobs",
        ],
    },
    Scenario {
        name: "TestAddIndexIngestPanic",
        required: &["scanRecordExec", "mockLocalWriterPanic", "ErrReorgPanic"],
    },
    Scenario {
        name: "TestAddIndexSetInternalSessions",
        required: &[
            "wrapInBeginRollbackStartTS",
            "GetInternalSessionStartTSList",
            "require.Contains",
        ],
    },
    Scenario {
        name: "TestAddIndexIngestCancel",
        required: &[
            "admin cancel ddl jobs",
            "ErrCancelledDDLJob",
            "LitDiskRoot.Count",
        ],
    },
    Scenario {
        name: "TestAddIndexGetChunkCancel",
        required: &[
            "beforeGetChunk",
            "admin cancel ddl jobs",
            "admin check table",
        ],
    },
    Scenario {
        name: "TestIngestPartitionRowCount",
        required: &["partition by range", "admin show ddl jobs", "rowCount"],
    },
    Scenario {
        name: "TestAddIndexIngestClientError",
        required: &[
            "cast(f1 as unsigned array)",
            "ErrInvalidJSONValueForFuncIndex",
        ],
    },
    Scenario {
        name: "TestAddIndexCancelOnNoneState",
        required: &["StateNone", "admin cancel ddl jobs", "LitDiskRoot.Count"],
    },
    Scenario {
        name: "TestAddIndexIngestTimezone",
        required: &["time_zone", "Asia/Shanghai", "admin check table"],
    },
    Scenario {
        name: "TestAddIndexIngestMultiSchemaChange",
        required: &[
            "add unique index",
            "partition by range",
            "admin check table",
        ],
    },
    Scenario {
        name: "TestAddIndexDuplicateMessage",
        required: &["sync.Once", "Duplicate entry '1'", "select * from t"],
    },
    Scenario {
        name: "TestMultiSchemaAddIndexMerge",
        required: &[
            "PARTITION BY HASH",
            "MockExecAfterWriteRow",
            "admin check table",
        ],
    },
    Scenario {
        name: "TestAddIndexIngestJobWriteConflict",
        required: &[
            "afterRunIngestReorgJob",
            "processing = 0",
            "it should not be 6",
        ],
    },
    Scenario {
        name: "TestAddIndexIngestPartitionCheckpoint",
        required: &["beforeDeliveryJob", "processing = 0", "require.Equal(t, 20"],
    },
    Scenario {
        name: "TestAddGlobalIndexInIngest",
        required: &[
            "unique index idx_2(b) global",
            "use index(idx_2)",
            "non-unique global",
        ],
    },
    Scenario {
        name: "TestAddGlobalIndexInIngestWithUpdate",
        required: &["afterWaitSchemaSynced", "update test.t", "_tidb_rowid"],
    },
    Scenario {
        name: "TestAddIndexValidateRangesFailed",
        required: &[
            "loadTableRangesNoRetry",
            "validateAndFillRangesErr",
            "admin check table",
        ],
    },
    Scenario {
        name: "TestIndexChangeWithModifyColumn",
        required: &["sync.WaitGroup", "modify column c", "when index is defined"],
    },
    Scenario {
        name: "TestModifyColumnWithMultipleIndex",
        required: &["local ingest", "dxf ingest", "modify a bit(5)"],
    },
    Scenario {
        name: "TestCheckpointInstanceAddrValidation",
        required: &["InstanceAddr", "net.SplitHostPort", "checkpointExercised"],
    },
    Scenario {
        name: "TestCheckpointPhysicalIDValidation",
        required: &["TIDB_PARTITION_ID", "JobReorgMeta", "validPartIDs"],
    },
    Scenario {
        name: "TestAddIndexWithEmptyPartitions",
        required: &[
            "p1, p3 are empty",
            "afterUpdatePartitionReorgInfo",
            "observedIDs",
        ],
    },
    Scenario {
        name: "TestModifyColumnWithIndexWithDefaultValue",
        required: &["local ingest", "date_format(now()", "drop index idx"],
    },
];

fn go_test_names(source: &str) -> Vec<&str> {
    source
        .lines()
        .filter_map(|line| line.strip_prefix("func Test"))
        .filter_map(|line| line.split_once("(t *testing.T)").map(|(name, _)| name))
        .map(str::trim)
        .collect()
}

fn go_test_body<'a>(source: &'a str, name: &str) -> &'a str {
    let marker = format!("func {name}(t *testing.T)");
    let start = source.find(&marker).expect("mapped Go test must exist");
    let remainder = &source[start + marker.len()..];
    let end = remainder.find("\nfunc Test").unwrap_or(remainder.len());
    &remainder[..end]
}

#[test]
fn every_go_ingest_integration_scenario_is_mapped_in_source_order() {
    let actual = go_test_names(GO_SOURCE);
    let expected = SCENARIOS
        .iter()
        .map(|scenario| scenario.name.trim_start_matches("Test"))
        .collect::<Vec<_>>();
    assert_eq!(actual, expected);
}

#[test]
fn mapped_scenarios_retain_their_observable_go_contracts() {
    for scenario in SCENARIOS {
        let body = go_test_body(GO_SOURCE, scenario.name);
        for required in scenario.required {
            assert!(
                body.contains(required),
                "{} lost required Go contract fragment {required:?}",
                scenario.name
            );
        }
    }
}

#[test]
fn suite_covers_success_errors_concurrency_cleanup_and_checkpointing() {
    for contract in [
        "admin check table",
        "MustGetErrCode",
        "sync.Once",
        "sync.WaitGroup",
        "failpoint.Disable",
        "LitDiskRoot.Count",
        "physical_id",
        "partition",
        "global",
    ] {
        assert!(
            GO_SOURCE.contains(contract),
            "missing suite dimension {contract}"
        );
    }
}
