// Copyright 2026 AsterSQL.

// `bootstraptest2` 测试包入口。
//
// 挂接更高版本 bootstrap 升级回归（DDL 表版本、dist task、runaway 等）与 harness 测试。

#![allow(dead_code)]

#[cfg(test)]
mod boot_test;
#[cfg(test)]
mod main_test;
