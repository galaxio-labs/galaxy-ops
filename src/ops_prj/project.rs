use super::prelude::*;

use crate::module::depend::DependencySet;
const OPS_PRJ_WORK: &str = include_str!("init/_gal/work.gxl");
const OPS_PRJ_ADM: &str = include_str!("init/_gal/adm.gxl");
pub const OPS_PRJ_FILE: &str = "ops-prj.yml";
pub const PRJ_OPS_TARGET: &str = "ops-systems.yml";

use crate::types::Accessor;

#[derive(Getters, Clone, Debug, MutGetters)]
#[getset(get = "pub")]
pub struct OpsProject {
    conf: ProjectConf,
    project: GxlProject,
    paths: ProjectPath,
    #[getset(get = "pub", get_mut = "pub")]
    ops_target: OpsTarget,
}
impl OpsProject {
    pub fn new(conf: ProjectConf, root_local: PathBuf) -> Self {
        Self {
            conf,
            project: GxlProject::from((OPS_PRJ_WORK, OPS_PRJ_ADM)),
            paths: ProjectPath::new(root_local),
            ops_target: OpsTarget::default(),
        }
    }
    pub fn import_ops_sys(&mut self, ops_sys: OpsSystem) {
        if !self.ops_target.contains(&ops_sys) {
            self.ops_target.push(ops_sys);
        }
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
        let conf = ProjectConf::load(paths.root())?;

        let ops_target = OpsTarget::load_conf(&paths.target_file()).source_conf()?;
        let project = GxlProject::load_from(paths.root()).owe(OpsReason::Load.into())?;
        flag.mark_suc();
        Ok(Self {
            conf,
            project,
            paths,
            ops_target,
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
        orion_conf::ConfigIO::save_conf(&self.ops_target, &self.paths.target_file())
            .source_resource()?;
        orion_conf::ConfigIO::save_conf(&self.conf, &self.paths.conf_file()).source_resource()?;
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
impl OpsProject {
    pub fn make_new(prj_path: &Path, name: &str) -> MainResult<Self> {
        let conf = ProjectConf::new(name, DependencySet::default());
        Ok(OpsProject::new(conf, prj_path.to_path_buf()))
    }
    pub fn for_test(name: &str) -> MainResult<Self> {
        let prj_path = PathBuf::from(OPS_PRJ_ROOT).join(name);
        make_clean_path(&prj_path).source_logic()?;

        let conf = ProjectConf::for_test();
        let proj = OpsProject::new(conf, prj_path);
        Ok(proj)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        const_vars::{SYS_VALUE_FILE, SYS_VARS_YML},
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
        let vars_path = root.join("sys/sys_vars.yml");
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
        let vars_path = root.join("sys").join(SYS_VARS_YML);
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
        let vars_path = root.join("sys").join(SYS_VARS_YML);
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
        let vars_path = root.join("sys").join(SYS_VARS_YML);
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
        let vars_path = root.join("sys").join(SYS_VARS_YML);
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
        let vars_path = root.join("sys/sys_vars.yml");
        let value_path = root.join("values/test");

        // Create directories but no vars.yml file (this should cause an error)
        std::fs::create_dir_all(vars_path.parent().unwrap()).unwrap();
        std::fs::create_dir_all(&value_path).unwrap();

        // Test function should return an error when vars.yml doesn't exist
        let result = OpsProject::process_system_vars(&vars_path, &value_path, "test_system", false);

        // Verify that an error occurred
        assert!(result.is_err());
    }
}
