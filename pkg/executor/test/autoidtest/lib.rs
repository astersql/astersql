// Copyright 2026 AsterSQL.

// 自增 ID（auto_increment / auto_random）分配器测试 crate 入口。
//
// 对应 Go `pkg/executor/test/autoidtest` 包。
// AutoID：为 AUTO_INCREMENT / AUTO_RANDOM 列分配唯一标识；
// 本 crate 挂载功能用例与包级 TestMain。

#![allow(dead_code)]

/// Go `TestMain` keeps these process-wide settings installed for every test.
/// Rust tests acquire this fixture before touching AutoID state, which also
/// serializes the process-wide mutations and restores the caller's state.
#[cfg(test)]
pub(crate) struct AutoidTestLock(std::sync::Mutex<()>);

#[cfg(test)]
pub(crate) struct AutoidTestGuard<'a> {
    _lock: std::sync::MutexGuard<'a, ()>,
    previous_step: i64,
    restore_config: Option<Box<dyn FnOnce() + 'a>>,
}

#[cfg(test)]
impl AutoidTestLock {
    pub(crate) const fn new() -> Self {
        Self(std::sync::Mutex::new(()))
    }

    pub(crate) fn lock(
        &self,
    ) -> Result<AutoidTestGuard<'_>, std::sync::PoisonError<AutoidTestGuard<'_>>> {
        match self.0.lock() {
            Ok(lock) => Ok(AutoidTestGuard::install(lock)),
            Err(error) => Err(std::sync::PoisonError::new(AutoidTestGuard::install(
                error.into_inner(),
            ))),
        }
    }
}

#[cfg(test)]
impl<'a> AutoidTestGuard<'a> {
    fn install(lock: std::sync::MutexGuard<'a, ()>) -> Self {
        astersql_testkit_testsetup::SetupForCommonTest();
        let previous_step = astersql_meta_autoid::get_step();
        astersql_meta_autoid::set_step(5_000);
        let restore_config = astersql_config::restore_func();
        astersql_config::update_global(|config| {
            config.instance.slow_threshold = 30_000;
            config.tikv_client.async_commit.safe_window = 0;
            config.tikv_client.async_commit.allowed_clock_drift = 0;
            config.experimental.allows_expression_index = true;
        });
        Self {
            _lock: lock,
            previous_step,
            restore_config: Some(Box::new(restore_config)),
        }
    }
}

#[cfg(test)]
impl Drop for AutoidTestGuard<'_> {
    fn drop(&mut self) {
        astersql_meta_autoid::set_step(self.previous_step);
        if let Some(restore_config) = self.restore_config.take() {
            restore_config();
        }
    }
}

/// AutoID failpoints and global configuration are process-wide, so these tests
/// must not overlap.
#[cfg(test)]
pub(crate) static AUTOID_TEST_LOCK: AutoidTestLock = AutoidTestLock::new();

/// auto_increment / auto_random / AUTO_ID_CACHE 等功能与回归测试。
#[cfg(test)]
mod autoid_test;
/// 包级 TestMain：全局配置与测试夹具。
#[cfg(test)]
mod main_test;
