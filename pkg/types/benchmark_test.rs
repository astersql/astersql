// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// `types` 包中按值推断字段类型的基准/压力型单元测试。
//
// 对大量随机整数调用 `DefaultTypeForValue`，校验推断为 UNSIGNED LONGLONG，
// 并检查显示宽度（flen）与十进制位数一致。

use std::any::Any;

use rand::{RngCore, SeedableRng, rngs::StdRng};
use types_field_group::{DefaultTypeForValue, FieldType, mysql};

/// 用全量、模 64k、模 512 三组随机数压测默认类型推断路径。
#[test]
fn benchmark_default_type_for_value() {
    let len_nums = 1_000_000usize;
    let mut nums_full = vec![0_u64; len_nums];
    let mut nums_64k = vec![0_u64; len_nums];
    let mut nums_512 = vec![0_u64; len_nums];
    let mut rng = StdRng::seed_from_u64(1);

    // 固定种子生成三组不同取值范围的样本，覆盖不同 flen。
    for index in 0..len_nums {
        let value = rng.next_u64();
        nums_full[index] = value;
        nums_64k[index] = value % 64_000;
        nums_512[index] = value % 512;
    }

    // 每组独立推断：类型应为 LONGLONG，且带 UNSIGNED 标志。
    for values in [&nums_full, &nums_64k, &nums_512] {
        let mut field_type = FieldType::default();
        for value in values {
            DefaultTypeForValue(
                Some(value as &dyn Any),
                &mut field_type,
                mysql::DefaultCharset,
                mysql::DefaultCollationName,
            );
            assert_eq!(field_type.GetType(), mysql::TypeLonglong);
            assert!(mysql::HasUnsignedFlag(field_type.GetFlag()));
            assert_eq!(field_type.GetFlen(), value.to_string().len() as isize);
        }
    }
}
