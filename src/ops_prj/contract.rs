//! 对外契约（`sys-prj.yml: contract`）的导出 / 导入。
//!
//! 契约 = **不可重建的公开现场事实**（域名 / 端点 / 公开证书）；声明在 `sys-prj.yml: contract`
//! （只列文件或目录），内容 site-owned、不在交付包内。见 `docs/design/contract.md`。
//!
//! - `export`：把各系统 `contract:` 列出的内容打成一个归档（日期时间命名），用于交付 / 迁移 / 存档；
//! - `import`：把归档里的契约内容合并回现场（先解到临时再覆盖），**幂等**。

use std::fs::File;
use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::write::GzEncoder;

use crate::const_vars::SYS_PRJ_CONF_FILE_V2;
use crate::error::MainReason;
use crate::ops_prj::backup::{append_entry, collect_matching, prune_prefixed, restore};
use crate::ops_prj::prelude::*;
use crate::ops_prj::project::OpsProject;
use crate::project::expand_process_env;
use crate::system::SysConf;
use crate::system::pack::compile_path_patterns;

/// 一次 `contract export` 的结果。
#[derive(Debug, Clone, Default)]
pub struct ContractExportReport {
    pub archive: PathBuf,
    /// 归档内条目（形如 `<系统名>/<相对路径>`）。
    pub entries: Vec<String>,
    /// 因超过 `contract.keep` 被清掉的旧份。
    pub pruned: Vec<PathBuf>,
}

/// 一次 `contract import` 的结果。
#[derive(Debug, Clone, Default)]
pub struct ContractImportReport {
    /// 合并回现场的顶层条目（系统名）。
    pub entries: Vec<String>,
    pub dry_run: bool,
}

impl OpsProject {
    /// `prj contract export`：把各系统 `sys-prj.yml: contract` 列出的内容打成一个归档。
    ///
    /// 归档名带日期时间：`<项目名>-contract-<YYYYMMDD-HHMMSS>.tar.gz`，落在项目侧
    /// `ops-prj.yml: contract.target`（缺省 = 项目根），并保留最近 `contract.keep` 份；
    /// 条目形如 `<系统名>/<相对路径>`。
    pub fn contract_export(&self) -> MainResult<ContractExportReport> {
        let root = self.paths().root().to_path_buf();
        // 落点 / 份数：项目侧 `ops-prj.yml: contract`；缺省 = 项目根、保留 10 份。
        let (target, keep) = match self.conf().contract() {
            Some(c) => (
                PathBuf::from(expand_process_env(c.target().to_string().as_str())),
                *c.keep(),
            ),
            None => (root.clone(), 10),
        };
        ensure_path(&target).source_resource().with(&target)?;
        let name = self.conf().name();
        let ts = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
        let archive = target.join(format!("{name}-contract-{ts}.tar.gz"));

        let file = File::create(&archive).source_resource().with(&archive)?;
        let encoder = GzEncoder::new(file, Compression::default());
        let mut tar = tar::Builder::new(encoder);

        let mut entries = Vec::new();
        let mut seen: Vec<String> = Vec::new();
        for sys in self.conf().sys_models() {
            let name = sys.sys().name().clone();
            // `sys_models` 可能对同一系统写多条（`prj diagnose` 会报重名）；按名去重，避免重复收。
            if seen.contains(&name) {
                continue;
            }
            seen.push(name.clone());
            let sys_root = root.join(&name);
            if !sys_root.is_dir() {
                println!("跳过 {name}：未导入");
                continue;
            }
            let conf_file = sys_root.join(SYS_PRJ_CONF_FILE_V2);
            if !conf_file.exists() {
                continue;
            }
            let conf = SysConf::load_conf(&conf_file).source_resource()?;
            if conf.contract().is_empty() {
                continue;
            }
            let patterns = compile_path_patterns(conf.contract(), "contract")?;
            for rel in collect_matching(&sys_root, &patterns, &[])? {
                let abs = sys_root.join(&rel);
                let arc = format!("{name}/{}", rel.to_string_lossy());
                append_entry(&mut tar, &abs, Path::new(&arc))?;
                entries.push(arc);
            }
        }
        tar.finish()
            .map_err(|e| MainReason::logic_detail(format!("tar finish failed: {e}")))?;
        let pruned = prune_prefixed(&archive, &format!("{name}-contract-"), keep)?;
        entries.sort();
        Ok(ContractExportReport {
            archive,
            entries,
            pruned,
        })
    }

    /// `prj contract import`：把归档里的契约内容合并回现场，**幂等**（同一归档重复导入结果不变）。
    ///
    /// 复用 `backup::restore`：条目 `<系统名>/<相对路径>` 合并回 `<项目根>/<条目路径>`；
    /// 先解到临时目录再覆盖，中途失败不留半截。
    pub fn contract_import(
        &self,
        archive: &Path,
        dry_run: bool,
    ) -> MainResult<ContractImportReport> {
        let report = restore(self.paths().root(), archive, dry_run)?;
        Ok(ContractImportReport {
            entries: report.entries,
            dry_run: report.dry_run,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orion_variate::tools::test_init;
    use tempfile::TempDir;

    const OPS_PRJ: &str = "name: cust\nwork_envs:\n  dep_root: ''\n  deps: []\nsys_models:\n- sys:\n    name: web-stack\n    kind: docker-compose\n    vender: ''\n  addr:\n    url: http://example.com/web-stack.tar.gz\n";
    const SYS_PRJ: &str =
        "test_envs:\n  dep_root: ''\n  deps: []\ncontract:\n  - configs/gateway/public\n";

    fn write_ops_project(root: &Path) {
        std::fs::create_dir_all(root.join("_gal")).unwrap();
        std::fs::write(root.join("_gal/work.gxl"), "mod envs {}\nmod main {}\n").unwrap();
        std::fs::write(root.join("ops-prj.yml"), OPS_PRJ).unwrap();
    }

    fn write_sys(root: &Path, body: &str, with_contract_file: bool) {
        let sys = root.join("web-stack");
        std::fs::create_dir_all(&sys).unwrap();
        std::fs::write(sys.join("sys-prj.yml"), body).unwrap();
        if with_contract_file {
            std::fs::create_dir_all(sys.join("configs/gateway/public")).unwrap();
            std::fs::write(
                sys.join("configs/gateway/public/contract.yml"),
                "public_base_url: https://gw.example.com\n",
            )
            .unwrap();
        }
        // 非契约内容：不应进归档
        std::fs::write(sys.join("app.txt"), "y").unwrap();
    }

    fn write_project(root: &Path) {
        write_ops_project(root);
        write_sys(root, SYS_PRJ, true);
    }

    #[test]
    fn test_contract_export_import_round_trip() {
        test_init();
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write_project(root);
        let prj = OpsProject::load(root).unwrap();

        let rep = prj.contract_export().unwrap();
        assert!(rep.archive.is_file(), "archive: {}", rep.archive.display());
        assert!(
            rep.entries
                .iter()
                .any(|e| e == "web-stack/configs/gateway/public/contract.yml"),
            "entries: {:?}",
            rep.entries
        );
        // 非契约内容不进归档
        assert!(!rep.entries.iter().any(|e| e.ends_with("app.txt")));

        // 现场丢失后，从归档恢复
        let live = root.join("web-stack/configs/gateway/public/contract.yml");
        std::fs::remove_file(&live).unwrap();
        assert!(!live.exists());

        prj.contract_import(&rep.archive, false).unwrap();
        let body = std::fs::read_to_string(&live).unwrap();
        assert!(body.contains("gw.example.com"), "body: {body}");

        // 幂等：再导入一次结果不变
        prj.contract_import(&rep.archive, false).unwrap();
        assert_eq!(
            std::fs::read_to_string(&live).unwrap(),
            "public_base_url: https://gw.example.com\n"
        );
    }

    #[test]
    fn test_contract_export_empty_when_no_declaration() {
        test_init();
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write_ops_project(root);
        // 系统存在，但没声明 contract
        write_sys(root, "test_envs:\n  dep_root: ''\n  deps: []\n", false);
        let prj = OpsProject::load(root).unwrap();

        let rep = prj.contract_export().unwrap();
        assert!(rep.archive.is_file());
        assert!(rep.entries.is_empty(), "entries: {:?}", rep.entries);
    }

    #[test]
    fn test_contract_export_skips_unimported_system() {
        test_init();
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write_ops_project(root); // 只写项目，不建系统目录
        let prj = OpsProject::load(root).unwrap();

        let rep = prj.contract_export().unwrap();
        assert!(rep.archive.is_file());
        assert!(rep.entries.is_empty());
    }

    #[test]
    fn test_contract_import_dry_run_keeps_site() {
        test_init();
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write_project(root);
        let prj = OpsProject::load(root).unwrap();
        let rep = prj.contract_export().unwrap();

        let live = root.join("web-stack/configs/gateway/public/contract.yml");
        std::fs::remove_file(&live).unwrap();

        prj.contract_import(&rep.archive, true).unwrap();
        assert!(!live.exists(), "dry-run 不应改动现场");
    }

    #[test]
    fn test_contract_import_missing_archive_errors() {
        test_init();
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write_project(root);
        let prj = OpsProject::load(root).unwrap();

        assert!(
            prj.contract_import(&root.join("nope.tar.gz"), false)
                .is_err()
        );
    }

    #[test]
    fn test_contract_export_uses_configured_target() {
        test_init();
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let target = tmp.path().join("contract-out");
        // 项目侧声明 `contract.target`（用绝对路径便于断言）
        let ops = format!(
            "{OPS_PRJ}contract:\n  target: {}\n  keep: 3\n",
            target.display()
        );
        std::fs::create_dir_all(root.join("_gal")).unwrap();
        std::fs::write(root.join("_gal/work.gxl"), "mod envs {}\nmod main {}\n").unwrap();
        std::fs::write(root.join("ops-prj.yml"), ops).unwrap();
        write_sys(root, SYS_PRJ, true);

        let prj = OpsProject::load(root).unwrap();
        let rep = prj.contract_export().unwrap();
        assert!(
            rep.archive.starts_with(&target),
            "应落在配置的 target：{}",
            rep.archive.display()
        );
        assert!(rep.archive.is_file());
    }
}
