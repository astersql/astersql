// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// naming 包单元测试：对齐 Go TestScope 的合法/非法名称用例顺序。

use super::*;

// test_scope 对应 Go 的 TestScope，按原顺序覆盖合法字符、非法字符、超长输入、空串和全连字符。
/// 覆盖合法字符、非法字符、超长、空串与全连字符等 Check 场景。
#[test]
fn test_scope() {
    // require.NoError/Error 只验证 Check 的成功或失败，不锁定具体错误文本。
    assert!(Check("789z-_").is_ok());
    assert!(Check("789z-_)").is_err());
    assert!(
        Check("78912345678982u7389217897238917389127893781278937128973812728397281378932179837",)
            .is_err()
    );
    assert!(Check("scope1").is_ok());
    assert!(Check("").is_ok());
    assert!(Check("-----").is_ok());
}
