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

// 角色谓词（IsEnabled / IsMaster / IsGCV2Worker 等）的单元测试。
//
// 使用最小 `StubManager` 实现验证：Manager 为空时全部为 false；
// 专职角色下仅对应谓词为 true。

use astersql_extworkload::{
    IsAutoAnalyzeWorker, IsEnabled, IsGCV2Worker, IsMaster, IsTTLTaskWorker, Manager, ManagerError,
    config, context, keyspacepb,
};

/// 仅实现 Role/Close/Meta 的最小 Manager 桩；其余方法不可达。
struct StubManager {
    /// 预设的外部工作负载角色。
    role: config::ExternalWorkloadRole,
    abort_count: usize,
    abort_error: bool,
}

impl Manager for StubManager {
    fn Close(&mut self) -> Result<(), ManagerError> {
        Ok(())
    }
    fn Role(&self) -> config::ExternalWorkloadRole {
        self.role.clone()
    }
    fn Meta(&self) -> Option<&keyspacepb::KeyspaceMeta> {
        None
    }
    fn InitializeGCV2(
        &mut self,
        _: &context::Context,
        _: std::time::Duration,
    ) -> Result<(), ManagerError> {
        unreachable!()
    }
    fn AbortGCV2(&mut self, _: &context::Context) -> Result<(), ManagerError> {
        self.abort_count += 1;
        if self.abort_error {
            Err(std::io::Error::other("abort failed").into())
        } else {
            Ok(())
        }
    }
    fn RegisterGCV2(
        &mut self,
        _: &context::Context,
        _: u64,
        _: std::time::Duration,
    ) -> Result<(), ManagerError> {
        unreachable!()
    }
    fn RecycleGCV2(&mut self, _: &context::Context, _: u64) -> Result<(), ManagerError> {
        unreachable!()
    }
    fn UpdateGCLifeTime(
        &mut self,
        _: &context::Context,
        _: std::time::Duration,
    ) -> Result<(), ManagerError> {
        unreachable!()
    }
    fn RegisterTTLTask(
        &mut self,
        _: &context::Context,
        _: i64,
        _: bool,
    ) -> Result<(), ManagerError> {
        unreachable!()
    }
    fn DeleteTTLTableInfo(&mut self, _: &context::Context, _: i64) -> Result<(), ManagerError> {
        unreachable!()
    }
    fn RecycleTTLTask(&mut self, _: &context::Context, _: u64) -> Result<(), ManagerError> {
        unreachable!()
    }
    fn UpdateTTLJobEnable(&mut self, _: &context::Context, _: bool) -> Result<(), ManagerError> {
        unreachable!()
    }
    fn RegisterAutoAnalyze(&mut self, _: &context::Context, _: u64) -> Result<(), ManagerError> {
        unreachable!()
    }
    fn RecycleAutoAnalyze(&mut self, _: &context::Context, _: u64) -> Result<(), ManagerError> {
        unreachable!()
    }
}

/// Manager 为 None 时，所有角色谓词应返回 false。
#[test]
fn test_role_predicates_when_disabled() {
    assert!(!IsEnabled(None));
    assert!(!IsMaster(None));
    assert!(!IsGCV2Worker(None));
    assert!(!IsTTLTaskWorker(None));
    assert!(!IsAutoAnalyzeWorker(None));
}

/// 专职角色矩阵：每个角色仅使自身谓词为 true。
#[test]
fn test_role_predicates_dedicated() {
    // (角色, 对应谓词) 四元组，交叉断言仅对角线为 true。
    let cases: [(
        config::ExternalWorkloadRole,
        fn(Option<&dyn Manager>) -> bool,
    ); 4] = [
        (config::RoleMaster.to_owned(), IsMaster),
        (config::RoleGCV2Worker.to_owned(), IsGCV2Worker),
        (config::RoleTTLTaskWorker.to_owned(), IsTTLTaskWorker),
        (
            config::RoleAutoAnalyzeWorker.to_owned(),
            IsAutoAnalyzeWorker,
        ),
    ];

    for (role, _) in &cases {
        let manager = StubManager {
            role: role.clone(),
            abort_count: 0,
            abort_error: false,
        };
        for (other_role, predicate) in &cases {
            assert_eq!(
                other_role == role,
                predicate(Some(&manager)),
                "{other_role} predicate result for role {role}"
            );
        }
    }
}

#[test]
fn test_abort_gcv2_for_upgrade_role_and_error() {
    let ctx = context::Background();
    assert!(!crate::AbortGCV2ForUpgrade(&ctx, None).unwrap());
    for (role, should_terminate, calls) in [
        (config::RoleMaster, false, 0),
        (config::RoleGCV2Worker, true, 1),
    ] {
        let mut manager = StubManager {
            role: role.into(),
            abort_count: 0,
            abort_error: false,
        };
        assert_eq!(
            should_terminate,
            crate::AbortGCV2ForUpgrade(&ctx, Some(&mut manager)).unwrap()
        );
        assert_eq!(calls, manager.abort_count);
    }
    let mut manager = StubManager {
        role: config::RoleGCV2Worker.into(),
        abort_count: 0,
        abort_error: true,
    };
    let err = crate::AbortGCV2ForUpgrade(&ctx, Some(&mut manager)).unwrap_err();
    assert_eq!("abort failed", err.to_string());
    assert!(err.downcast_ref::<std::io::Error>().is_some());
    assert_eq!(1, manager.abort_count);
}
