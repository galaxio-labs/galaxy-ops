use super::prelude::*;

use crate::module::depend::DependencySet;
const OPS_PRJ_WORK: &str = include_str!("init/_gal/work.gxl");
const OPS_PRJ_ADM: &str = include_str!("init/_gal/adm.gxl");
pub const OPS_PRJ_FILE: &str = "ops-prj.yml";

use crate::types::Accessor;

#[derive(Getters, Clone, Debug)]
#[getset(get = "pub")]
pub struct OpsProject {
    conf: OpsProjectConf,
    project: GxlProject,
    paths: ProjectPath,
}
impl OpsProject {
    pub fn new(conf: OpsProjectConf, root_local: PathBuf) -> Self {
        Self {
            conf,
            project: GxlProject::from((OPS_PRJ_WORK, OPS_PRJ_ADM)),
            paths: ProjectPath::new(root_local),
        }
    }
    pub fn import_ops_sys(&mut self, ops_sys: OpsSystem) {
        self.conf.import_sys(ops_sys);
    }
    pub fn load(root_local: &Path) -> MainResult<Self> {
        let mut flag = auto_exit_log!(
            info!(
                target : "ops-prj",
                "load project from {} success!", root_local.display()
            ),
            error!(
                target : "ops-prj",
                "load project  from {} fail!", root_local.display()
            )
        );

        let paths = ProjectPath::new(root_local);
        let mut conf = OpsProjectConf::load(paths.root())?;

        // 向后兼容：旧的 ops-systems.yml 存在时，合并其 sys_models，下次 save 迁移到单文件
        let target_file = paths.target_file();
        if target_file.exists() {
            let old = OpsTarget::load_conf(&target_file).source_conf()?;
            for sys in old.iter() {
                conf.import_sys(sys.clone());
            }
        }

        let project = GxlProject::load_from(paths.root()).owe(OpsReason::Load.into())?;
        flag.mark_suc();
        Ok(Self {
            conf,
            project,
            paths,
        })
    }
    pub fn save(&self) -> MainResult<()> {
        let mut flag = auto_exit_log!(
            info!(
                target : "workprj",
                "save project to {} success!", self.paths.root().display()
            ),
            error!(
                target : "workprj",
                "save project  to {} fail!", self.paths.root().display()
            )
        );
        orion_conf::ConfigIO::save_conf(&self.conf, &self.paths.conf_file()).source_resource()?;
        // 迁移：删除旧的 ops-systems.yml（已合并进 ops-prj.yml）
        let target_file = self.paths.target_file();
        if target_file.exists() {
            std::fs::remove_file(&target_file).source_resource()?;
        }
        self.project
            .save_to(self.paths.root(), None)
            .source_logic()?;

        workins_init_gitignore(self.paths.root())?;
        flag.mark_suc();
        Ok(())
    }
}

#[async_trait]
impl InsUpdateable<OpsProject> for OpsProject {
    async fn update_local(
        mut self,
        accessor: Accessor,
        path: &Path,
        options: &DownloadOptions,
    ) -> MainResult<Self> {
        self.conf = self.conf.update_local(accessor, path, options).await?;
        self.save()?;
        Ok(self)
    }
}

impl OpsProject {
    pub fn value_path(&self) -> ValuePath {
        self.paths.to_value_path()
    }

    /// 获取项目根路径，保持与原有 API 的兼容性
    pub fn root_local(&self) -> &Path {
        self.paths.root()
    }
}

/// 若 `sys_dir` 是某个运维项目下已导入的系统（其父目录存在 `ops-prj.yml` 且列出该系统名），
/// 返回该项目为该系统维护的值目录 `values/<sys_name>`。
///
/// 用于让 `gops sys localize` / `sys update` 在运维项目内直接使用项目值（客户值），
/// 不必依赖 `<sys>/values` 符号链接是否完整。
pub fn owner_project_value_dir(sys_dir: &Path) -> Option<PathBuf> {
    let sys_name = sys_dir.file_name()?.to_str()?;
    let prj_root = sys_dir.parent()?;
    if !prj_root.join(OPS_PRJ_CONF_FILE).exists() {
        return None;
    }
    let conf = OpsProjectConf::load(prj_root).ok()?;
    if conf.sys_models().iter().any(|s| s.sys().name() == sys_name) {
        Some(prj_root.join("values").join(sys_name))
    } else {
        None
    }
}

impl OpsProject {
    pub fn make_new(prj_path: &Path, name: &str) -> MainResult<Self> {
        let conf = OpsProjectConf::new(name, DependencySet::default());
        Ok(OpsProject::new(conf, prj_path.to_path_buf()))
    }
    pub fn for_test(name: &str) -> MainResult<Self> {
        let prj_path = PathBuf::from(OPS_PRJ_ROOT).join(name);
        make_clean_path(&prj_path).source_logic()?;

        let conf = OpsProjectConf::for_test();
        let proj = OpsProject::new(conf, prj_path);
        Ok(proj)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        const_vars::{MERGED_VARS_YML, SYS_VALUE_FILE},
        ops_prj::project::OpsProject,
    };
    use orion_error::dev::testing::TestAssert;
    use orion_variate::tools::test_init;
    use orion_vars::vars::ValueDict;

    use tempfile::TempDir;

    #[test]
    fn test_process_system_vars_non_interactive() {
        test_init();
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Create test paths
        let vars_path = root.join("sys/merged_vars.yml");
        let value_path = root.join("values/test");
        let value_file = root.join("values/test/").join(SYS_VALUE_FILE);
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

        // DO NOT create value file initially to test variable processing

        // Test function in non-interactive mode with no existing value file
        OpsProject::process_system_vars(&vars_path, &value_path, "test_system", false).assert();

        // The value file should be created by the function
        assert!(value_file.exists());
        // The symlink should be created
    }

    #[test]
    fn test_process_system_vars_no_existing_file() {
        test_init();
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Create test paths
        let vars_path = root.join("sys").join(MERGED_VARS_YML);
        let value_path = root.join("values/test");
        let value_file = root.join("values/test").join(SYS_VALUE_FILE);
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

        // Test function in non-interactive mode
        let result = OpsProject::process_system_vars(&vars_path, &value_path, "test_system", false);

        // Verify the function succeeds
        result.assert();

        // Read and verify the value file was created with default values
        assert!(value_file.exists());
        let updated_vals = ValueDict::load_conf(&value_file).unwrap();
        assert_eq!(
            updated_vals
                .get_case_insensitive("test_var")
                .unwrap()
                .to_string(),
            "default_value"
        );
        assert_eq!(
            updated_vals
                .get_case_insensitive("immutable_var")
                .unwrap()
                .to_string(),
            "immutable_value"
        );
    }

    #[test]
    fn test_process_system_vars_existing_value_file() {
        test_init();
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Create test paths
        let vars_path = root.join("sys").join(MERGED_VARS_YML);
        let value_path = root.join("values/test");
        let value_file = root.join("values/test/").join(SYS_VALUE_FILE);
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

        // Create existing value file with some initial values
        let initial_value_content = r#"
test_var: "existing_value"
immutable_var: "existing_immutable"
"#;
        std::fs::write(&value_file, initial_value_content).unwrap();

        // Test function in non-interactive mode
        let result = OpsProject::process_system_vars(&vars_path, &value_path, "test_system", false);

        // Verify function succeeds
        result.assert();

        // Verify value file still exists and contains expected values
        assert!(value_file.exists());
        let updated_vals = ValueDict::load_conf(&value_file).unwrap();

        // Both mutable and immutable variables should retain their existing values
        // because in non-interactive mode, we use the existing values from value file
        assert_eq!(
            updated_vals.get("test_var").unwrap().to_string(),
            "existing_value"
        );
        assert_eq!(
            updated_vals.get("immutable_var").unwrap().to_string(),
            "existing_immutable"
        );
    }

    #[test]
    fn test_process_system_vars_empty_vars_file() {
        test_init();
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Create test paths
        let vars_path = root.join("sys").join(MERGED_VARS_YML);
        let value_path = root.join("values/test");
        let value_link = root.join("test/values");

        // Create necessary directories
        std::fs::create_dir_all(vars_path.parent().unwrap()).unwrap();
        std::fs::create_dir_all(&value_path).unwrap();
        std::fs::create_dir_all(value_link.parent().unwrap()).unwrap();

        // Create an empty vars.yml file
        std::fs::write(&vars_path, "").unwrap();

        // Test function in non-interactive mode
        let result = OpsProject::process_system_vars(&vars_path, &value_path, "test_system", false);

        // Verify the function succeeds
        result.assert();
    }

    #[test]
    fn test_process_system_vars_all_immutable_vars() {
        test_init();
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Create test paths
        let vars_path = root.join("sys").join(MERGED_VARS_YML);
        let value_path = root.join("values/test");
        let value_file = root.join("values/test/").join(SYS_VALUE_FILE);
        let value_link = root.join("test/values");

        // Create necessary directories
        std::fs::create_dir_all(vars_path.parent().unwrap()).unwrap();
        std::fs::create_dir_all(&value_path).unwrap();
        std::fs::create_dir_all(value_link.parent().unwrap()).unwrap();

        // Create a vars.yml file with only immutable variables
        let vars_content = r#"
system:
  - name: "immutable_var1"
    value: "immutable_value1"
    mutable: false
    desp: "An immutable variable"
  - name: "immutable_var2"
    value: "immutable_value2"
    mutable: false
    desp: "Another immutable variable"
"#;
        std::fs::write(&vars_path, vars_content).unwrap();

        // Test function in non-interactive mode
        let result = OpsProject::process_system_vars(&vars_path, &value_path, "test_system", false);

        // Verify the function succeeds
        result.assert();

        // Verify the value file was created
        assert!(value_file.exists());
    }

    #[test]
    fn test_process_system_vars_error_handling() {
        test_init();
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Create test paths
        let vars_path = root.join("sys/merged_vars.yml");
        let value_path = root.join("values/test");

        // Create directories but no vars.yml file (this should cause an error)
        std::fs::create_dir_all(vars_path.parent().unwrap()).unwrap();
        std::fs::create_dir_all(&value_path).unwrap();

        // Test function should return an error when vars.yml doesn't exist
        let result = OpsProject::process_system_vars(&vars_path, &value_path, "test_system", false);

        // Verify that an error occurred
        assert!(result.is_err());
    }

    #[test]
    fn test_load_legacy_ops_systems_migrates() {
        test_init();
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // 构造旧式项目：ops-prj.yml（无 sys_models）+ ops-systems.yml（有 sys_models）
        std::fs::write(
            root.join("ops-prj.yml"),
            "name: legacy\nwork_envs:\n  dep_root: ''\n  deps: []\n",
        )
        .unwrap();
        std::fs::write(
            root.join("ops-systems.yml"),
            "sys_models:\n- sys:\n    name: web-stack\n    model: arm-mac14-host\n    vender: ''\n  addr:\n    path: ../web-stack-0.1.0.tar.gz\n",
        )
        .unwrap();
        // GxlProject::load_from 需要 _gal/work.gxl
        std::fs::create_dir_all(root.join("_gal")).unwrap();
        std::fs::write(root.join("_gal/work.gxl"), "mod envs {}\nmod main {}\n").unwrap();

        let project = OpsProject::load(root).assert();
        assert_eq!(project.conf().sys_models().len(), 1);

        project.save().assert();
        assert!(!root.join("ops-systems.yml").exists());
        assert!(root.join("ops-prj.yml").exists());
    }

    #[test]
    fn test_owner_project_value_dir() {
        test_init();
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path().join("cust");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("ops-prj.yml"),
            "name: cust\nwork_envs:\n  dep_root: ''\n  deps: []\nsys_models:\n- sys:\n    name: web-stack\n    kind: docker-compose\n    vender: ''\n  addr:\n    url: http://example.com/web-stack.tar.gz\n",
        )
        .unwrap();
        let sys_dir = root.join("web-stack");
        std::fs::create_dir_all(&sys_dir).unwrap();

        // 项目声明的系统 -> 返回项目为它维护的值目录
        assert_eq!(
            owner_project_value_dir(&sys_dir),
            Some(root.join("values").join("web-stack"))
        );

        // 同层存在但项目未声明的系统 -> None
        let other = root.join("other-sys");
        std::fs::create_dir_all(&other).unwrap();
        assert_eq!(owner_project_value_dir(&other), None);

        // 不在任何运维项目内 -> None
        let plain = temp_dir.path().join("plain-sys");
        std::fs::create_dir_all(&plain).unwrap();
        assert_eq!(owner_project_value_dir(&plain), None);
    }
}
