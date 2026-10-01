use super::prelude::*;

use std::path::PathBuf;

use crate::{
    ops_prj::install::{SystemPackageInstaller, fetch_and_prepare},
    system::{operator::SysOperator, spec::SysModelSpec},
};
use orion_variate::addr::Address;

use crate::{
    artifact::types::convert_addr,
    const_vars::{MERGED_VARS_YML, SYS_PRJ_CONF_FILE_V2, SYS_VALUE_FILE, SYS_VARS_YML},
    error::{MainReason, MainResult},
    ops_prj::{project::OpsProject, system::OpsSystem},
    system::pack::{compile_path_patterns, path_matches},
    types::Accessor,
};

impl OpsProject {
    pub async fn import_sys(
        &mut self,
        accessor: Accessor,
        path: &str,
        up_opt: &DownloadOptions,
    ) -> MainResult<()> {
        // 1. 解析地址
        let addr = convert_addr(path)?;

        // 2. 取包并解到工作区
        let sys_src = fetch_and_prepare(self.paths().clone(), path, accessor, up_opt).await?;

        // 3. 导入到工作目录
        let installer = SystemPackageInstaller::new(self.paths().clone());
        let ops_target_system = installer.install_system_package(&sys_src)?;

        let ops_sys = OpsSystem::new(ops_target_system.spec().define().clone(), addr);
        self.import_ops_sys(ops_sys);
        self.save()?;
        let sys_operator = SysOperator::load(&ops_target_system.installation_path)?;
        sys_operator.init_setting_value()?;
        // 5. 提供系统包的信息， 包组所有组件。
        Ok(())
    }

    /// 补装缺失：按 `ops-prj.yml` 记录的 `sys_models` 重新导入**不在场的**系统。
    ///
    /// **只补、不删** —— 目录已存在的系统一律不动，并在结束时拒绝（点名 `prj update` 这条出路）。
    /// 适用于“误删了某个系统目录，但保留了 values/ + ops-prj.yml”的场景。
    /// 要重建已存在的目录请用 `gops prj rebuild`。
    pub async fn reimport(
        &mut self,
        accessor: Accessor,
        options: &DownloadOptions,
    ) -> MainResult<()> {
        let mut refused: Vec<String> = Vec::new();
        for (name, path) in self.sys_targets() {
            if self.paths().root().join(&name).exists() {
                // 已存在的不动，继续把**缺失的**建起来
                // （否则多系统项目里“恢复一个被删系统”会被其它在场的系统卡死）。
                refused.push(name);
                continue;
            }
            self.import_sys(accessor.clone(), &path, options).await?;
        }
        if !refused.is_empty() {
            return Err(MainReason::logic_detail(format!(
                "以下系统目录已存在，`prj reimport` 只补缺失、不删：{}。\n  \
                 · 非破坏性升级：`gops prj update`\n  \
                 · 重建（会丢包外未声明内容）：`gops prj rebuild`",
                refused.join("、")
            )));
        }
        Ok(())
    }

    /// 重建：目录不在场 → 直接导入；已存在 → 原子重建。
    ///
    /// 重建 = 现场态（`preserve`）先搬走 → 旧目录改名保留 → 铺新内容 → 再搬回；失败回滚。
    /// 它是**唯一**会丢“包外未声明内容”的操作，所以不做成默认，必须显式敲 `prj rebuild`。
    ///
    /// `only` 给定时只处理该系统（名字不在 `ops-prj.yml` 里则报错）；不给则处理全部系统。
    pub async fn rebuild(
        &mut self,
        only: Option<&str>,
        accessor: Accessor,
        options: &DownloadOptions,
    ) -> MainResult<()> {
        let mut targets = self.sys_targets();
        if let Some(name) = only {
            targets.retain(|(n, _)| n.as_str() == name);
            if targets.is_empty() {
                let known: Vec<String> = self.sys_targets().into_iter().map(|(n, _)| n).collect();
                return Err(MainReason::logic_detail(format!(
                    "ops-prj.yml 里没有系统 `{name}`（可用：{}）",
                    known.join("、")
                )));
            }
        }
        for (name, path) in targets {
            if self.paths().root().join(&name).exists() {
                self.rebuild_one(&name, &path, accessor.clone(), options)
                    .await?;
            } else {
                println!("{name} 不在场：直接导入");
                self.import_sys(accessor.clone(), &path, options).await?;
            }
        }
        Ok(())
    }

    /// （系统名, addr 反推路径）清单。抽出来避开各处重复，也绕开迭代借用与 `&mut self` 冲突。
    fn sys_targets(&self) -> Vec<(String, String)> {
        self.conf()
            .sys_models()
            .iter()
            .map(|sys| (sys.sys().name().clone(), addr_to_path_string(sys.addr())))
            .collect()
    }

    /// 重建一个系统：现场态整体搬走搬回，旧目录改名保留，失败回滚。
    async fn rebuild_one(
        &mut self,
        name: &str,
        addr: &str,
        accessor: Accessor,
        options: &DownloadOptions,
    ) -> MainResult<()> {
        let root = self.paths().root().to_path_buf();
        let target = root.join(name);

        // 0) 预检：上次的暂存区还在时**拒续**。
        //    它可能装着「最后一份现场拷贝」（上次回滚没走干净），所以这里不自动清理 ——
        //    下面那句 `make_clean_path` 会把里面的 old_dir / stash 一起删掉。
        let tmp = root.join(".rebuild-tmp");
        if tmp.exists() {
            return Err(MainReason::logic_detail(format!(
                "上次重建的暂存区还在：{}\n  \
                 它可能包含**最后一份现场拷贝**（上次回滚没走干净），所以这里不自动删除。\n  \
                 确认里面不再需要后手工删除它，再重跑 `gops prj rebuild`。",
                tmp.display()
            )));
        }

        // 1) 先取包：取不到就什么都不动（今天的行为是先删再发现取不到）
        let sys_src = fetch_and_prepare(self.paths().clone(), addr, accessor, options).await?;
        let pkg_name = SysModelSpec::load_from(&sys_src.join("sys"))?
            .define()
            .name()
            .clone();
        if pkg_name != name {
            return Err(MainReason::logic_detail(format!(
                "包内系统名（{pkg_name}）与 ops-prj.yml 记录的（{name}）不一致：拒绝重建"
            )));
        }

        // 2) preserve：包内与目标取并集
        let mut preserve =
            crate::ops_prj::update::load_preserve(&sys_src.join(SYS_PRJ_CONF_FILE_V2))?;
        for item in crate::ops_prj::update::load_preserve(&target.join(SYS_PRJ_CONF_FILE_V2))? {
            if !preserve.contains(&item) {
                preserve.push(item);
            }
        }
        let patterns = compile_path_patterns(&preserve, "preserve")?;

        // 3) 预演：三类清单
        let keep_rels: Vec<PathBuf> = crate::ops_prj::update::walk_files(&target)?
            .into_iter()
            .filter(|rel| path_matches(rel, &patterns))
            .collect();
        let undeclared = crate::ops_prj::update::undeclared_top_entries(
            &target, &sys_src, &patterns, &preserve,
        )?;
        println!("重建 {name}（gops prj rebuild）：");
        println!(
            "  · 将被替换：{}（旧目录先改名保留，成功后删除）",
            target.display()
        );
        println!("  · 将原样搬回：{} 项（preserve）", keep_rels.len());
        if !undeclared.is_empty() {
            println!(
                "  · 包外且未声明（**会随重建消失**）：{}",
                undeclared.join("、")
            );
        }
        if preserve.is_empty() {
            println!(
                "  ⚠ `{name}` 未声明 preserve：包外内容（很可能含身份材料）会全部消失；建议先补 sys-prj.yml"
            );
        }

        // 4) 原子重建：preserve 先搬走 → 旧目录改名 → 铺新内容 → 搬回；失败回滚
        let stash = tmp.join(name);
        let old_dir = tmp.join(format!("{name}.old"));
        make_clean_path(&stash).source_resource().with(&stash)?;
        make_clean_path(&old_dir).source_resource().with(&old_dir)?;

        for rel in &keep_rels {
            move_path(&target.join(rel), &stash.join(rel))?;
        }
        std::fs::rename(&target, &old_dir)
            .source_resource()
            .with(&target)?;

        let rebuilt = (|| -> MainResult<()> {
            let installer = SystemPackageInstaller::new(self.paths().clone());
            installer.install_system_package(&sys_src)?;
            for rel in &keep_rels {
                move_path(&stash.join(rel), &target.join(rel))?;
            }
            Ok(())
        })();

        match rebuilt {
            Ok(()) => {
                // 与 `import_sys` 对齐：补齐客户值模板（已存在不覆盖）。
                // 只告警不报错 —— 重建本身已经成功，不该因这一步把结果说成失败。
                match SysOperator::load(&target) {
                    Ok(op) => {
                        if let Err(e) = op.init_setting_value() {
                            println!("  提示：客户值模板未初始化（{e}）；重建本身已完成");
                        }
                    }
                    Err(e) => println!("  提示：重建后无法加载系统（{e}）；重建本身已完成"),
                }
                if let Err(e) = std::fs::remove_dir_all(&tmp) {
                    println!("提示：重建暂存区未删除 {}: {e}", tmp.display());
                }
                println!("重建完成：{name}（现场态已搬回；建议再跑 `gops sys localize`）");
                Ok(())
            }
            Err(e) => {
                // 回滚：删掉可能生成的新目录，把旧目录与现场态放回原位。
                // 回滚本身失败不能静默 —— 否则用户以为回到了动手前，实际没有。
                eprintln!("重建 {name} 失败，正在回滚：{e}");
                let mut rollback_ok = true;
                if let Err(re) = std::fs::remove_dir_all(&target) {
                    // target 可能压根没被建出来；只有确实还在却删不掉才算异常
                    if target.exists() {
                        rollback_ok = false;
                        eprintln!("  回滚告警：没能清理新目录 {}: {re}", target.display());
                    }
                }
                if let Err(re) = std::fs::rename(&old_dir, &target) {
                    rollback_ok = false;
                    eprintln!("  回滚告警：没能恢复原目录 {}: {re}", target.display());
                }
                for rel in &keep_rels {
                    if let Err(re) = move_path(&stash.join(rel), &target.join(rel)) {
                        rollback_ok = false;
                        eprintln!("  回滚告警：现场态 {} 未能搬回: {re}", rel.display());
                    }
                }
                if rollback_ok {
                    if let Err(re) = std::fs::remove_dir_all(&tmp) {
                        println!("提示：重建暂存区未删除 {}: {re}", tmp.display());
                    }
                } else {
                    // 回滚没走干净：**保留**暂存区 —— 它就是最后一份现场拷贝，顺手删掉等于毁掉现场。
                    eprintln!(
                        "  回滚未完成：现场可能停在半途。**保留**暂存区请勿删除：\n    \
                         · 原目录备份：{}\n    \
                         · 现场态暂存：{}",
                        old_dir.display(),
                        stash.display()
                    );
                }
                Err(e)
            }
        }
    }

    pub fn ia_setting_interactive(&self) -> MainResult<()> {
        self.ia_setting(true)
    }

    pub fn process_system_vars(
        vars_path: &Path,
        value_path: &Path,
        system_name: &str,
        interactive: bool,
    ) -> MainResult<()> {
        use dialoguer::{Confirm, Input};

        let value_file = value_path.join(SYS_VALUE_FILE);

        let vars_vec = VarCollection::load_conf(vars_path).source_resource()?;
        let mut vals_dict = if value_file.exists() {
            ValueDict::load_conf(&value_file).source_resource()?
        } else {
            ValueDict::default()
        };

        // 通过交互模式设定vars的值
        println!("Setting variables for {system_name}");

        for var in vars_vec.system_vars() {
            if !var.is_mutable() {
                continue;
            }
            let prompt = if let Some(desp) = var.desc() {
                format!("{}\n{desp}", var.name())
            } else {
                var.name().to_string()
            };
            let mut default_value = var.value().clone();
            let value_str = if interactive {
                Input::new()
                    .with_prompt(&prompt)
                    .default(var.value().to_string())
                    .interact_text()
                    .source_data()?
            } else {
                // 非交互模式，如果已有值则保留，否则使用默认值
                if let Some(existing_value) = vals_dict.get(var.name()) {
                    existing_value.to_string()
                } else {
                    var.value().to_string()
                }
            };
            default_value
                .update_from_str(value_str.as_str())
                .source_data()?;
            vals_dict.insert(var.name().to_string(), default_value);
        }

        // 如果用户确认保存更改
        let should_save = if interactive {
            Confirm::new()
                .with_prompt("Do you want to save these changes?")
                .interact()
                .source_data()?
        } else {
            // 非交互模式，自动保存
            true
        };
        if should_save {
            // 保存修改后的vars到文件
            // vars.save_to_file(&vars_path)?; // 假设的方法
            println!("Changes saved to {}", value_file.display());
            orion_conf::ConfigIO::save_conf(&vals_dict, &value_file).source_resource()?;
        }
        Ok(())
    }

    pub fn ia_setting(&self, interactive: bool) -> MainResult<()> {
        for i in self.conf().sys_models().iter() {
            let sys_dir = self.root_local().join(i.sys().name()).join("sys");
            // 兼容旧名：merged_vars.yml 优先，缺失时回退 sys_vars.yml
            let new_vars_path = sys_dir.join(MERGED_VARS_YML);
            let legacy_vars_path = sys_dir.join(SYS_VARS_YML);
            let vars_path = if new_vars_path.exists() {
                new_vars_path
            } else if legacy_vars_path.exists() {
                legacy_vars_path
            } else {
                new_vars_path
            };

            let value_path = self.root_local().join("values").join(i.sys().name());
            ensure_path(&value_path).source_resource()?;

            Self::process_system_vars(&vars_path, &value_path, i.sys().name(), interactive)?;
        }
        Ok(())
    }
}

/// 把 `from` 移到 `to`（父目录按需创建；`to` 已存在先移除）。
/// 用于重建时把现场态搬到暂存区、再搬回：两侧都在项目根下，同一文件系统，`rename` 即可。
fn move_path(from: &Path, to: &Path) -> MainResult<()> {
    if from.symlink_metadata().is_err() {
        return Ok(());
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)
            .source_resource()
            .with(parent)?;
    }
    if std::fs::symlink_metadata(to).is_ok() {
        std::fs::remove_file(to).source_resource().with(to)?;
    }
    std::fs::rename(from, to).source_resource().with(to)?;
    Ok(())
}

/// 把 `Address` 反推回可用于 `convert_addr` / `build_pkg` 的路径字符串。
/// 用于 reimport：从 ops-prj.yml 记录的 addr 重新导入系统。
pub(crate) fn addr_to_path_string(addr: &Address) -> String {
    match addr {
        Address::Local(local) => local.path().clone(),
        Address::Git(git) => git.repo().clone(),
        Address::Http(http) => http.url().clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orion_variate::addr::{GitRepository, HttpResource, LocalPath};

    #[test]
    fn test_addr_to_path_string() {
        let local = Address::Local(LocalPath::from("/tmp/foo.tar.gz"));
        assert_eq!(addr_to_path_string(&local), "/tmp/foo.tar.gz");

        let git = Address::Git(GitRepository::from("https://github.com/x/y.git"));
        assert_eq!(addr_to_path_string(&git), "https://github.com/x/y.git");

        let http = Address::Http(HttpResource::from("https://x.com/y.tar.gz"));
        assert_eq!(addr_to_path_string(&http), "https://x.com/y.tar.gz");
    }

    #[test]
    fn test_move_path_moves_overwrites_and_skips_missing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let from = tmp.path().join("a/keep.pem");
        std::fs::create_dir_all(from.parent().unwrap()).unwrap();
        std::fs::write(&from, "secret").unwrap();
        let to = tmp.path().join("b/keep.pem");
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::write(&to, "old").unwrap();

        move_path(&from, &to).unwrap();
        assert!(!from.exists());
        assert_eq!(std::fs::read_to_string(&to).unwrap(), "secret");

        // 源不存在 → 静默 Ok（重建时 preserve 项可能已被删）
        move_path(&tmp.path().join("nope"), &to).unwrap();
        assert_eq!(std::fs::read_to_string(&to).unwrap(), "secret");
    }

    /// `reimport` 默认不删：目录已存在时拒绝，且**不取包、不动现场**。
    #[tokio::test]
    async fn test_reimport_refuses_existing_without_fetch() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("prj");
        std::fs::create_dir_all(root.join("_gal")).unwrap();
        std::fs::write(root.join("_gal/work.gxl"), "mod envs {}\nmod main {}\n").unwrap();
        // addr 指向不存在的包：若真去取包就会失败，从而区分“拒绝”与“取包失败”
        std::fs::write(
            root.join("ops-prj.yml"),
            "name: cust\nwork_envs:\n  dep_root: ''\n  deps: []\nsys_models:\n- sys:\n    name: web-stack\n    kind: docker-compose\n    vender: ''\n  addr:\n    path: /nonexistent/web-stack-0.1.0.tar.gz\n",
        )
        .unwrap();
        let site = root.join("web-stack");
        std::fs::create_dir_all(&site).unwrap();
        std::fs::write(site.join("identity.pem"), "keep me").unwrap();

        let mut prj = OpsProject::load(&root).unwrap();
        let accessor = crate::accessor::accessor_for_test();
        let opts = DownloadOptions::from((0usize, ValueDict::default()));

        let err = prj.reimport(accessor, &opts).await.unwrap_err();
        assert!(
            err.detail()
                .as_deref()
                .is_some_and(|d| d.contains("已存在")),
            "detail={:?}",
            err.detail()
        );
        // 现场未被触碰
        assert_eq!(
            std::fs::read_to_string(site.join("identity.pem")).unwrap(),
            "keep me"
        );
    }

    /// `prj rebuild <未知系统>`：直接报错、不取包。
    #[tokio::test]
    async fn test_rebuild_unknown_system_errors_without_fetch() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("prj");
        std::fs::create_dir_all(root.join("_gal")).unwrap();
        std::fs::write(root.join("_gal/work.gxl"), "mod envs {}\nmod main {}\n").unwrap();
        std::fs::write(
            root.join("ops-prj.yml"),
            "name: cust\nwork_envs:\n  dep_root: ''\n  deps: []\nsys_models:\n- sys:\n    name: web-stack\n    kind: docker-compose\n    vender: ''\n  addr:\n    path: /nonexistent/web-stack-0.1.0.tar.gz\n",
        )
        .unwrap();

        let mut prj = OpsProject::load(&root).unwrap();
        let accessor = crate::accessor::accessor_for_test();
        let opts = DownloadOptions::from((0usize, ValueDict::default()));

        let err = prj
            .rebuild(Some("nope"), accessor, &opts)
            .await
            .unwrap_err();
        let detail = err.detail().as_deref().unwrap_or_default();
        assert!(detail.contains("没有系统 `nope`"), "detail={detail}");
        assert!(detail.contains("web-stack"), "应列出可用系统：{detail}");
    }

    /// 上次回滚没走干净（`.rebuild-tmp` 还在）时：**拒续**，不顺手删掉最后一份现场拷贝。
    #[tokio::test]
    async fn test_rebuild_refuses_when_stash_leftover_exists() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("prj");
        std::fs::create_dir_all(root.join("_gal")).unwrap();
        std::fs::write(root.join("_gal/work.gxl"), "mod envs {}\nmod main {}\n").unwrap();
        std::fs::write(
            root.join("ops-prj.yml"),
            "name: cust\nwork_envs:\n  dep_root: ''\n  deps: []\nsys_models:\n- sys:\n    name: web-stack\n    kind: docker-compose\n    vender: ''\n  addr:\n    path: /nonexistent/web-stack-0.1.0.tar.gz\n",
        )
        .unwrap();
        let site = root.join("web-stack");
        std::fs::create_dir_all(&site).unwrap();
        // 模拟上次回滚失败后留下的暂存区
        let leftover = root.join(".rebuild-tmp/web-stack.old");
        std::fs::create_dir_all(&leftover).unwrap();

        let mut prj = OpsProject::load(&root).unwrap();
        let accessor = crate::accessor::accessor_for_test();
        let opts = DownloadOptions::from((0usize, ValueDict::default()));

        let err = prj.rebuild(None, accessor, &opts).await.unwrap_err();
        let detail = err.detail().as_deref().unwrap_or_default();
        assert!(detail.contains("暂存区还在"), "detail={detail}");
        // 暂存区与现场都原样保留（没被顺手删）
        assert!(leftover.exists());
        assert!(site.exists());
    }
}
