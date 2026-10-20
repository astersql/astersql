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

use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
enum ModeState {
    Uninitialized,
    Disabled,
    Enabled,
}

impl ModeState {
    const fn for_enabled(enabled: bool) -> Self {
        if enabled {
            Self::Enabled
        } else {
            Self::Disabled
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Uninitialized => "uninitialized",
            Self::Disabled => "disabled",
            Self::Enabled => "enabled",
        }
    }

    fn load(value: u8) -> Self {
        match value {
            value if value == Self::Disabled as u8 => Self::Disabled,
            value if value == Self::Enabled as u8 => Self::Enabled,
            _ => Self::Uninitialized,
        }
    }
}

static CURRENT_MODE: AtomicU8 = AtomicU8::new(ModeState::Uninitialized as u8);

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum DiagnosticModeError {
    #[error("DDL operations are not allowed in diagnostic mode")]
    DdlNotAllowed,
    #[error(
        "diagnostic mode is already initialized as {current} and cannot be changed to {desired}"
    )]
    AlreadyInitialized {
        current: &'static str,
        desired: &'static str,
    },
}

/// Error returned when a diagnostic instance is asked to run DDL.
pub static ErrDDLNotAllowed: DiagnosticModeError = DiagnosticModeError::DdlNotAllowed;

/// Sets the process-wide diagnostic mode during startup.
///
/// Repeating the same initialization is allowed. Once initialized, the mode
/// cannot be changed.
pub fn Initialize(enabled: bool) -> Result<(), DiagnosticModeError> {
    let desired = ModeState::for_enabled(enabled);
    let current = ModeState::load(CURRENT_MODE.load(Ordering::SeqCst));
    if current == desired {
        return Ok(());
    }
    if current != ModeState::Uninitialized {
        return Err(DiagnosticModeError::AlreadyInitialized {
            current: current.as_str(),
            desired: desired.as_str(),
        });
    }
    CURRENT_MODE.store(desired as u8, Ordering::SeqCst);
    Ok(())
}

/// Reports whether this process runs in diagnostic mode.
///
/// This returns false before [`Initialize`] is called.
pub fn Enabled() -> bool {
    ModeState::load(CURRENT_MODE.load(Ordering::SeqCst)) == ModeState::Enabled
}

/// Overrides diagnostic mode for a test and returns a function that restores
/// the previous process-wide state.
///
/// Tests using this function must not run in parallel with code that reads or
/// overrides diagnostic mode.
pub fn SetForTest(enabled: bool) -> impl FnOnce() {
    if !cfg!(test) && !astersql_util_intest::InTest.load(Ordering::SeqCst) {
        panic!("diagnosticmode.SetForTest can only be called in tests built with the intest tag");
    }

    let previous = CURRENT_MODE.swap(ModeState::for_enabled(enabled) as u8, Ordering::SeqCst);
    move || CURRENT_MODE.store(previous, Ordering::SeqCst)
}

#[cfg(test)]
pub(crate) fn reset_for_test() -> impl FnOnce() {
    let previous = CURRENT_MODE.swap(ModeState::Uninitialized as u8, Ordering::SeqCst);
    move || CURRENT_MODE.store(previous, Ordering::SeqCst)
}
