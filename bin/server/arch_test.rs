//! 架构边界测试：解析 workspace Cargo.toml 验证依赖方向规则。
//!
//! 零外部依赖，无需数据库。规则见 `AGENTS.md` / `docs/ai/conventions.md`。
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )
)]

use std::collections::HashSet;
use std::fs::read_dir;
use std::fs::read_to_string;
use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn workspace_members() -> Vec<PathBuf> {
    let root = workspace_root();
    let content = read_to_string(root.join("Cargo.toml")).unwrap();
    let doc: toml::Value = toml::from_str(&content).unwrap();
    doc["workspace"]["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|member| root.join(member.as_str().unwrap()))
        .collect()
}

fn inherits_workspace_lints(crate_dir: &Path) -> bool {
    let cargo_toml = crate_dir.join("Cargo.toml");
    let content = read_to_string(&cargo_toml).unwrap();
    let doc: toml::Value = toml::from_str(&content).unwrap();
    doc.get("lints")
        .and_then(|lints| lints.get("workspace"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

fn feature_dir_names() -> Vec<String> {
    let mut names = Vec::new();
    let features_dir = workspace_root().join("features");
    for entry in read_dir(&features_dir).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            names.push(entry.file_name().to_string_lossy().to_string());
        }
    }
    names.sort();
    names
}

fn contract_crates() -> Vec<(String, HashSet<String>)> {
    feature_dir_names()
        .into_iter()
        .filter(|n| n.ends_with("_contract"))
        .map(|n| {
            let deps = parse_cargo_deps(workspace_root().join("features").join(&n));
            (n, deps)
        })
        .collect()
}

fn feature_runtime_crates() -> Vec<(String, HashSet<String>)> {
    feature_dir_names()
        .into_iter()
        .filter(|n| !n.ends_with("_contract"))
        .map(|n| {
            let deps = parse_cargo_deps(workspace_root().join("features").join(&n));
            (n, deps)
        })
        .collect()
}

fn parse_cargo_deps<P: Into<PathBuf>>(crate_dir: P) -> HashSet<String> {
    let cargo_toml = crate_dir.into().join("Cargo.toml");
    let content = match read_to_string(&cargo_toml) {
        Ok(content) => content,
        Err(_) => return HashSet::new(),
    };
    let mut deps = HashSet::new();

    let doc: toml::Value = toml::from_str(&content).unwrap();
    if let Some(dep_table) = doc.get("dependencies").and_then(|v| v.as_table()) {
        for (name, _) in dep_table {
            deps.insert(name.to_owned());
        }
    }
    deps
}

fn infra_crates() -> Vec<(String, HashSet<String>)> {
    let infra_dir = workspace_root().join("infra");
    let mut names = Vec::new();
    for entry in read_dir(&infra_dir).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            let name = entry.file_name().to_string_lossy().to_string();
            names.push((name.clone(), parse_cargo_deps(infra_dir.join(&name))));
        }
    }
    names.sort_by(|a, b| a.0.cmp(&b.0));
    names
}

fn kit_crates() -> Vec<(String, HashSet<String>)> {
    let crates_dir = workspace_root().join("crates");
    let mut names = Vec::new();
    for entry in read_dir(&crates_dir).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            let name = entry.file_name().to_string_lossy().to_string();
            names.push((name.clone(), parse_cargo_deps(crates_dir.join(&name))));
        }
    }
    names.sort_by(|a, b| a.0.cmp(&b.0));
    names
}

#[test]
fn workspace_members_inherit_workspace_lints() {
    for member in workspace_members() {
        assert!(
            inherits_workspace_lints(&member),
            "workspace member `{}` must set `[lints] workspace = true` in Cargo.toml",
            member.display()
        );
    }
}

#[test]
fn contract_crates_should_not_depend_on_feature_runtime_crates() {
    let runtime_names: HashSet<String> = feature_runtime_crates()
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    for (contract, deps) in contract_crates() {
        let violations: Vec<_> = deps.intersection(&runtime_names).collect();
        assert!(
            violations.is_empty(),
            "contract `{contract}` depends on runtime crate(s): {violations:?}"
        );
    }
}

#[test]
fn contract_crates_should_not_depend_on_other_contract_crates() {
    let contract_names: HashSet<String> = contract_crates()
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    for (contract, deps) in contract_crates() {
        let violations: Vec<_> = deps
            .iter()
            .filter(|d| *d != "shared_contract" && contract_names.contains(*d))
            .collect();
        assert!(
            violations.is_empty(),
            "contract `{contract}` depends on contract crate(s): {violations:?}"
        );
    }
}

#[test]
fn contract_crates_should_not_depend_on_infra() {
    let infra_names: HashSet<String> = infra_crates().into_iter().map(|(n, _)| n).collect();
    for (contract, deps) in contract_crates() {
        let violations: Vec<_> = deps.intersection(&infra_names).collect();
        assert!(
            violations.is_empty(),
            "contract `{contract}` depends on infra crate(s): {violations:?}"
        );
    }
}

#[test]
fn contract_crates_should_not_depend_on_framework_or_db() {
    // 契约保持纯：OpenAPI / HTTP / DB 适配放使用方 DTO 或独立适配层，勿硬依赖。
    // （即使 optional/feature-gate 也不要写进 contract 的 [dependencies]。）
    let forbidden = ["utoipa", "axum", "sqlx", "validify"];
    for (contract, deps) in contract_crates() {
        let violations: Vec<_> = forbidden
            .iter()
            .filter(|name| deps.contains(**name))
            .collect();
        assert!(
            violations.is_empty(),
            "contract `{contract}` must not depend on {violations:?}; keep values pure + serde"
        );
    }
}

#[test]
fn kit_crates_should_not_depend_on_modrun_or_axum() {
    let forbidden = ["modrun", "axum"];
    for (name, deps) in kit_crates() {
        let violations: Vec<_> = forbidden
            .iter()
            .filter(|dep| deps.contains(**dep))
            .collect();
        assert!(
            violations.is_empty(),
            "crates/`{name}` must not depend on {violations:?} (framework-agnostic kits only)"
        );
    }
}

#[test]
fn infra_crates_should_not_depend_on_feature_runtime_crates() {
    let runtime_names: HashSet<String> = feature_runtime_crates()
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    for (name, deps) in infra_crates() {
        let violations: Vec<_> = deps.intersection(&runtime_names).collect();
        assert!(
            violations.is_empty(),
            "infra `{name}` depends on feature runtime crate(s): {violations:?}"
        );
    }
}

#[test]
fn feature_crates_should_not_depend_on_other_feature_runtime_crates() {
    let all_runtime: Vec<(String, HashSet<String>)> = feature_runtime_crates();
    let runtime_names: HashSet<_> = all_runtime.iter().map(|(n, _)| n.clone()).collect();
    for (crate_name, deps) in &all_runtime {
        let violations: Vec<_> = deps
            .iter()
            .filter(|d| *d != crate_name && runtime_names.contains(*d))
            .collect();
        assert!(
            violations.is_empty(),
            "feature `{crate_name}` depends on runtime crate(s): {violations:?}"
        );
    }
}

#[test]
fn kit_crates_should_not_depend_on_features_or_infra() {
    let forbidden: HashSet<String> = feature_runtime_crates()
        .into_iter()
        .map(|(name, _)| name)
        .chain(contract_crates().into_iter().map(|(name, _)| name))
        .chain(infra_crates().into_iter().map(|(n, _)| n))
        .collect();
    for (name, deps) in kit_crates() {
        let violations: Vec<_> = deps.intersection(&forbidden).collect();
        assert!(
            violations.is_empty(),
            "crates/`{name}` depends on feature/infra crate(s): {violations:?}"
        );
    }
}

#[test]
fn feature_package_names_match_directories() {
    let workspace_root = workspace_root();
    for dir_name in feature_dir_names() {
        let cargo_path = workspace_root
            .join("features")
            .join(&dir_name)
            .join("Cargo.toml");
        let content = read_to_string(&cargo_path).unwrap();
        let doc: toml::Value = toml::from_str(&content).unwrap();
        let pkg_name = doc["package"]["name"].as_str().unwrap();
        assert_eq!(
            pkg_name, dir_name,
            "directory `{dir_name}` does not match package name `{pkg_name}`"
        );
    }
}

/// 生产 default（blob-cos+otlp）下 `shared_contract` 依赖图快照；与 `just tree-server` 同源。
/// 失败 = 契约沾框架/DB，或额外 crate 依赖了 shared_contract（增量编译回潮）。
/// 有意变更：`just tree-server-bless` 后提交 golden。
#[test]
fn server_shared_contract_tree_matches_golden() {
    let script = workspace_root().join("scripts/check-server-tree.sh");
    let status = Command::new("bash")
        .arg(&script)
        .current_dir(workspace_root())
        .status()
        .unwrap_or_else(|e| panic!("failed to spawn {}: {e}", script.display()));
    assert!(
        status.success(),
        "shared_contract cargo-tree golden mismatch（见 scripts/check-server-tree.sh / just tree-server-bless）"
    );
}
