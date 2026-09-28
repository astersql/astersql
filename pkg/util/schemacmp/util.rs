// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// MySQL 整数与 BLOB 类型编号的三态比较辅助。
//
// 对应 Go `util.go`。整数类型中 `TypeInt24` 的编号与语义序不一致；BLOB 中
// `TypeBlob` 同理。比较结果为 -1/0/1，供字段类型格的 `Compare` 使用。

use crate::mysql;

// compareMySQLIntegerType compares two MySQL integer types,
// return -1 if a < b, 0 if a == b and 1 if a > b.
// compareMySQLIntegerType 对应 Go 中的同名非导出函数。
// Go 的 byte 在这里机械映射为 u8，返回值保留 -1/0/1 的三态比较结果。
pub fn compareMySQLIntegerType(a: u8, b: u8) -> i32 {
    // 类型编号完全相同时直接返回 0，对应 Go 代码最前面的快速路径。
    if a == b {
        return 0;
    }

    // TypeTiny(1) < TypeShort(2) < TypeInt24(9) < TypeLong(3) < TypeLonglong(8)
    // MySQL 的 TypeInt24 编号为 9，但排序语义位于 TypeShort 和 TypeLong 之间，
    // 因此不能只按字节编号比较，必须先处理 TypeInt24 的特殊分支。
    if a == mysql::TypeInt24 {
        // a 是 TypeInt24 时，只有 b 为 TypeTiny/TypeShort 才认为 a 更大。
        if b <= mysql::TypeShort {
            return 1;
        }
        return -1;
    } else if b == mysql::TypeInt24 {
        // b 是 TypeInt24 时反向处理，保持 Go switch 中第二个 case 的比较方向。
        if a <= mysql::TypeShort {
            return -1;
        }
        return 1;
    } else if a < b {
        // 其它整数类型可退回到原始编号顺序比较。
        return -1;
    }
    // Go switch 的 default 分支：既不相等，也不是 a < b，则 a > b。
    1
}

// compareMySQLBlobType compares two MySQL blob types,
// return -1 if a < b, 0 if a == b and 1 if a > b.
// compareMySQLBlobType 对应 Go 中的同名非导出函数。
// 该函数只比较 MySQL blob 类型编号，不分配资源，也没有 IO 或外部副作用。
pub fn compareMySQLBlobType(a: u8, b: u8) -> i32 {
    // 类型编号完全相同时直接返回 0，对应 Go 代码最前面的快速路径。
    if a == b {
        return 0;
    }

    // TypeTinyBlob(0xf9, 249) < TypeBlob(0xfc, 252) < TypeMediumBlob(0xfa, 250) < TypeLongBlob(0xfb, 251)
    // TypeBlob 的编号 0xfc 大于 Medium/Long Blob，但排序语义在 TinyBlob 和 MediumBlob 之间，
    // 因此先处理 TypeBlob，避免普通字节比较产生错误顺序。
    if a == mysql::TypeBlob {
        // a 是 TypeBlob 时，只比 TypeTinyBlob 大；相对 Medium/Long Blob 都更小。
        if b == mysql::TypeTinyBlob {
            return 1;
        }
        return -1;
    } else if b == mysql::TypeBlob {
        // b 是 TypeBlob 时反向处理，保持 Go switch 中第二个 case 的比较方向。
        if a == mysql::TypeTinyBlob {
            return -1;
        }
        return 1;
    } else if a < b {
        // 除 TypeBlob 外，其它 blob 类型按原始编号顺序即可得到 Go 里的相对顺序。
        return -1;
    }
    // Go switch 的 default 分支：既不相等，也不是 a < b，则 a > b。
    1
}
