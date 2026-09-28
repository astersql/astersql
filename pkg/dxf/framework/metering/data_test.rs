// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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
// `Data` 相等性与增量计算（`cal_meter_data_item`）的单元测试。

// limitations under the License.

use crate::data::{
    CLUSTER_READ_BYTES_FIELD, CLUSTER_WRITE_BYTES_FIELD, Data, DataValues, GET_REQUESTS_FIELD,
    MeterItem, MeterValue, OBJ_STORE_READ_BYTES_FIELD, OBJ_STORE_WRITE_BYTES_FIELD,
    PUT_REQUESTS_FIELD,
};

/// 覆盖 equals：各计数器相等/不等，以及 task_id 不同但计数值相同仍视为相等。
#[test]
fn test_data_equals() {
    let cases = [
        (
            Data::new(
                1,
                "",
                "",
                DataValues {
                    get_requests: 1,
                    ..Default::default()
                },
            ),
            Data::new(
                1,
                "",
                "",
                DataValues {
                    get_requests: 1,
                    ..Default::default()
                },
            ),
            true,
        ),
        (
            Data::new(
                1,
                "",
                "",
                DataValues {
                    get_requests: 1,
                    ..Default::default()
                },
            ),
            Data::new(
                1,
                "",
                "",
                DataValues {
                    get_requests: 2,
                    ..Default::default()
                },
            ),
            false,
        ),
        (
            Data::new(
                1,
                "",
                "",
                DataValues {
                    put_requests: 1,
                    ..Default::default()
                },
            ),
            Data::new(
                1,
                "",
                "",
                DataValues {
                    put_requests: 1,
                    ..Default::default()
                },
            ),
            true,
        ),
        (
            Data::new(
                1,
                "",
                "",
                DataValues {
                    put_requests: 1,
                    ..Default::default()
                },
            ),
            Data::new(
                1,
                "",
                "",
                DataValues {
                    put_requests: 2,
                    ..Default::default()
                },
            ),
            false,
        ),
        (
            Data::new(
                1,
                "",
                "",
                DataValues {
                    obj_store_read_bytes: 1,
                    ..Default::default()
                },
            ),
            Data::new(
                1,
                "",
                "",
                DataValues {
                    obj_store_read_bytes: 2,
                    ..Default::default()
                },
            ),
            false,
        ),
        (
            Data::new(
                1,
                "",
                "",
                DataValues {
                    obj_store_write_bytes: 1,
                    ..Default::default()
                },
            ),
            Data::new(
                1,
                "",
                "",
                DataValues {
                    obj_store_write_bytes: 2,
                    ..Default::default()
                },
            ),
            false,
        ),
        (
            Data::new(
                1,
                "",
                "",
                DataValues {
                    cluster_read_bytes: 1,
                    ..Default::default()
                },
            ),
            Data::new(
                1,
                "",
                "",
                DataValues {
                    cluster_read_bytes: 2,
                    ..Default::default()
                },
            ),
            false,
        ),
        (
            Data::new(
                1,
                "",
                "",
                DataValues {
                    cluster_write_bytes: 1,
                    ..Default::default()
                },
            ),
            Data::new(
                1,
                "",
                "",
                DataValues {
                    cluster_write_bytes: 2,
                    ..Default::default()
                },
            ),
            false,
        ),
        // Go intentionally compares only the data fields, not taskID.
        (
            Data::new(
                1,
                "",
                "",
                DataValues {
                    get_requests: 1,
                    ..Default::default()
                },
            ),
            Data::new(
                2,
                "",
                "",
                DataValues {
                    get_requests: 1,
                    ..Default::default()
                },
            ),
            true,
        ),
    ];

    for (index, (left, right, expected)) in cases.iter().enumerate() {
        assert_eq!(left.equals(right), *expected, "case {index} failed");
    }
}

/// 为断言补齐 GetBaseMeterItem 中的固定基础字段。
fn base_item(mut item: MeterItem) -> MeterItem {
    item.extend([
        ("version".to_owned(), MeterValue::from("1")),
        ("cluster_id".to_owned(), MeterValue::from("ks")),
        ("source_name".to_owned(), MeterValue::from("dxf")),
        ("task_type".to_owned(), MeterValue::from("tt")),
        ("task_id".to_owned(), MeterValue::from(1_i64)),
    ]);
    item
}

/// 校验相对快照的正增量：全字段差分、仅单字段变化、以及自身对比返回 None。
#[test]
fn test_data_cal_meter_data_item() {
    let current = Data::new(
        1,
        "ks",
        "tt",
        DataValues {
            get_requests: 10,
            put_requests: 20,
            obj_store_read_bytes: 300,
            obj_store_write_bytes: 400,
            cluster_read_bytes: 500,
            cluster_write_bytes: 600,
        },
    );
    assert!(current.cal_meter_data_item(&current).is_none());

    assert_eq!(
        current.cal_meter_data_item(&Data::new(
            0,
            "",
            "",
            DataValues {
                get_requests: 5,
                put_requests: 5,
                obj_store_read_bytes: 100,
                obj_store_write_bytes: 100,
                cluster_read_bytes: 200,
                cluster_write_bytes: 200,
            },
        )),
        Some(base_item(MeterItem::from([
            (GET_REQUESTS_FIELD.to_owned(), MeterValue::from(5_u64)),
            (PUT_REQUESTS_FIELD.to_owned(), MeterValue::from(15_u64)),
            (
                OBJ_STORE_READ_BYTES_FIELD.to_owned(),
                MeterValue::from(200_u64),
            ),
            (
                OBJ_STORE_WRITE_BYTES_FIELD.to_owned(),
                MeterValue::from(300_u64),
            ),
            (
                CLUSTER_READ_BYTES_FIELD.to_owned(),
                MeterValue::from(300_u64),
            ),
            (
                CLUSTER_WRITE_BYTES_FIELD.to_owned(),
                MeterValue::from(400_u64),
            ),
        ])))
    );

    assert_eq!(
        current.cal_meter_data_item(&Data::new(
            0,
            "",
            "",
            DataValues {
                get_requests: 5,
                put_requests: 20,
                obj_store_read_bytes: 300,
                obj_store_write_bytes: 400,
                cluster_read_bytes: 500,
                cluster_write_bytes: 600,
            },
        )),
        Some(base_item(MeterItem::from([(
            GET_REQUESTS_FIELD.to_owned(),
            MeterValue::from(5_u64),
        )])))
    );
}
