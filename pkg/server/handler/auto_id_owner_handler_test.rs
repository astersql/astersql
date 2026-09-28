// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// Auto ID Owner handler 单元测试。
//
// 用假检查器与响应记录器覆盖：非 owner、owner、健康检查失败三条路径，
// 对齐 Go 侧 `TestAutoIDOwnerHandler` 的断言语义。

use crate::auto_id_owner_handler::{
    AutoIDOwnerChecker, AutoIDOwnerResponse, NewAutoIDOwnerHandler, autoIDOwnerStatus,
};

/// fakeAutoIDOwnerChecker 对应 Go 测试中的假检查器，两个布尔字段分别控制健康状态和 owner 状态。
struct FakeAutoIDOwnerChecker {
    healthy: bool,
    owner: bool,
}

impl AutoIDOwnerChecker for FakeAutoIDOwnerChecker {
    fn Health(&self) -> bool {
        self.healthy
    }

    fn IsAutoIDOwner(&self) -> bool {
        self.owner
    }
}

impl AutoIDOwnerChecker for &FakeAutoIDOwnerChecker {
    fn Health(&self) -> bool {
        (*self).Health()
    }

    fn IsAutoIDOwner(&self) -> bool {
        (*self).IsAutoIDOwner()
    }
}

/// 记录写出的 HTTP 状态码与 body，便于断言。
#[derive(Default)]
struct Recorder {
    code: u16,
    body: String,
}

impl AutoIDOwnerResponse for Recorder {
    type Error = ();

    fn write_status(&mut self, status: u16) -> Result<(), Self::Error> {
        self.code = status;
        self.body.clear();
        Ok(())
    }

    fn write_owner_status(&mut self, status: autoIDOwnerStatus) -> Result<(), Self::Error> {
        self.code = 200;
        self.body = format!(
            r#"{{"is_owner": {}}}"#,
            if status.IsOwner { "true" } else { "false" }
        );
        Ok(())
    }
}

/// TestAutoIDOwnerHandler 对应 Go 测试：依次覆盖非 owner、owner、健康检查失败三种分支。
#[test]
fn TestAutoIDOwnerHandler() {
    let mut checker = FakeAutoIDOwnerChecker {
        healthy: true,
        owner: false,
    };

    // 健康且非 owner：期望 200 与 is_owner=false。
    {
        let h = NewAutoIDOwnerHandler(&checker);
        let mut recorder = Recorder::default();
        h.ServeHTTP(&mut recorder).expect("healthy non-owner");
        assert_eq!(200, recorder.code);
        assert_eq!(r#"{"is_owner": false}"#, recorder.body);
    }

    // 切换为 owner 后再请求。
    checker.owner = true;
    {
        let h = NewAutoIDOwnerHandler(&checker);
        let mut recorder = Recorder::default();
        h.ServeHTTP(&mut recorder).expect("healthy owner");
        assert_eq!(200, recorder.code);
        assert_eq!(r#"{"is_owner": true}"#, recorder.body);
    }

    // 健康检查失败时 Go handler 不再写 JSON body，只检查状态码为 500。
    checker.healthy = false;
    {
        let h = NewAutoIDOwnerHandler(&checker);
        let mut recorder = Recorder::default();
        h.ServeHTTP(&mut recorder).expect("unhealthy status");
        assert_eq!(500, recorder.code);
        assert!(recorder.body.is_empty());
    }
}
