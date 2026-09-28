// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// Chunk 包测试入口相关断言：字段宽度估算与定长/变长布局分派。
//
// Rust 无 Go `TestMain`；此处用最小用例锁定 `EstimateTypeWidth`
// 对 BIGINT（定长 8）与 VARCHAR（变长估算 32）的返回值。

/// 断言定长与变长列类型的宽度估算与 Go 一致。
#[test]
fn field_width_dispatch_matches_fixed_and_variable_layouts() {
    use super::{EstimateTypeWidth, mysql, types};
    assert_eq!(
        EstimateTypeWidth(&types::NewFieldType(mysql::TypeLonglong)),
        8
    );
    assert_eq!(
        EstimateTypeWidth(&types::NewFieldType(mysql::TypeVarchar)),
        32
    );
}

/// 跨越磁盘与未刷盘缓存且请求超过逻辑末尾时，应像 Go 一样短读而非 panic。
#[test]
fn reader_with_cache_short_reads_past_cache_end() {
    use super::{NewReaderWithCache, errors, io};

    struct PrefixReader(Vec<u8>);
    impl io::ReaderAt for PrefixReader {
        fn ReadAt(&self, data: &mut [u8], offset: i64) -> Result<usize, errors::Error> {
            let start = usize::try_from(offset).map_err(|_| errors::New("negative offset"))?;
            if start >= self.0.len() {
                return Ok(0);
            }
            let count = data.len().min(self.0.len() - start);
            data[..count].copy_from_slice(&self.0[start..start + count]);
            Ok(count)
        }
    }

    let reader = NewReaderWithCache(PrefixReader(b"disk".to_vec()), b"cache".to_vec(), 4);
    let mut output = [0_u8; 10];
    let count = io::ReaderAt::ReadAt(&reader, &mut output, 0).unwrap();

    assert_eq!(count, 9);
    assert_eq!(&output[..count], b"diskcache");
}
