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
            // `addr` 以**项目根**为基准解析（`gops prj *` 在项目根运行）——这里给相对项目根的路径，
            // 而不是相对仓库根的 `out_path`（后者只有从仓库根跑才对）。
            "../../sys-operators/example_sys_x-1.0.0.tar.gz",
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
    // 预置旧布局内容与旧 `.gitignore`，验证 `sys update` 会迁移
    let legacy_dir = sys_path.join("sys/mods/legacy-mod/arm-mac14-host");
    std::fs::create_dir_all(&legacy_dir).unwrap();
    std::fs::write(legacy_dir.join("vars.yml"), "spoof").unwrap();
    std::fs::write(sys_path.join(".gitignore"), "sys/mods\nvalues/*\n").unwrap();
    sys_proj
        .update_local(accessor, &sys_path, &DownloadOptions::default())
        .await?;
    // 布局：模块按目标模型分组到 `sys/<model>/mods/<mod>`
    let sys_dir = sys_path.join("sys");
    assert!(
        sys_dir
            .join("arm-mac14-host/mods/redis2_mock/vars.yml")
            .exists()
    );
    assert!(
        sys_dir
            .join("arm-mac14-host/mods/mysql2_mock/vars.yml")
            .exists()
    );
    assert!(!sys_dir.join("mods").exists(), "旧布局 sys/mods 应被清理");
    // `.gitignore` 被迁移，新布局目录不会沦为未跟踪
    let ignore = std::fs::read_to_string(sys_path.join(".gitignore")).unwrap();
    assert!(
        ignore.contains("sys/*/mods"),
        "gitignore 应迁移到新布局规则"
    );
    sys_proj
        .localize(sys_value_path, LocalizeOptions::new(sys_value_dict))
        .await?;

    // 生成物应带上示例声明（这些目录被 .gitignore，仅在测试运行时生成）：
    //  - 系统侧 `sys-prj.yml`：现场态（protect / contract / backup）
    //  - 项目侧 `ops-prj.yml`：备份决策（backup.target）
    let src_sys_prj = std::fs::read_to_string(
        PathBuf::from(SYS_OPERATORS_ROOT)
            .join("example_sys_x")
            .join("sys-prj.yml"),
    )
    .source_resource()?;
    for key in ["protect:", "contract:", "backup:"] {
        assert!(
            src_sys_prj.contains(key),
            "sys-prj.yml 缺 {key}：\n{src_sys_prj}"
        );
    }
    let ops_prj = std::fs::read_to_string(
        PathBuf::from(OPS_PRJ_ROOT)
            .join("obs_prj_x")
            .join("ops-prj.yml"),
    )
    .source_resource()?;
    assert!(
        ops_prj.contains("../../sys-operators/"),
        "ops-prj.yml 的 addr 应相对项目根：\n{ops_prj}"
    );
    assert!(
        ops_prj.contains("backup:"),
        "ops-prj.yml 缺 backup：\n{ops_prj}"
    );
    assert!(
        ops_prj.contains("target: ./backup"),
        "ops-prj.yml 的 backup.target 不符：\n{ops_prj}"
    );
    assert!(
        ops_prj.contains("contract:"),
        "ops-prj.yml 缺 contract：\n{ops_prj}"
    );
    assert!(
        ops_prj.contains("target: ./contract"),
        "ops-prj.yml 的 contract.target 不符：\n{ops_prj}"
    );

    // `sync` 补齐缺失系统：删掉刚导入的系统目录，sync 应按包把它重新导入（不再是静默跳过）。
    let imported_sys = ops_proj.root_local().join("example_sys_x");
    std::fs::remove_dir_all(&imported_sys).source_resource()?;
    let reports = ops_proj
        .update_sys_content(accessor_for_test(), &DownloadOptions::for_test())
        .await?;
    assert!(
        reports
            .iter()
            .any(|r| r.name == "example_sys_x" && r.imported),
        "缺失系统应被导入，reports={reports:?}"
    );
    assert!(
        imported_sys.join("sys/sys_model.yml").exists(),
        "sync 应把缺失的系统目录按包重新铺回"
    );
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
    // 对外契约内容（site-owned，随示例入库以便演示）：见 docs/design/contract.md。
    let cfg_dir = prj_path.join("configs");
    std::fs::create_dir_all(&cfg_dir).assert("mk configs");
    std::fs::write(
        cfg_dir.join("public.yml"),
        "# 对外契约（external contract）：不可重建的公开现场事实。\n\
         # 见 docs/design/contract.md。它由 `sys-prj.yml: contract` 指到，内容是现场选的、\n\
         # 从交付包推不出、丢了（连人工）也补不回的东西。\n\
         public_url: https://example-sys.example.com\n",
    )
    .assert("write contract");
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
            PathTemplate::from("test_res"),
        )
        .with_rename("bit-common"),
    );
    // 示例演示现场态声明：`.env` 是 `sys localize` 产出（`pkg_exclude` 不进包 / `protect` 升级不碰 / `backup` 归档）；
    // `configs/` 是对外契约（不可重建的公开现场事实，只走 `prj contract export/import`）。
    Ok(
        SysOperator::new(mod_spec, res, prj_path.to_path_buf()).with_site_state(
            vec![],
            vec![".env".to_string()],
            vec![".env".to_string()],
            vec!["configs".to_string()],
            vec![".env".to_string()],
        ),
    )
}
