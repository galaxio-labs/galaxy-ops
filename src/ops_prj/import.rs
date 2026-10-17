use super::prelude::*;

use crate::{
    ops_prj::install::{SystemPackageInstaller, fetch_and_prepare},
    system::operator::SysOperator,
};
use orion_variate::addr::Address;

use crate::{
    artifact::types::convert_addr,
    const_vars::{MERGED_VARS_YML, SYS_VALUE_FILE, SYS_VARS_YML},
    error::MainResult,
    ops_prj::{
        project::OpsProject,
        system::{OpsSystem, OpsTargetSystem, VERSION_PLACEHOLDER},
        upgrade::read_sys_version,
        version::same_version,
    },
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

        // 2. 取包、铺进项目根、初始化设置值（不改 conf）
        let target = self.install_sys_files(path, accessor, up_opt).await?;

        // 3. 登记到 ops-prj.yml：若导入地址里带版本，自动改写成 `{version}` 模板，
        //    这样后续 `prj sync` / `prj upgrade latest|patch|...` 无需手工改 addr。
        let installed_version = read_sys_version(&target.installation_path);
        let (addr, version) = versioned_import_addr(path, addr, &installed_version);
        let mut ops_sys = OpsSystem::new(target.spec().define().clone(), addr);
        if let Some(version) = version {
            ops_sys.set_version(version);
        }
        self.import_ops_sys(ops_sys);
        self.save()?;
        Ok(())
    }

    /// 取包 → 铺进 `<项目根>/<系统名>/`（重建 `values/` 符号链接）→ 初始化设置值。
    ///
    /// **不**改 `ops-prj.yml`。供 `prj import`（导入新系统）与 `prj sync`（补齐缺失系统）复用，
    /// 避免“取包 + 落盘”逻辑两处各写一遍而分叉。
    pub(crate) async fn install_sys_files(
        &self,
        path: &str,
        accessor: Accessor,
        up_opt: &DownloadOptions,
    ) -> MainResult<OpsTargetSystem> {
        let sys_src = fetch_and_prepare(self.paths().clone(), path, accessor, up_opt).await?;
        let installer = SystemPackageInstaller::new(self.paths().clone());
        let target = installer.install_system_package(&sys_src)?;
        let sys_operator = SysOperator::load(&target.installation_path)?;
        sys_operator.init_setting_value()?;
        Ok(target)
    }

    /// （系统名, addr 反推路径）清单。抽出来避开各处重复，也绕开迭代借用与 `&mut self` 冲突。
    ///
    /// addr 经 `{version}` 模板解析（`OpsSystem::resolved_addr`）：含占位但没写 `version` 时
    /// 在这里就报错，而不是拿字面量 `{version}` 去下载。
    pub(crate) fn sys_targets(&self) -> MainResult<Vec<(String, String)>> {
        let mut out = Vec::new();
        for sys in self.conf().sys_models() {
            out.push((sys.sys().name().clone(), sys.resolved_addr()?));
        }
        Ok(out)
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

/// 把具体包地址里的版本改写成 `{version}` 模板；返回模板地址与应声明的版本。
///
/// 优先从地址的最后两段识别版本（文件名 / release tag 目录）；识别不到时保留原地址。
/// git 地址不含版本坐标（版本在 tag 里），不做改写。
fn versioned_import_addr(
    input: &str,
    addr: Address,
    installed_version: &str,
) -> (Address, Option<String>) {
    let Some(path_version) = detect_version(input).or_else(|| {
        let installed = installed_version.trim();
        (!installed.is_empty() && input.contains(installed)).then(|| installed.to_string())
    }) else {
        return (addr, None);
    };

    let Some(template) = templatize_version(input, &path_version) else {
        return (addr, None);
    };
    let Ok(template_addr) = convert_addr(&template) else {
        return (addr, None);
    };

    let installed = installed_version.trim();
    if !installed.is_empty() && !same_version(installed, &path_version) {
        println!(
            "提示：包内版本 {installed} 与导入地址版本 {path_version} 不一致；\n  \
             已按地址版本生成 {VERSION_PLACEHOLDER} 模板（version: {path_version}）。"
        );
    }
    (template_addr, Some(path_version))
}

/// 从地址最后两段里识别 `1.2.3[-pre][+build]`（可带 `v` 前缀）。
fn detect_version(input: &str) -> Option<String> {
    let re =
        regex::Regex::new(r"(?i)v?(\d+(?:\.\d+){2,}(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?)")
            .ok()?;
    for segment in input.split('/').rev().take(2) {
        for caps in re.captures_iter(segment) {
            let Some(version) = caps.get(1) else {
                continue;
            };
            let version = strip_archive_suffix(version.as_str());
            if is_version_coordinate(version) {
                return Some(version.to_string());
            }
        }
    }
    None
}

/// semver（3 段）或日期版本（4+ 段纯数字）都可作为版本坐标。
fn is_version_coordinate(version: &str) -> bool {
    if semver::Version::parse(version).is_ok() {
        return true;
    }
    let core = version.split(['-', '+']).next().unwrap_or(version);
    let parts: Vec<&str> = core.split('.').collect();
    parts.len() >= 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
}

/// 正则的预发布段会把 `.tar.gz` 一起吃进去；按常见制品后缀反复剥离。
fn strip_archive_suffix(mut version: &str) -> &str {
    loop {
        let before = version;
        for suffix in [
            ".tar.gz", ".tgz", ".tar", ".gz", ".zip", ".sig", ".asc", ".sha256",
        ] {
            if let Some(stripped) = version.strip_suffix(suffix) {
                version = stripped;
                break;
            }
        }
        if version == before {
            break;
        }
    }
    version.trim_end_matches(['.', '-'])
}

/// 把版本字符串替换成 `{version}`：只动文件名与紧邻的 release tag 目录，
/// 避免误改 owner/repo 等与版本无关的路径段。
fn templatize_version(input: &str, version: &str) -> Option<String> {
    let version = version.trim();
    if version.is_empty() || input.contains(VERSION_PLACEHOLDER) {
        return None;
    }
    if input.starts_with("git@") || input.ends_with(".git") {
        return None;
    }

    let mut segments: Vec<String> = input.split('/').map(str::to_string).collect();
    let last = segments.len().checked_sub(1)?;
    let mut changed = false;

    if segments[last].contains(version) {
        segments[last] = segments[last].replace(version, VERSION_PLACEHOLDER);
        changed = true;
    }

    if last > 0 {
        let parent = &segments[last - 1];
        let without_v = parent
            .strip_prefix('v')
            .or_else(|| parent.strip_prefix('V'))
            .unwrap_or(parent);
        if without_v == version {
            segments[last - 1] = parent.replace(version, VERSION_PLACEHOLDER);
            changed = true;
        }
    }

    changed.then(|| segments.join("/"))
}

/// 把 `Address` 反推回可用于 `convert_addr` / `build_pkg` 的路径字符串。
/// 用于从 `ops-prj.yml` 记录的 addr 反推路径（`OpsSystem::addr_template`）。
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
    fn test_detect_version_from_filename_and_release_tag() {
        assert_eq!(
            detect_version("../../sys-operators/example_sys_x-1.0.0.tar.gz"),
            Some("1.0.0".to_string())
        );
        assert_eq!(
            detect_version(
                "https://github.com/o/r/releases/download/v0.10.0-alpha/s-v0.10.0-alpha.tar.gz"
            ),
            Some("0.10.0-alpha".to_string())
        );
        // 版本只在 release tag 目录里。
        assert_eq!(
            detect_version("https://github.com/o/r/releases/download/v1.2.3/pkg.tar.gz"),
            Some("1.2.3".to_string())
        );
        assert_eq!(
            detect_version("https://example.com/pkg-1.2.3-rc.1+build.5.tar.gz"),
            Some("1.2.3-rc.1+build.5".to_string())
        );
        assert_eq!(
            detect_version("https://example.com/pkg-1.2.3.tgz"),
            Some("1.2.3".to_string())
        );
        assert_eq!(
            detect_version("https://example.com/pkg-1.2.3.zip"),
            Some("1.2.3".to_string())
        );
        // 日期式 4 段版本也不能被截成 3 段。
        assert_eq!(
            detect_version("https://example.com/pkg-2026.10.17.1.tar.gz"),
            Some("2026.10.17.1".to_string())
        );
        assert_eq!(detect_version("https://example.com/pkg.tar.gz"), None);
    }

    #[test]
    fn test_templatize_version_touches_filename_and_tag_dir_only() {
        assert_eq!(
            templatize_version("../pkgs/s-1.0.0.tar.gz", "1.0.0"),
            Some("../pkgs/s-{version}.tar.gz".to_string())
        );
        assert_eq!(
            templatize_version(
                "https://github.com/o/r/releases/download/v1.0.0/s-v1.0.0.tar.gz",
                "1.0.0"
            ),
            Some(
                "https://github.com/o/r/releases/download/v{version}/s-v{version}.tar.gz"
                    .to_string()
            )
        );
        // owner/repo 里即使出现相同版本数字也不改。
        assert_eq!(
            templatize_version(
                "https://github.com/acme-1.0.0/r/releases/download/v2.0.0/pkg.tar.gz",
                "2.0.0"
            ),
            Some(
                "https://github.com/acme-1.0.0/r/releases/download/v{version}/pkg.tar.gz"
                    .to_string()
            )
        );
        assert_eq!(
            templatize_version(
                "https://example.com/pkg-2026.10.17.1.tar.gz",
                "2026.10.17.1"
            ),
            Some("https://example.com/pkg-{version}.tar.gz".to_string())
        );
        assert_eq!(
            templatize_version("https://github.com/o/r.git", "1.0.0"),
            None
        );
        assert_eq!(
            templatize_version("https://example.com/pkg.tar.gz", "1.0.0"),
            None
        );
    }

    #[test]
    fn test_versioned_import_addr_uses_path_version_and_falls_back() {
        let input = "../../sys-operators/example_sys_x-1.0.0.tar.gz";
        let addr = convert_addr(input).unwrap();
        let (addr, version) = versioned_import_addr(input, addr, "1.0.0");
        assert_eq!(
            addr_to_path_string(&addr),
            "../../sys-operators/example_sys_x-{version}.tar.gz"
        );
        assert_eq!(version.as_deref(), Some("1.0.0"));

        // 地址里有版本但包内版本不一致：仍按地址版本生成模板。
        let input = "https://github.com/o/r/releases/download/v1.0.0/pkg.tar.gz";
        let addr = convert_addr(input).unwrap();
        let (addr, version) = versioned_import_addr(input, addr, "0.9.0");
        assert_eq!(
            addr_to_path_string(&addr),
            "https://github.com/o/r/releases/download/v{version}/pkg.tar.gz"
        );
        assert_eq!(version.as_deref(), Some("1.0.0"));

        // 地址里识别不到版本：保留原地址，不写 version。
        let input = "https://example.com/pkg.tar.gz";
        let addr = convert_addr(input).unwrap();
        let (addr, version) = versioned_import_addr(input, addr, "1.0.0");
        assert_eq!(addr_to_path_string(&addr), input);
        assert!(version.is_none());

        // git 地址的版本在 tag 里，不在 URL 中 → 不改写。
        let input = "https://github.com/o/r.git";
        let addr = convert_addr(input).unwrap();
        let (addr, version) = versioned_import_addr(input, addr, "1.0.0");
        assert_eq!(addr_to_path_string(&addr), input);
        assert!(version.is_none());
    }

    #[test]
    fn test_addr_to_path_string() {
        let local = Address::Local(LocalPath::from("/tmp/foo.tar.gz"));
        assert_eq!(addr_to_path_string(&local), "/tmp/foo.tar.gz");

        let git = Address::Git(GitRepository::from("https://github.com/x/y.git"));
        assert_eq!(addr_to_path_string(&git), "https://github.com/x/y.git");

        let http = Address::Http(HttpResource::from("https://x.com/y.tar.gz"));
        assert_eq!(addr_to_path_string(&http), "https://x.com/y.tar.gz");
    }
}
