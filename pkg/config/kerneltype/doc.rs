// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 本文件由 pkg/config/kerneltype/doc.go 迁移而来，保留 Go 包文档结构。

// kerneltype 模块的文档说明文件：描述 TiDB 的“内核类型”（Kernel Type）概念。
//
// 内核类型分为两种：
// - Classic（经典内核）：基于 share-nothing（无共享）架构，数据由 TiKV
//   （分布式事务型键值存储组件）在各节点本地独立管理。
// - NextGen（下一代/云原生内核）：数据面采用共享存储架构，通常以 Amazon S3
//   之类的对象存储作为数据的唯一权威来源（single source of truth）。
//
// 内核类型在编译期通过构建开关（Go 中为 build tag，Rust 中为 feature/cfg）
// 确定，因此不同内核类型会生成不同的二进制文件，不支持混合部署。
// 本文件仅承载文档，不包含任何代码；实际的模块接线在 lib.rs 中完成。

// Package kerneltype provides the kernel type of TiDB.
//
// We have 2 types of kernel: Classic and NextGen.
//
// TiDB Classic Kernel refers to the original architecture used during the early
// development stages of TiDB. It utilizes a share-nothing architecture, primarily
// implemented through TiKV, TiDB's distributed transactional key-value storage
// component. In this setup, each TiKV instance independently manages its own
// local storage and computing resources, eliminating dependencies on shared resources.
//
// This architecture provides advantages in terms of horizontal scalability, fault
// tolerance, and simplified management. Each node independently handles data,
// allowing for easy addition or removal of nodes to adapt to workload changes.
// However, unlike the next-generation (cloud native) kernel, it does not leverage
// shared storage solutions like S3 and requires managing local storage directly
// on each node.
//
// The TiDB Next-gen (Cloud Native) Kernel is a new architecture specifically
// designed for cloud-native infrastructure. It adopts a shared-storage architecture
// for the data plane, typically using object storage solutions like Amazon S3 as
// the single source of truth for data storage.
//
// methods inside this package is chosen based on compile time build tag for
// different kernel type. So different kernel type will have different binary,
// and we don't support deploying components with different kernel types.
//
// Go package docs use build tags in classic.go and nextgen.go.
// Rust 保留文档和这条编译期选择语义，模块接线位于 lib.rs。
