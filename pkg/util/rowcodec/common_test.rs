// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// `RemoveKeyspacePrefix` 单测：验证 next-gen / classic / StandAlone 下的 keyspace 前缀剥离。
//
// Keyspace 是多租户隔离前缀；next-gen 键前 4 字节为 keyspace，classic 无此前缀。
// `StandAloneTiDB` 为真时强制剥离，与内核类型分支共同覆盖 Go 同名测试路径。

#![allow(non_snake_case)]

use super::{intest, kerneltype, kv, rowcodec};
use std::sync::atomic::Ordering;

/// 覆盖 classic / 测试态 / 生产态与 StandAlone 开关下 `RemoveKeyspacePrefix` 的行为。
#[test]
fn TestRemoveKeyspacePrefix() {
    // next-gen 样例键：前 4 字节为 keyspace，其后为 classic 形态。
    let next_gen_key =
        hex::decode("78000001748000fffffffffffe5F728000000000000002").expect("valid hex fixture");
    let classic_key = &next_gen_key[4..];

    struct RestoreFlags {
        in_test: bool,
        standalone: bool,
    }
    impl Drop for RestoreFlags {
        fn drop(&mut self) {
            intest::InTest.store(self.in_test, Ordering::SeqCst);
            kv::StandAloneTiDB.store(self.standalone, Ordering::SeqCst);
        }
    }
    let _restore = RestoreFlags {
        in_test: intest::InTest.load(Ordering::SeqCst),
        standalone: kv::StandAloneTiDB.load(Ordering::SeqCst),
    };

    // Go exercises both runtime InTest values and both standalone values.
    for in_test in [true, false] {
        intest::InTest.store(in_test, Ordering::SeqCst);
        for standalone in [false, true] {
            kv::StandAloneTiDB.store(standalone, Ordering::SeqCst);
            let expected = if kerneltype::IsClassic() || (!in_test && !standalone) {
                next_gen_key.as_slice()
            } else {
                &next_gen_key[4..]
            };
            assert_eq!(
                expected,
                rowcodec::RemoveKeyspacePrefix(&next_gen_key),
                "InTest={in_test}, StandAloneTiDB={standalone}"
            );
            assert_eq!(classic_key, rowcodec::RemoveKeyspacePrefix(classic_key));
        }
    }
}
