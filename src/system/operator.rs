use super::prelude::*;

use crate::const_vars::SYS_MODLE_DEF_YML;
use crate::module::ModelSTD;
use crate::module::depend::DependencySet;
use crate::system::spec::SysDefine;
use crate::types::ValuePath;
use crate::workflow::prj::GxlProject;

use super::init::{SYS_PRJ_ADM, SYS_PRJ_WORK, sys_init_docker_compose, sys_init_gitignore};
use super::{
    conf::{SysConf, SysKind},
    path::SysOperatorPath,
};
use orion_infra::path::{ensure_path, make_clean_path};
use orion_variate::update::DownloadOptions;
use orion_vars::vars::{VarCollection, VarToValue, find_project_define_base};

#[derive(Getters, Clone, Debug)]
#[getset(get = "pub")]
pub struct SysOperator {
    conf: SysConf,
    sys_spec: SysModelSpec,
    project: GxlProject,
    paths: SysOperatorPath,
}

impl SysOperator {
    pub fn new(spec: SysModelSpec, local_res: DependencySet, root_local: PathBuf) -> Self {
        let conf = SysConf::new(local_res);
        //let mut val_dict = ValueDict::default();
        //val_dict.insert("TEST_WORK_ROOT", ValueType::from("/home/galaxy"));
        Self {
            conf,
            sys_spec: spec,
            project: GxlProject::from((SYS_PRJ_WORK, SYS_PRJ_ADM, "define")),
            paths: SysOperatorPath::new(root_local),
            //val_dict,
        }
    }
    pub fn load(root_local: &Path) -> MainResult<Self> {
        let mut ctx = OperationContext::want("load sys-operator")
            .with_auto_log()
            .with_mod_path("sys/prj");

        let paths = SysOperatorPath::new(root_local);

        // 执行配置文件迁移
        paths.migrate_conf_file().with(&ctx).want("migrate conf")?;

        ctx.record("sys-conf", paths.conf_file_v2().display());
        let conf = SysConf::load_conf(&paths.conf_file_v2())
            .source_resource()
            .with(&ctx)?;
        let sys_path = paths.sys_dir();
        ctx.record("sys_path", sys_path.display());
        let sys_spec = SysModelSpec::load_from(&sys_path).with(&ctx)?;

        let project = GxlProject::load_from(paths.root())
            .owe(SysReason::Load.into())
            .with(&ctx)?;
        ensure_path(paths.value_dir()).source_logic().with(&ctx)?;
        ctx.mark_suc();
        Ok(Self {
            conf,
            sys_spec,
            project,
            paths,
        })
    }
    /// 读取系统部署类型，用于 `gops sys` 命令分派。
    ///
    /// 仅解析 `sys/sys_model.yml` 里的 `kind` 字段，
    /// 缺失或解析失败时退回默认的 GXL 类型，保证向后兼容。
    pub fn load_kind(root_local: &Path) -> SysKind {
        let paths = SysOperatorPath::new(root_local);
        let define_path = paths.sys_dir().join(SYS_MODLE_DEF_YML);
        if !define_path.exists() {
            return SysKind::default();
        }
        SysDefine::load_yaml(&define_path)
            .map(|d| *d.kind())
            .unwrap_or_default()
    }
    pub fn with_kind(mut self, kind: SysKind) -> Self {
        let define = self.sys_spec.define().clone().with_kind(kind);
        *self.sys_spec.define_mut() = define;
        self
    }
    pub fn kind(&self) -> SysKind {
        *self.sys_spec.define().kind()
    }
    pub fn is_docker_compose(&self) -> bool {
        self.sys_spec.define().is_docker_compose()
    }
    pub fn save(&self) -> MainResult<()> {
        let mut ctx = OperationContext::want("save sys-prj")
            .with_auto_log()
            .with_mod_path("sys/prj");
        ctx.record("root", self.paths.root().display());
        let conf_file_v2 = self.paths.conf_file_v2();
        if !conf_file_v2.exists() {
            orion_conf::ConfigIO::save_conf(&self.conf, &conf_file_v2)
                .source_resource()
                .with(&ctx)?;
        }
        if self.is_docker_compose() {
            // 纯 docker-compose 系统：精简结构，不生成 _gal / mod_list / workflows / list / values
            self.sys_spec.save_local_minimal(self.paths.root(), "sys")?;
            // version.txt 原本由 GxlProject::save_to 写，纯 compose 系统这里单独补上
            let version_path = self.paths.root().join("version.txt");
            if !version_path.exists() {
                std::fs::write(&version_path, "0.1.0").source_resource()?;
            }
        } else {
            self.sys_spec.save_local(self.paths.root(), "sys")?;
            self.project
                .save_to(self.paths.root(), None)
                .owe(SysReason::Save.into())
                .with(&ctx)?;
            ensure_path(self.paths.value_dir())
                .source_logic()
                .with(&ctx)?;
        }
        sys_init_gitignore(self.paths.root()).with(&ctx)?;
        sys_init_docker_compose(self.paths.root()).with(&ctx)?;
        ctx.mark_suc();
        Ok(())
    }
}

#[async_trait]
impl RefUpdateable<()> for SysOperator {
    async fn update_local(
        &self,
        accessor: Accessor,
        path: &Path,
        options: &DownloadOptions,
    ) -> MainResult<()> {
        self.conf
            .update_local(accessor.clone(), path, options)
            .await?;
        self.sys_spec().update_local(accessor, path, options).await
    }
}

impl SysOperator {
    pub async fn localize(
        &self,
        val_path: SysValuePaths,
        options: LocalizeOptions,
    ) -> MainResult<()> {
        //let value_path = self.value_path().ensure_exist().owe_res()?;

        self.conf.sys_localize((), options.clone()).await?;
        // 导出系统值到 .env（供 docker-compose 等使用 ${VAR} 的工具消费）。
        // 密钥不落盘：compose 里用 ${SEC_xxx} 占位，由 `gops sys start` 运行时从 ~/.galaxy/sec_value.yml 注入子进程环境。
        let env_path = self.paths.root().join(ENV_FILE);
        crate::project::export_env_file(options.evaled_value(), &env_path)?;
        self.sys_spec()
            .sys_localize(val_path, options.clone())
            .await?;
        Ok(())
    }
    pub fn value_path(&self) -> ValuePath {
        self.paths.to_value_path()
    }

    /// 系统解析出的默认值（`sys/merged_vars.yml` 的 `system:` 段）。
    ///
    /// 用作本地化基线：值文件（`values/sys_value.yml` / `values/value.yml`）
    /// 可以只写需要覆盖的项，其余取系统默认值。缺少已解析变量时返回空字典。
    pub fn system_default_values(&self) -> MainResult<ValueDict> {
        let vars_file = self.paths.resolve_merged_vars_file();
        if !vars_file.exists() {
            return Ok(ValueDict::default());
        }
        Ok(VarCollection::load_yaml(&vars_file)
            .source_resource()?
            .system_vars()
            .to_val())
    }

    /// 系统变量是否已解析（`sys/merged_vars.yml`，兼容旧名 `sys_vars.yml`）。
    pub fn has_resolved_vars(&self) -> bool {
        self.paths.resolve_merged_vars_file().exists()
    }

    /// 值文件模板：把可用系统变量以**注释**形式列出（默认值来自 `sys/merged_vars.yml`）。
    ///
    /// 取消注释并改写需要的项即可覆盖；全部保持注释时等价于空覆盖，不会钉住默认值。
    pub fn value_reference(&self) -> MainResult<String> {
        let mut out = format!(
            "# 系统变量参考（默认值来自 {}）。\n\
             # 只取消注释需要覆盖的项并修改数值，其余取默认值。\n",
            self.paths.resolve_merged_vars_file().display()
        );
        let defaults = self.system_default_values()?;
        if defaults.is_empty() {
            return Ok(out);
        }
        let body = serde_yaml::to_string(&defaults).map_err(|e| {
            crate::error::MainReason::logic_detail(format!("序列化变量参考失败: {e}"))
        })?;
        for line in body.lines() {
            if line.trim().is_empty() {
                continue;
            }
            out.push_str("# ");
            out.push_str(line);
            out.push('\n');
        }
        Ok(out)
    }
}

impl SysOperator {
    /// 获取项目根路径，保持与原有 API 的兼容性
    pub fn root_local(&self) -> &Path {
        self.paths.root()
    }

    pub fn make_new(prj_path: &Path, name: &str, model: ModelSTD) -> MainResult<Self> {
        let mod_spec = SysModelSpec::make_new(SysDefine::new(name, model))?;
        let res = DependencySet::default();
        Ok(SysOperator::new(mod_spec, res, prj_path.to_path_buf()))
    }
    /// 纯 docker-compose 系统：无目标型号（型号对 compose 无意义）
    pub fn make_new_docker(prj_path: &Path, name: &str) -> MainResult<Self> {
        let mod_spec = SysModelSpec::make_new(SysDefine::new_without_model(name))?;
        let res = DependencySet::default();
        Ok(SysOperator::new(mod_spec, res, prj_path.to_path_buf()))
    }
    pub fn make_test_prj(name: &str) -> MainResult<Self> {
        let prj_path = PathBuf::from(SYS_MODEL_SPC_ROOT).join(name);
        make_clean_path(&prj_path).source_logic()?;
        let proj = SysOperator::make_new(&prj_path, name, ModelSTD::from_cur_sys())?;
        proj.save()?;
        Ok(proj)
    }
    pub fn init_setting_value(&self) -> MainResult<SysValuePaths> {
        let value_root = SysValuePaths::from(PathBuf::from(self.root_local()))
            .ensure_join(VALUE_DIR)
            .source_resource()?;
        self.init_setting_value_in(value_root)
    }

    /// 与 [`Self::init_setting_value`] 相同，但把值文件写入指定的值目录。
    ///
    /// 运维项目会为每个系统单独维护 `values/<sys_name>`（客户值）；在项目内执行 `gops sys`
    /// 时应把值落到那里，而不是系统自带的 `<sys>/values`（后者在旧版本导入时可能只是包内的副本）。
    ///
    /// 注意：生成的 `values/sys_value.yml` 是**注释模板**（可用变量全部注释），默认不生效；
    /// 用户取消注释需要覆盖的项即可。
    pub fn init_setting_value_in(&self, value_root: SysValuePaths) -> MainResult<SysValuePaths> {
        let value_root = value_root.ensure_root().source_resource()?;
        // 系统变量必须先解析：本地化基线来自 sys/merged_vars.yml
        if !self.has_resolved_vars() {
            return Err(crate::error::MainReason::logic_detail(format!(
                "系统变量未解析：缺少 `{}`。请先在该系统上执行 `gops sys update` 解析变量",
                self.paths.merged_vars_file().display()
            )));
        }
        //let mut all_vars = VarCollection::default();
        for x in self.sys_spec().mod_list().iter() {
            if let Some(mmo) = x.get_target_spec()? {
                let mm_path = value_root.clone().ensure_join(x.name()).source_resource()?;
                //all_vars = all_vars.merge(mmo.vars().clone());
                if !mm_path.mod_value_file().exists() {
                    let mod_vars = mmo.vars().module_vars().to_val();
                    mod_vars
                        .save_yaml(&mm_path.mod_value_file())
                        .source_resource()?;
                }

                //mm.vars()
            }
        }
        let setting_val_path = value_root
            .clone()
            .ensure_join("setting")
            .source_resource()?;
        if !setting_val_path.mod_value_file().exists() {
            let setting_vars = self.sys_spec().setting().vars().module_vars().to_val();
            setting_vars
                .save_yaml(&setting_val_path.mod_value_file())
                .source_resource()?;
        }
        // 值文件模板：可用变量以注释形式列出，默认不生效（不钉住系统默认值），
        // 用户取消注释并改写需要的项即可覆盖。
        let sys_value_file = value_root.sys_value_file();
        if !sys_value_file.exists() {
            std::fs::write(&sys_value_file, self.value_reference()?).source_resource()?;
        }
        Ok(value_root)
    }
}

/// 按指定 base 向上探测项目根并设置 `GXL_PRJ_ROOT`。
///
/// 用于当前目录不是目标目录的场景（如测试指定临时项目目录）。
/// CLI 正常路径**无需**调用：`GxOps::run()` 启动时已通过 `setup_start_env_vars()`
/// 按当前目录设好了 `GXL_PRJ_ROOT`，不要在命令实现里每次重设。
pub fn setup_prj_root_env_vars(base: PathBuf) -> MainResult<()> {
    let prj_root = find_project_define_base(base).unwrap_or(PathBuf::from("UNDEFIN"));
    unsafe { std::env::set_var("GXL_PRJ_ROOT", format!("{}", prj_root.display())) };
    Ok(())
}

#[cfg(test)]
pub mod tests {
    use std::path::{Path, PathBuf};

    use crate::prelude::ErrorOwe;
    use orion_conf::YamlIO;
    use orion_error::dev::testing::TestAssertWithMsg;
    use orion_infra::path::make_clean_path;
    use orion_variate::{
        addr::{Address, HttpResource, types::PathTemplate},
        tools::test_init,
        update::DownloadOptions,
    };
    use orion_vars::vars::{OriginDict, ValueDict};

    use crate::{
        accessor::accessor_for_test,
        const_vars::SYS_OPERATORS_ROOT,
        error::MainResult,
        module::{
            ModelSTD,
            depend::{Dependency, DependencySet},
        },
        system::{
            SysKind,
            operator::{SysOperator, setup_prj_root_env_vars},
            spec::SysModelSpec,
        },
        types::{LocalizeOptions, RefUpdateable},
    };
    #[tokio::test]
    async fn test_mod_prj_new() -> MainResult<()> {
        test_init();
        let prj_path = PathBuf::from(SYS_OPERATORS_ROOT).join("sys_new");
        make_clean_path(&prj_path).source_logic()?;
        let proj = SysOperator::make_new(&prj_path, "sys_new", ModelSTD::from_cur_sys())?;
        proj.save()?;
        Ok(())
    }

    #[test]
    fn test_load_kind_compat() -> MainResult<()> {
        test_init();
        let root = PathBuf::from(SYS_OPERATORS_ROOT).join("sys_load_kind");
        make_clean_path(&root).source_logic()?;
        let define_dir = root.join("sys");
        std::fs::create_dir_all(&define_dir).source_resource()?;
        let define_file = define_dir.join("sys_model.yml");

        // 1. 缺 sys_model.yml → 默认 Gxl，不报错
        assert_eq!(SysOperator::load_kind(&root), SysKind::Gxl);

        // 2. 有 sys_model.yml 但缺 kind → 默认 Gxl（向后兼容，不报错）
        std::fs::write(&define_file, "name: x\nvender: ''\n").source_resource()?;
        assert_eq!(SysOperator::load_kind(&root), SysKind::Gxl);

        // 3. kind: docker-compose → DockerCompose
        std::fs::write(&define_file, "name: x\nvender: ''\nkind: docker-compose\n")
            .source_resource()?;
        assert_eq!(SysOperator::load_kind(&root), SysKind::DockerCompose);

        Ok(())
    }

    #[test]
    fn test_with_kind_save_roundtrip() -> MainResult<()> {
        test_init();
        let prj_path = PathBuf::from(SYS_OPERATORS_ROOT).join("sys_kind_roundtrip");
        make_clean_path(&prj_path).source_logic()?;
        let proj =
            SysOperator::make_new(&prj_path, "sys_kind_roundtrip", ModelSTD::from_cur_sys())?
                .with_kind(SysKind::DockerCompose);
        proj.save()?;

        // 保存后 sys_model.yml 写入 kind，load_kind 能识别
        assert_eq!(SysOperator::load_kind(&prj_path), SysKind::DockerCompose);
        // 完整 load 也能读回 kind
        let loaded = SysOperator::load(&prj_path)?;
        assert_eq!(loaded.kind(), SysKind::DockerCompose);
        assert!(loaded.is_docker_compose());
        Ok(())
    }

    #[test]
    fn test_docker_compose_minimal_structure() -> MainResult<()> {
        test_init();
        let prj_path = PathBuf::from(SYS_OPERATORS_ROOT).join("sys_compose_minimal");
        make_clean_path(&prj_path).source_logic()?;
        let proj = SysOperator::make_new_docker(&prj_path, "sys_compose_minimal")?
            .with_kind(SysKind::DockerCompose);
        proj.save()?;

        let root = &prj_path;
        // 应生成的文件
        assert!(root.join("sys-prj.yml").exists());
        assert!(root.join("docker-compose.yml").exists());
        assert!(root.join("version.txt").exists());
        assert!(root.join("sys/sys_model.yml").exists());
        assert!(root.join("sys/setting/vars.yml").exists());
        // sys_model.yml 不含无意义的 model 字段（纯 compose 无目标型号），但含 kind
        let define = std::fs::read_to_string(root.join("sys/sys_model.yml")).unwrap();
        assert!(!define.contains("model:"));
        assert!(define.contains("kind: docker-compose"));
        // 不应生成的 GXL 相关文件/目录（纯 compose 系统多余）
        assert!(!root.join("sys/mod_list.yml").exists());
        assert!(!root.join("sys/setting/list.yml").exists());
        assert!(!root.join("sys/workflows").exists());
        assert!(!root.join("_gal/work.gxl").exists());
        assert!(!root.join("values").exists());

        // 完整 load 仍可读回 kind
        let loaded = SysOperator::load(root)?;
        assert_eq!(loaded.kind(), SysKind::DockerCompose);
        Ok(())
    }

    #[test]
    fn test_save_docker_compose_preserves_existing_files() -> MainResult<()> {
        test_init();
        let prj_path = PathBuf::from(SYS_OPERATORS_ROOT).join("sys_compose_preserve");
        make_clean_path(&prj_path).source_logic()?;
        let proj = SysOperator::make_new_docker(&prj_path, "sys_compose_preserve")?
            .with_kind(SysKind::DockerCompose);
        proj.save()?;

        // 模拟已有文件（用户自定义内容）
        let conf = prj_path.join("sys-prj.yml");
        let define = prj_path.join("sys/sys_model.yml");
        std::fs::write(&conf, "# user conf\n").unwrap();
        std::fs::write(&define, "# user define\n").unwrap();

        // 再次 save 不应覆盖已有文件
        proj.save()?;
        assert_eq!(std::fs::read_to_string(&conf).unwrap(), "# user conf\n");
        assert_eq!(std::fs::read_to_string(&define).unwrap(), "# user define\n");
        Ok(())
    }

    #[test]
    fn test_system_default_values_falls_back_to_legacy_sys_vars() -> MainResult<()> {
        test_init();
        let prj_path = PathBuf::from(SYS_OPERATORS_ROOT).join("sys_legacy_vars");
        make_clean_path(&prj_path).source_logic()?;
        let proj = SysOperator::make_new_docker(&prj_path, "sys_legacy_vars")?
            .with_kind(SysKind::DockerCompose);
        proj.save()?;

        // save() 不生成 merged_vars.yml；模拟旧系统只有 sys_vars.yml（旧名）
        assert!(!prj_path.join("sys/merged_vars.yml").exists());
        std::fs::write(
            prj_path.join("sys/sys_vars.yml"),
            "system:\n  - name: SERVICE_IMAGE\n    value: legacy-image\n",
        )
        .unwrap();

        // system_default_values 应回退读取旧名 sys_vars.yml
        let defaults = proj.system_default_values()?;
        assert_eq!(
            defaults
                .get("SERVICE_IMAGE")
                .map(|v| v.to_string())
                .as_deref(),
            Some("legacy-image")
        );

        // 参考以注释形式给出，可直接取消注释使用
        assert!(
            proj.value_reference()?
                .contains("# SERVICE_IMAGE: legacy-image")
        );

        // 生成的值文件是注释模板：存在但默认不生效
        let value_path = proj.init_setting_value()?;
        let text = std::fs::read_to_string(value_path.sys_value_file()).unwrap();
        assert!(text.contains("# SERVICE_IMAGE: legacy-image"));
        assert!(
            text.lines()
                .all(|l| l.trim().is_empty() || l.trim().starts_with('#'))
        );
        Ok(())
    }

    #[test]
    fn test_system_default_values_reads_merged_vars() -> MainResult<()> {
        test_init();
        let prj_path = PathBuf::from(SYS_OPERATORS_ROOT).join("sys_default_values");
        make_clean_path(&prj_path).source_logic()?;
        let proj = SysOperator::make_new_docker(&prj_path, "sys_default_values")?
            .with_kind(SysKind::DockerCompose);
        proj.save()?;

        // 缺少已解析变量时返回空字典（本地化会退化为只用值文件）
        assert!(proj.system_default_values()?.get("SERVICE_PORT").is_none());

        std::fs::write(
            prj_path.join("sys/merged_vars.yml"),
            "system:\n  - name: SERVICE_IMAGE\n    value: \"nginx:alpine\"\n  - name: SERVICE_PORT\n    value: \"8080\"\n",
        )
        .unwrap();

        let vals = proj.system_default_values()?;
        assert_eq!(
            vals.get("SERVICE_IMAGE").map(|v| v.to_string()).as_deref(),
            Some("nginx:alpine")
        );
        assert_eq!(
            vals.get("SERVICE_PORT").map(|v| v.to_string()).as_deref(),
            Some("8080")
        );
        Ok(())
    }

    #[test]
    fn test_value_reference_is_inert() -> MainResult<()> {
        test_init();
        let prj_path = PathBuf::from(SYS_OPERATORS_ROOT).join("sys_value_ref_inert");
        make_clean_path(&prj_path).source_logic()?;
        let proj = SysOperator::make_new_docker(&prj_path, "sys_value_ref_inert")?
            .with_kind(SysKind::DockerCompose);
        proj.save()?;

        // 含空格与 `#` 的值也要能被安全注释
        std::fs::write(
            prj_path.join("sys/merged_vars.yml"),
            "system:\n  - name: SERVICE_IMAGE\n    value: \"nginx:alpine\"\n  - name: NOTE\n    value: \"a # b\"\n",
        )
        .unwrap();

        let reference = proj.value_reference()?;
        assert!(reference.contains("# SERVICE_IMAGE:"));
        assert!(reference.contains("# NOTE:"));
        // 模板内容必须全部是注释（或空行）
        assert!(
            reference
                .lines()
                .all(|l| l.trim().is_empty() || l.trim().starts_with('#'))
        );

        // 核心不变式：模板作为值文件加载时等价于空覆盖（惰性）
        let value_path = proj.init_setting_value()?;
        let loaded = crate::project::load_value_file(&value_path.sys_value_file())?;
        assert!(loaded.is_empty(), "template must be inert, got: {loaded:?}");
        Ok(())
    }

    #[test]
    fn test_value_reference_empty_defaults() -> MainResult<()> {
        test_init();
        let prj_path = PathBuf::from(SYS_OPERATORS_ROOT).join("sys_value_ref_empty");
        make_clean_path(&prj_path).source_logic()?;
        let proj = SysOperator::make_new_docker(&prj_path, "sys_value_ref_empty")?
            .with_kind(SysKind::DockerCompose);
        proj.save()?;
        // 已解析但没有 system 变量
        std::fs::write(prj_path.join("sys/merged_vars.yml"), "system: []\n").unwrap();

        let reference = proj.value_reference()?;
        assert!(reference.contains("系统变量参考"));
        assert!(
            reference
                .lines()
                .all(|l| l.trim().is_empty() || l.trim().starts_with('#'))
        );

        // 仍生成模板文件（作为覆盖位置），但为空覆盖
        let value_path = proj.init_setting_value()?;
        assert!(value_path.sys_value_file().exists());
        assert!(crate::project::load_value_file(&value_path.sys_value_file())?.is_empty());
        Ok(())
    }

    #[test]
    fn test_init_setting_value_keeps_existing_value_file() -> MainResult<()> {
        test_init();
        let prj_path = PathBuf::from(SYS_OPERATORS_ROOT).join("sys_value_keep");
        make_clean_path(&prj_path).source_logic()?;
        let proj = SysOperator::make_new_docker(&prj_path, "sys_value_keep")?
            .with_kind(SysKind::DockerCompose);
        proj.save()?;
        std::fs::write(
            prj_path.join("sys/merged_vars.yml"),
            "system:\n  - name: SERVICE_IMAGE\n    value: \"nginx:alpine\"\n",
        )
        .unwrap();

        // 用户已写好的值文件（只覆盖一项）不应被模板覆盖
        let user_value = prj_path.join("values/sys_value.yml");
        std::fs::create_dir_all(user_value.parent().unwrap()).unwrap();
        std::fs::write(&user_value, "SERVICE_IMAGE: custom:tag\n").unwrap();

        let value_path = proj.init_setting_value()?;
        assert_eq!(value_path.sys_value_file(), user_value);
        assert_eq!(
            std::fs::read_to_string(&user_value).unwrap(),
            "SERVICE_IMAGE: custom:tag\n"
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_update_local_migrates_legacy_sys_vars() -> MainResult<()> {
        test_init();
        let prj_path = PathBuf::from(SYS_OPERATORS_ROOT).join("sys_migrate_vars");
        make_clean_path(&prj_path).source_logic()?;
        let proj = SysOperator::make_new_docker(&prj_path, "sys_migrate_vars")?
            .with_kind(SysKind::DockerCompose);
        proj.save()?;

        // 模拟旧系统：只有旧名 sys_vars.yml
        std::fs::write(
            prj_path.join("sys/sys_vars.yml"),
            "system:\n  - name: SERVICE_IMAGE\n    value: legacy-image\n",
        )
        .unwrap();
        assert!(prj_path.join("sys/sys_vars.yml").exists());

        // 重新从磁盘加载（设置 local），再执行 update_local
        let proj = SysOperator::load(&prj_path)?;
        let accessor = accessor_for_test();
        proj.update_local(accessor, &prj_path, &DownloadOptions::default())
            .await?;

        // 迁移：旧名被清理，新名生成
        assert!(!prj_path.join("sys/sys_vars.yml").exists());
        assert!(prj_path.join("sys/merged_vars.yml").exists());
        Ok(())
    }

    #[tokio::test]
    async fn test_sys_load_without_mod_list() -> MainResult<()> {
        test_init();
        let prj_path = PathBuf::from(SYS_OPERATORS_ROOT).join("sys_no_modlist");
        make_clean_path(&prj_path).source_logic()?;
        let proj = SysOperator::make_new(&prj_path, "sys_no_modlist", ModelSTD::from_cur_sys())?;
        proj.save()?;

        // 删除 mod_list.yml，验证加载时退化为空模块列表（纯 docker-compose 等场景）
        let modlist = prj_path.join("sys").join("mod_list.yml");
        std::fs::remove_file(&modlist).source_resource()?;

        let loaded = SysOperator::load(&prj_path)?;
        assert!(loaded.sys_spec().mod_list().is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_sys_load_without_workflows() -> MainResult<()> {
        test_init();
        let prj_path = PathBuf::from(SYS_OPERATORS_ROOT).join("sys_no_workflows");
        make_clean_path(&prj_path).source_logic()?;
        let proj = SysOperator::make_new(&prj_path, "sys_no_workflows", ModelSTD::from_cur_sys())?;
        proj.save()?;

        // 删除 sys/workflows/，验证加载时退化为空工作流
        let workflows = prj_path.join("sys").join("workflows");
        std::fs::remove_dir_all(&workflows).source_resource()?;

        let loaded = SysOperator::load(&prj_path)?;
        assert!(loaded.sys_spec().workflow().actions().is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_sys_load_without_setting_list() -> MainResult<()> {
        test_init();
        let prj_path = PathBuf::from(SYS_OPERATORS_ROOT).join("sys_no_list");
        make_clean_path(&prj_path).source_logic()?;
        let proj = SysOperator::make_new(&prj_path, "sys_no_list", ModelSTD::from_cur_sys())?;
        proj.save()?;

        // 删除 sys/setting/list.yml，验证加载时退化为空本地化列表
        let list = prj_path.join("sys").join("setting").join("list.yml");
        std::fs::remove_file(&list).source_resource()?;

        let loaded = SysOperator::load(&prj_path)?;
        assert!(loaded.sys_spec().setting().list().dicts().is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_sys_prj_example() -> MainResult<()> {
        test_init();

        let prj_path = PathBuf::from(SYS_OPERATORS_ROOT).join("example_sys_y");
        make_clean_path(&prj_path).source_logic()?;
        let project = make_sys_operator(&prj_path).assert("make cust");
        project.save().assert("save dss_prj");
        let project = SysOperator::load(&prj_path).assert("dss-project");
        let accessor = accessor_for_test();
        project
            .update_local(accessor, &prj_path, &DownloadOptions::default())
            .await
            .assert("spec.update_local");
        let value_path = project.init_setting_value()?;
        let mut dict = OriginDict::from(project.system_default_values()?);
        dict.set_source("sys-setting");
        setup_prj_root_env_vars(prj_path.clone()).source_sys()?;
        project
            .localize(value_path, LocalizeOptions::new(dict))
            .await
            .assert("spec.localize");
        Ok(())
    }

    fn make_sys_operator(prj_path: &Path) -> MainResult<SysOperator> {
        let mod_spec = SysModelSpec::for_example("exmaple_sys_2")?;
        let mut res = DependencySet::default();
        res.push(
            Dependency::new(
                Address::from(HttpResource::from(
                    "https://github.com/galaxio-labs/hello-word.git",
                )),
                PathTemplate::from(prj_path.join("test_res")),
            )
            .with_rename("bit-common"),
        );
        Ok(SysOperator::new(mod_spec, res, prj_path.to_path_buf()))
    }
}
