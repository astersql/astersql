// Copyright 2023 PingCAP, Inc.
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

// Aster 迁移补充单测：对齐 Go 用例的编解码序、切分键与检测行为。
//
// 覆盖 InternalKey 往返、`gen_split_key` 边界、明确重复组结果，以及构造失败不挂死。
// Go 映射：internal_test.go::TestInternalKey（全部 11 组输入）、
// worker_test.go::TestGenSplitKey（全部 6 组边界）、detector_test.go 的
// result/collector、TestDetector/verifyResults、TestDetectorFail。
// alpha/beta 是额外的确定性结果断言；完整并发输入场景由 detector_test.rs 覆盖。
// Collector 克隆回调切片、End 转移并清空状态，Mutex 对应 Go 结果 channel 的同步。

use std::io;
use std::sync::{Arc, Mutex};

use super::lightning::log::log::L;
use super::util::extsort::disk_sorter::{DiskSorterOptions, open_disk_sorter};
use super::util::extsort::external_sorter::{Error, ExternalSorter};
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

use super::{
    DetectOptions, Handler, HandlerConstructor, InternalKey, compare_internal_key,
    decode_internal_key, encode_internal_key, gen_split_key, new_detector,
};

#[derive(Clone, Debug, Eq, PartialEq)]
/// 收集到的重复组：用户键与全部来源 key_id。
struct DuplicateGroup {
    key: Vec<u8>,
    key_ids: Vec<Vec<u8>>,
}

/// 线程安全地将完成的重复组写入共享列表。
struct Collector {
    current: DuplicateGroup,
    output: Arc<Mutex<Vec<DuplicateGroup>>>,
}

impl Handler for Collector {
    fn begin(&mut self, key: &[u8]) -> Result<(), Error> {
        self.current.key = key.to_vec();
        Ok(())
    }

    fn append(&mut self, key_id: &[u8]) -> Result<(), Error> {
        self.current.key_ids.push(key_id.to_vec());
        Ok(())
    }

    fn end(&mut self) -> Result<(), Error> {
        let completed = std::mem::replace(
            &mut self.current,
            DuplicateGroup {
                key: Vec::new(),
                key_ids: Vec::new(),
            },
        );
        self.output.lock().unwrap().push(completed);
        Ok(())
    }

    fn close(&mut self) -> Result<(), Error> {
        Ok(())
    }
}

#[test]
/// 编解码往返且逻辑比较序与编码字节序一致（对齐 Go）。
fn internal_key_round_trip_preserves_go_ordering() {
    let inputs = vec![
        InternalKey::new(vec![], vec![]),
        InternalKey::new(vec![], vec![1, 2, 3, 4]),
        InternalKey::new(vec![0], vec![2, 3, 4, 5]),
        InternalKey::new(vec![0, 1], vec![3, 4, 5, 6]),
        InternalKey::new(vec![0, 1, 2], vec![4, 5, 6, 7]),
        InternalKey::new(vec![0, 1, 2, 3], vec![5, 6, 7, 8]),
        InternalKey::new(vec![0, 1, 2, 3, 4], vec![6, 7, 8, 9]),
        InternalKey::new(vec![0, 1, 2, 3, 4, 5], vec![7, 8, 9, 10]),
        InternalKey::new(vec![0, 1, 2, 3, 4, 5, 6], vec![8, 9, 10, 11]),
        InternalKey::new(vec![0, 1, 2, 3, 4, 5, 6, 7], vec![9, 10, 11, 12]),
        InternalKey::new(vec![0, 1, 2, 3, 4, 5, 6, 7, 8], vec![10, 11, 12, 13]),
    ];

    let mut encoded = Vec::new();
    for input in &inputs {
        let mut output = Vec::new();
        encode_internal_key(&mut output, input);
        let mut decoded = InternalKey::default();
        decode_internal_key(&output, &mut decoded).unwrap();
        assert_eq!(&decoded, input);
        encoded.push(output);
    }

    for i in 0..inputs.len() {
        for j in i + 1..inputs.len() {
            assert_eq!(
                compare_internal_key(&inputs[i], &inputs[j]),
                encoded[i].cmp(&encoded[j]) as i32
            );
        }
    }
}

#[test]
/// `gen_split_key` 在若干边界用例上与 Go 期望一致。
fn split_key_matches_go_cases() {
    let cases: &[(&[u8], &[u8], &[u8])] = &[
        (&[1, 2], &[1, 2], &[1, 2]),
        (&[1, 2], &[1, 2, 3, 4, 5], &[1, 2, 1]),
        (&[1, 2, 3, 4, 5, 6], &[1, 2, 5, 6, 7, 8], &[1, 2, 4]),
        (&[1, 2, 3, 4], &[1, 2, 4, 5], &[1, 2, 3, 0xff]),
        (&[1, 2, 3, 0xff, 4], &[1, 2, 4, 5], &[1, 2, 3, 0xff, 0xff]),
        (
            &[1, 2, 3, 0xff, 0xff],
            &[1, 2, 4, 5],
            &[1, 2, 3, 0xff, 0xff, 0xff],
        ),
    ];
    for (start, end, expected) in cases {
        assert_eq!(gen_split_key(start, end), *expected);
    }
}

#[test]
/// 明确输入下报告 alpha/beta 两组重复，且 key_id 已排序。
fn detector_reports_duplicate_groups_and_sorted_key_ids() {
    let directory = tempdir().unwrap();
    let sorter =
        Arc::new(open_disk_sorter(directory.path(), DiskSorterOptions::default()).unwrap());
    let detector = new_detector(sorter.clone(), L());
    let ctx = CancellationToken::new();

    let input: &[(&[u8], &[u8])] = &[
        (b"beta", b"03"),
        (b"alpha", b"09"),
        (b"beta", b"01"),
        (b"single", b"07"),
        (b"alpha", b"02"),
        (b"beta", b"02"),
    ];
    let mut adder = detector.key_adder(&ctx).unwrap();
    for (key, key_id) in input {
        adder.add(key, key_id).unwrap();
    }
    adder.close().unwrap();

    let output = Arc::new(Mutex::new(Vec::new()));
    let constructor: HandlerConstructor = {
        let output = output.clone();
        Arc::new(move |_| {
            Ok(Box::new(Collector {
                current: DuplicateGroup {
                    key: Vec::new(),
                    key_ids: Vec::new(),
                },
                output: output.clone(),
            }))
        })
    };
    let (count, result) = detector.detect(
        &ctx,
        Some(&mut DetectOptions {
            concurrency: 4,
            handler_constructor: Some(constructor),
        }),
    );
    result.unwrap();

    let mut groups = output.lock().unwrap().clone();
    groups.sort_by(|left, right| left.key.cmp(&right.key));
    assert_eq!(count, 2);
    assert_eq!(
        groups,
        vec![
            DuplicateGroup {
                key: b"alpha".to_vec(),
                key_ids: vec![b"02".to_vec(), b"09".to_vec()],
            },
            DuplicateGroup {
                key: b"beta".to_vec(),
                key_ids: vec![b"01".to_vec(), b"02".to_vec(), b"03".to_vec()],
            },
        ]
    );
    sorter.close_and_cleanup().unwrap();
}

#[test]
/// 对齐 Go TestDetectorFail 的 ErrorIs：保留工厂错误身份且不挂起。
fn detector_returns_handler_constructor_error_without_hanging() {
    let directory = tempdir().unwrap();
    let sorter =
        Arc::new(open_disk_sorter(directory.path(), DiskSorterOptions::default()).unwrap());
    let detector = new_detector(sorter.clone(), L());
    let ctx = CancellationToken::new();
    let mut adder = detector.key_adder(&ctx).unwrap();
    adder.add(b"key", b"key-id").unwrap();
    adder.close().unwrap();

    let expected_error = Arc::new(io::Error::other("mock handler constructor error"));
    let constructor: HandlerConstructor = {
        let expected_error = expected_error.clone();
        Arc::new(move |_| Err(Box::new(expected_error.clone())))
    };
    let error = detector
        .detect(
            &ctx,
            Some(&mut DetectOptions {
                concurrency: 4,
                handler_constructor: Some(constructor),
            }),
        )
        .1
        .unwrap_err();
    let actual_error = error
        .downcast_ref::<Arc<io::Error>>()
        .expect("Detect must preserve the constructor error");
    assert!(Arc::ptr_eq(actual_error, &expected_error));
    sorter.close_and_cleanup().unwrap();
}
