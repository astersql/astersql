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

// ConcurrentFileReader（并发文件范围读取器）的单元测试。
//
// 验证从给定 offset 起按缓冲大小切分非重叠区间，多路并行读后仍按文件顺序返回，
// 读到文件末尾后返回 EOF。

/// 一次 read 最多 concurrency 个分片；分片按提交顺序排列且互不重叠。
#[test]
fn canonical_concurrent_reader_returns_ordered_non_overlapping_ranges() {
    use std::sync::Arc;

    use crate::concurrent_reader::ConcurrentFileReader;

    // 数据为 0..19；从 offset=3 起，concurrency=3、每片 4 字节。
    let data = Arc::new((0_u8..19).collect::<Vec<_>>());
    let mut reader = ConcurrentFileReader::new(data, 3, 19, 3, 4).unwrap();
    assert_eq!(
        reader.read().unwrap(),
        vec![vec![3, 4, 5, 6], vec![7, 8, 9, 10], vec![11, 12, 13, 14]]
    );
    // 剩余不足一整批时仍按文件顺序返回最后一片。
    assert_eq!(reader.read().unwrap(), vec![vec![15, 16, 17, 18]]);
    assert!(reader.read().unwrap_err().is_eof());
}

/// Go 的 TestConcurrentRead 随机选择 offset、并发度与缓冲大小；这里用确定性参数矩阵
/// 覆盖同一契约，并把每批分片重新拼接，确认不会遗漏、重复或乱序。
#[test]
fn concurrent_reader_reassembles_every_go_parameter_shape() {
    use std::sync::Arc;

    use crate::concurrent_reader::ConcurrentFileReader;

    let data = Arc::new((0_u8..=255).collect::<Vec<_>>());
    for offset in [0, 1, 127, 255] {
        for concurrency in 1..=4 {
            for read_buffer_size in [1, 2, 31, 99, 100] {
                let mut reader = ConcurrentFileReader::new(
                    Arc::clone(&data),
                    offset,
                    data.len(),
                    concurrency,
                    read_buffer_size,
                )
                .unwrap();
                let mut got = Vec::new();
                loop {
                    match reader.read() {
                        Ok(chunks) => {
                            assert!(!chunks.is_empty());
                            assert!(chunks.len() <= concurrency);
                            got.extend(chunks.into_iter().flatten());
                        }
                        Err(error) if error.is_eof() => break,
                        Err(error) => panic!("unexpected read error: {error}"),
                    }
                }
                assert_eq!(got, data[offset..]);
                assert_eq!(reader.offset(), data.len());
            }
        }
    }
}
