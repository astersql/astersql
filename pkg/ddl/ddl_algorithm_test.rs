// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

use crate::ddl_algorithm::{
    AlgorithmError, AlgorithmType, AlterAlgorithm, AlterKind, proper_algorithm,
    resolve_alter_algorithm,
};

/// 验证各类 ALTER 操作与请求算法组合下的算法解析结果。
#[test]
fn test_find_alter_algorithm() {
    assert_eq!(
        resolve_alter_algorithm(AlterKind::AddConstraint, AlgorithmType::Default),
        (AlgorithmType::Inplace, None)
    );
    assert_eq!(
        resolve_alter_algorithm(AlterKind::AddConstraint, AlgorithmType::Inplace),
        (AlgorithmType::Inplace, None)
    );
    let requested = AlgorithmType::Copy;
    assert_eq!(
        resolve_alter_algorithm(AlterKind::AddConstraint, requested),
        (
            AlgorithmType::Inplace,
            Some(AlgorithmError {
                requested,
                selected: AlgorithmType::Inplace,
                default_algorithm: AlgorithmType::Inplace,
            })
        )
    );
    assert_eq!(
        resolve_alter_algorithm(AlterKind::AddConstraint, AlgorithmType::Instant),
        (
            AlgorithmType::Default,
            Some(AlgorithmError {
                requested: AlgorithmType::Instant,
                selected: AlgorithmType::Default,
                default_algorithm: AlgorithmType::Inplace,
            })
        )
    );

    for requested in [AlgorithmType::Default, AlgorithmType::Instant] {
        assert_eq!(
            resolve_alter_algorithm(AlterKind::Other, requested),
            (AlgorithmType::Instant, None),
            "requested={requested:?}"
        );
    }
    for requested in [AlgorithmType::Copy, AlgorithmType::Inplace] {
        assert_eq!(
            resolve_alter_algorithm(AlterKind::Other, requested),
            (
                AlgorithmType::Instant,
                Some(AlgorithmError {
                    requested,
                    selected: AlgorithmType::Instant,
                    default_algorithm: AlgorithmType::Instant,
                })
            ),
            "requested={requested:?}"
        );
    }
}

#[test]
fn test_proper_algorithm_preserves_go_order_and_error_contract() {
    let algorithms = AlterAlgorithm {
        supported: vec![AlgorithmType::Instant, AlgorithmType::Copy],
        default_algorithm: AlgorithmType::Instant,
    };

    assert_eq!(
        proper_algorithm(AlgorithmType::Inplace, &algorithms),
        (
            AlgorithmType::Instant,
            Some(AlgorithmError {
                requested: AlgorithmType::Inplace,
                selected: AlgorithmType::Instant,
                default_algorithm: AlgorithmType::Instant,
            })
        )
    );
    assert_eq!(
        proper_algorithm(
            AlgorithmType::Copy,
            &AlterAlgorithm {
                supported: vec![],
                default_algorithm: AlgorithmType::Instant,
            }
        ),
        (
            AlgorithmType::Default,
            Some(AlgorithmError {
                requested: AlgorithmType::Copy,
                selected: AlgorithmType::Default,
                default_algorithm: AlgorithmType::Instant,
            })
        )
    );
}
