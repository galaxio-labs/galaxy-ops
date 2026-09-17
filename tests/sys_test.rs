use std::path::{Path, PathBuf};

use galaxy_ops::prelude::ErrorOwe;
use galaxy_ops::{
    accessor::accessor_for_test,
    const_vars::{OPS_PRJ_ROOT, SYS_OPERATORS_ROOT, VALUE_DIR},
    error::MainResult,
    module::depend::{Dependency, DependencySet},
    ops_prj::project::OpsProject,
    system::{SysValuePaths, operator::SysOperator, spec::SysModelSpec},
    types::{InsUpdateable, LocalizeOptions, RefUpdateable},
};
use orion_conf::YamlIO;
use orion_error::dev::testing::TestAssertWithMsg;
use orion_infra::path::make_clean_path;
use orion_variate::{
    addr::{Address, HttpResource, types::PathTemplate},
    archive::compress,
    tools::test_init,
    update::DownloadOptions,
};
use orion_vars::vars::{OriginDict, ValueDict};
#[tokio::test]
async fn test_full_flow() -> MainResult<()> {
    test_init();
    let sys_proj = make_sys_opr_example().await?;
    let out_path = PathBuf::from(SYS_OPERATORS_ROOT).join("example_sys_x-1.0.0.tar.gz");
    if out_path.exists() {
        std::fs::remove_file(&out_path).source_sys()?;
    }
    compress(sys_proj.root_local(), &out_path).source_sys()?;
    let mut ops_proj = make_workins_example().await?;
    let accessor = accessor_for_test();
    ops_proj
        .import_sys(
            accessor.clone(),
            out_path.display().to_string().as_str(),
            &DownloadOptions::for_test(),
        )
        .await?;
    //ops_proj.ia_setting(false)?;
    let sys_path = ops_proj.root_local().join("example_sys_x");
    let sys_proj = SysOperator::load(&sys_path)?;
    let sys_value_path = SysValuePaths::from(sys_path.clone()).join(VALUE_DIR);
    let sys_value_dict =
        OriginDict::from(ValueDict::load_yaml(&sys_value_path.sys_value_file()).source_conf()?)
            .with_origin("sys-setting");
    sys_proj
        .update_local(accessor, &sys_path, &DownloadOptions::default())
        .await?;
    sys_proj
        .localize(sys_value_path, LocalizeOptions::new(sys_value_dict))
        .await?;
    Ok(())
}
async fn make_workins_example() -> MainResult<OpsProject> {
    test_init();
    let prj_name = "obs_prj_x";
    let prj_path = PathBuf::from(OPS_PRJ_ROOT).join(prj_name);
    make_clean_path(&prj_path).source_logic()?;
    let project = OpsProject::for_test(prj_name).assert("make workins");
    project.save().assert("save workins_prj");
    let project = OpsProject::load(&prj_path).assert("workins-prj");
    let accessor = accessor_for_test();
    let project = project
        .update_local(accessor, &prj_path, &DownloadOptions::default())
        .await
        .assert("spec.update_local");
    Ok(project)
}

async fn make_sys_opr_example() -> MainResult<SysOperator> {
    let name = "example_sys_x";
    let prj_path = PathBuf::from(SYS_OPERATORS_ROOT).join(name);
    make_clean_path(&prj_path).source_logic()?;
    let sys_opr = make_sys_operator(&prj_path, name).assert("make cust");
    if prj_path.exists() {
        std::fs::remove_dir_all(&prj_path).assert("ok");
    }
    std::fs::create_dir_all(&prj_path).assert("yes");
    sys_opr.save().assert("save dss_prj");
    let loaded_sys_opr = SysOperator::load(&prj_path).assert("dss-project");
    let accessor = accessor_for_test();
    loaded_sys_opr
        .update_local(accessor, &prj_path, &DownloadOptions::default())
        .await
        .assert("spec.update_local");
    std::fs::remove_dir_all(loaded_sys_opr.paths().value_dir()).source_resource()?;
    /*
    loaded_sys_opr
        .localize(LocalizeOptions::for_test())
        .await
        .assert("spec.localize");
        */
    Ok(loaded_sys_opr)
}

fn make_sys_operator(prj_path: &Path, name: &str) -> MainResult<SysOperator> {
    let mod_spec = SysModelSpec::for_example(name)?;
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
