use super::prelude::*;

use crate::error::MainReason;
use crate::system::spec::{SysDefine, SysModelSpec};

/// `addr` 里的**版本占位符**：`.../wist-gateway-stack-v{version}.tar.gz`。
pub const VERSION_PLACEHOLDER: &str = "{version}";

/// 把模板里的 `{version}` 换成 `version`（不含占位则原样返回）。纯函数，便于单测。
pub fn render_addr(template: &str, version: Option<&str>, sys_name: &str) -> MainResult<String> {
    if !template.contains(VERSION_PLACEHOLDER) {
        return Ok(template.to_string());
    }
    let Some(version) = version.map(str::trim).filter(|v| !v.is_empty()) else {
        return Err(MainReason::logic_detail(format!(
            "系统 {sys_name} 的 addr 含 {VERSION_PLACEHOLDER}，但 ref 没写 `version`。\n  \
             补上 `version: <x.y.z>`，或用 `gops prj upgrade --to <完整地址>`。"
        )));
    };
    Ok(template.replace(VERSION_PLACEHOLDER, version))
}

/// 用**显式版本**渲染地址：要求模板含 `{version}`（否则报错——否则就是拿字面量去下载）。
pub fn render_addr_with(template: &str, version: &str, sys_name: &str) -> MainResult<String> {
    if !template.contains(VERSION_PLACEHOLDER) {
        return Err(MainReason::logic_detail(format!(
            "系统 {sys_name} 的 addr 没有 {VERSION_PLACEHOLDER} 占位，没法按版本 `{version}` 解析。\n  \
             在 addr 里写 {VERSION_PLACEHOLDER}（如 .../x-v{VERSION_PLACEHOLDER}.tar.gz），或用 `--to <完整地址>`。"
        )));
    }
    Ok(template.replace(VERSION_PLACEHOLDER, version.trim()))
}

#[derive(Getters, Clone, Debug, Serialize, Deserialize, PartialEq)]
#[getset(get = "pub")]
pub struct OpsSystem {
    sys: SysDefine,
    /// 声明版本：`addr` 用 `{version}` 占位时按它解析（`prj upgrade --to <ver>` 可临时覆盖）。
    /// 缺省 = 不按版本解析（`addr` 当普通地址）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    addr: Address,
}

impl OpsSystem {
    pub fn new(sys: SysDefine, addr: Address) -> Self {
        Self {
            sys,
            version: None,
            addr,
        }
    }

    /// `addr` 的**原始形态**（可能含 `{version}` 占位）。
    pub fn addr_template(&self) -> String {
        super::import::addr_to_path_string(&self.addr)
    }

    /// 声明版本渲染出的可下载地址。
    /// - `addr` 不含 `{version}`：原样返回（第 1 步的纯地址）。
    /// - 含 `{version}`：按 `self.version` 替换；没写 `version` 就报错。
    pub fn resolved_addr(&self) -> MainResult<String> {
        let raw = self.addr_template();
        render_addr(&raw, self.version.as_deref(), self.sys.name())
    }

    /// 用**显式版本**覆盖声明版本后渲染地址（`prj upgrade --to <ver>` 用）。
    pub fn resolved_addr_with(&self, version: &str) -> MainResult<String> {
        let raw = self.addr_template();
        render_addr_with(&raw, version, self.sys.name())
    }
}

#[derive(Getters, Clone, Debug, Serialize, Deserialize, Default, Deref, DerefMut)]
pub struct OpsTarget {
    sys_models: Vec<OpsSystem>,
}

#[derive(Debug, Clone)]
pub struct OpsTargetSystem {
    pub installation_path: PathBuf,
    pub system_spec: SysModelSpec,
}

impl OpsTargetSystem {
    pub fn new(installation_path: PathBuf, system_spec: SysModelSpec) -> Self {
        Self {
            installation_path,
            system_spec,
        }
    }

    pub fn path(&self) -> &Path {
        &self.installation_path
    }

    pub fn spec(&self) -> &SysModelSpec {
        &self.system_spec
    }

    pub fn system_name(&self) -> &str {
        self.system_spec.define().name()
    }
}

#[cfg(test)]
mod tests {
    use super::{render_addr, render_addr_with};
    use crate::{const_vars::SYS_VALUE_FILE, ops_prj::project::OpsProject};
    use orion_variate::tools::test_init;
    use tempfile::TempDir;

    #[test]
    fn test_process_system_vars_symlink_already_exists() {
        test_init();
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Create test paths
        let vars_path = root.join("sys/vars.yml");
        let value_path = root.join("values/test");
        let _value_file = root.join("values/test/").join(SYS_VALUE_FILE);
        let value_link = root.join("test/values");

        // Create necessary directories
        std::fs::create_dir_all(vars_path.parent().unwrap()).unwrap();
        std::fs::create_dir_all(&value_path).unwrap();
        std::fs::create_dir_all(value_link.parent().unwrap()).unwrap();

        // Create a sample vars.yml file
        let vars_content = r#"
system:
  - name: "test_var"
    value: "default_value"
    mutable: true
    desp: "A test variable"
  - name: "immutable_var"
    value: "immutable_value"
    mutable: false
    desp: "An immutable variable"
"#;
        std::fs::write(&vars_path, vars_content).unwrap();

        // Create existing value file
        let value_file = root.join("values/test").join(SYS_VALUE_FILE);
        std::fs::write(&value_file, "test_key: test_value").unwrap();

        // Pre-create the symlink
        std::os::unix::fs::symlink(&value_path, &value_link).unwrap();

        // Test function should not fail when symlink already exists
        OpsProject::process_system_vars(&vars_path, &value_path, "test_system", false).unwrap();

        // Verify symlink still exists
        assert!(value_link.exists());
        // Verify symlink points to correct target
        let link_target = std::fs::read_link(&value_link).unwrap();
        assert!(link_target.exists());
    }

    #[test]
    fn render_addr_leaves_plain_address_untouched() {
        assert_eq!(
            render_addr("https://x/y.tar.gz", None, "s").unwrap(),
            "https://x/y.tar.gz"
        );
        // 不含占位时，给了版本也不动它。
        assert_eq!(
            render_addr("https://x/y.tar.gz", Some("1.2.3"), "s").unwrap(),
            "https://x/y.tar.gz"
        );
    }

    #[test]
    fn render_addr_substitutes_version() {
        assert_eq!(
            render_addr(
                "https://gh/o/r/releases/download/v{version}/s-v{version}.tar.gz",
                Some(" 0.1.24 "),
                "s"
            )
            .unwrap(),
            "https://gh/o/r/releases/download/v0.1.24/s-v0.1.24.tar.gz"
        );
    }

    #[test]
    fn render_addr_requires_version_when_template_present() {
        let err = render_addr("https://x/v{version}.tar.gz", None, "web-stack").unwrap_err();
        assert!(err.to_string().contains("没写 `version`"), "{err}");
        let blank =
            render_addr("https://x/v{version}.tar.gz", Some("   "), "web-stack").unwrap_err();
        assert!(blank.to_string().contains("{version}"), "{blank}");
    }

    #[test]
    fn render_addr_with_rejects_template_without_placeholder() {
        let err = render_addr_with("https://x/y.tar.gz", "0.1.24", "web-stack").unwrap_err();
        assert!(err.to_string().contains("没有 {version} 占位"), "{err}");
        assert_eq!(
            render_addr_with("https://x/v{version}.tar.gz", " 0.1.24 ", "s").unwrap(),
            "https://x/v0.1.24.tar.gz"
        );
    }
}
