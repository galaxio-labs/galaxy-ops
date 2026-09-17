use crate::internal_prelude::*;

use orion_vars::vars::{ValueType, VarToValue};

use crate::{
    const_vars::{
        MERGED_VARS_YML, MOD_VALUE_FILE, SYS_VALUE_FILE, SYS_VARS_YML, USER_VALUE_FILE, VALUE_DIR,
    },
    types::LocalizeOptions,
};

pub fn load_mod_opr_value(root: &Path, model: &str) -> MainResult<OriginDict> {
    let value_root = ensure_path(root.join(VALUE_DIR)).source_logic()?;
    let sys_v_file = value_root.join(SYS_VALUE_FILE);
    if !sys_v_file.exists() {
        let mut ctx = OperationContext::want("build sys-value.yml").with_auto_log();
        let vars_file = root.join("mod").join(model).join("vars.yml");
        let vars_vec = VarCollection::load_conf(&vars_file).source_resource()?;
        let sys_value = vars_vec.system_vars().to_val();
        ctx.record("sys-value", sys_v_file.display());
        orion_conf::ConfigIO::save_conf(&sys_value, &sys_v_file).source_resource()?;
        ctx.mark_suc();
    }
    let mut sys_dict = OriginDict::from(ValueDict::load_yaml(&sys_v_file).source_logic()?);
    sys_dict.set_source("sys-setting");

    let mod_v_file = value_root.join(model).join(MOD_VALUE_FILE);
    if !mod_v_file.exists() {
        ensure_path(&value_root.join(model)).source_resource()?;
        let vars_file = root.join("mod").join(model).join("vars.yml");
        let vars_vec = VarCollection::load_conf(&vars_file).source_resource()?;
        let mod_value = vars_vec.module_vars().to_val();
        orion_conf::ConfigIO::save_conf(&mod_value, &mod_v_file).source_resource()?;
    }
    let mut mod_dict = OriginDict::from(ValueDict::load_yaml(&mod_v_file).source_logic()?);
    mod_dict.set_source("mod-setting");
    sys_dict.merge(&mod_dict);
    Ok(sys_dict)
}

pub fn load_sys_opr_value(prj_root: &Path) -> MainResult<OriginDict> {
    let value_root = ensure_path(prj_root.join(VALUE_DIR)).source_logic()?;
    let sys_v_file = value_root.join(SYS_VALUE_FILE);
    if !sys_v_file.exists() {
        let mut ctx = OperationContext::want("build sys-value.yml").with_auto_log();
        // 兼容旧名：merged_vars.yml 优先，缺失时回退 sys_vars.yml
        let new_vars_file = prj_root.join("sys").join(MERGED_VARS_YML);
        let legacy_vars_file = prj_root.join("sys").join(SYS_VARS_YML);
        let vars_file = if new_vars_file.exists() {
            new_vars_file
        } else if legacy_vars_file.exists() {
            legacy_vars_file
        } else {
            new_vars_file
        };
        if !vars_file.exists() {
            return Err(crate::error::MainReason::logic_detail(format!(
                "系统变量未解析：缺少 `{}`。请先在该系统上执行 `gops sys update` 解析变量，再打包导入",
                vars_file.display()
            )));
        }
        let vars_vec = VarCollection::load_conf(&vars_file).source_resource()?;
        let sys_value = vars_vec.system_vars().to_val();
        ctx.record("sys-value", sys_v_file.display());
        orion_conf::ConfigIO::save_conf(&sys_value, &sys_v_file).source_resource()?;
        ctx.mark_suc();
    }
    let mut sys_dict = OriginDict::from(ValueDict::load_yaml(&sys_v_file).source_logic()?);
    sys_dict.set_source("sys-setting");
    Ok(sys_dict)
}

/// 读取值文件（YAML 映射）。
///
/// 允许“全注释/空”文件：值文件模板把可用变量以注释形式列出，未取消注释时内容全为注释，
/// 此时视为**空覆盖**（不钉住任何默认值），而不是报解析错误。
/// 仅含 YAML 文档标记（`---`、`...`）的文件同样视为空覆盖。
pub fn load_value_file(path: &Path) -> MainResult<ValueDict> {
    let text = std::fs::read_to_string(path).source_resource()?;
    // 仅注释 / 空 / 仅文档标记（`---`、`...`）都视为空覆盖
    let has_content = text.lines().any(|line| {
        let trimmed = line.trim();
        !trimmed.is_empty() && !trimmed.starts_with('#') && trimmed != "---" && trimmed != "..."
    });
    if !has_content {
        return Ok(ValueDict::default());
    }
    Ok(ValueDict::load_yaml(path).source_resource()?)
}

pub fn mix_used_value(
    options: LocalizeOptions,
    vars: &VarCollection,
    mod_value: &Path,
) -> MainResult<OriginDict> {
    let mut used = OriginDict::default();
    let mut default = OriginDict::from(vars.clone());
    default.set_source("mod-default");
    used.merge(&default);

    // 加载用户值文件（如果存在）
    let user_value_path = mod_value.parent().unwrap().join(USER_VALUE_FILE);
    if user_value_path.exists() {
        let mut user_dict = OriginDict::from(load_value_file(&user_value_path)?);
        user_dict.set_source("mod-cust");
        used.merge(&user_dict);
    }

    let mut mod_dict = OriginDict::from(ValueDict::load_yaml(mod_value).source_resource()?);
    mod_dict.set_source("mod-setting");
    let mut global = options.raw_value().clone();
    global.set_source("global");
    used.merge(&mod_dict);
    used.merge(&global);
    let used = used.env_eval(&EnvDict::default());
    Ok(used)
}

/// 把值字典导出为 dotenv 格式（`KEY=VALUE`），供 docker-compose 等使用 `${VAR}` 的工具消费。
///
/// - 键保持大写（与 `UpperKey` 一致）；
/// - 简单标量（字母数字 + `_.:/@+-`）原样输出，其余（含空格、引号、`#`、`$`、嵌套对象/列表）用双引号包裹并转义。
pub fn export_env_file(dict: &OriginDict, out_path: &Path) -> MainResult<()> {
    let mut content = String::new();
    for (key, value) in dict.iter() {
        content.push_str(key.as_str());
        content.push('=');
        content.push_str(&format_env_value(value.value()));
        content.push('\n');
    }
    std::fs::write(out_path, content).source_resource()?;
    Ok(())
}

fn format_env_value(v: &ValueType) -> String {
    match v {
        ValueType::String(s) => {
            if needs_env_quote(s) {
                quote_env(s)
            } else {
                s.clone()
            }
        }
        ValueType::Obj(o) => quote_env(&serde_json::to_string(o).unwrap_or_default()),
        ValueType::List(l) => quote_env(&serde_json::to_string(l).unwrap_or_default()),
        other => other.to_string(),
    }
}

fn needs_env_quote(s: &str) -> bool {
    s.is_empty()
        || s.chars().any(|c| {
            !(c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '/' | '@' | '+' | '-'))
        })
}

fn quote_env(s: &str) -> String {
    format!(
        "\"{}\"",
        s.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::const_vars::USER_VALUE_FILE;

    use super::*;
    use orion_error::dev::testing::TestAssert;
    use orion_vars::vars::{Mutability, OriginValue, ValueType, VarDefinition};
    use tempfile::tempdir;

    fn test_init() {
        let _ = env_logger::builder().is_test(true).try_init();
    }

    #[test]
    fn test_build_used_value_with_default_only() {
        test_init();
        let vars = VarCollection::define(vec![VarDefinition::from(("TEST_KEY", "default_value"))]);
        let options = LocalizeOptions::new(OriginDict::new());
        let temp_dir = tempdir().unwrap();
        let mod_value_path = temp_dir.path().join(MOD_VALUE_FILE);
        // 创建空的 mod_value.yml 文件
        std::fs::write(&mod_value_path, "").unwrap();

        let result = mix_used_value(options, &vars, &mod_value_path).unwrap();
        assert_eq!(
            result.get("TEST_KEY"),
            Some(&OriginValue::from("default_value").with_origin("mod-default"))
        );
    }

    #[test]
    fn test_build_used_value_with_global_value() {
        test_init();
        let mut global_dict = OriginDict::new();
        global_dict.insert("TEST_KEY".to_string(), ValueType::from("global_value"));
        global_dict.insert("PRJ_SPACE".to_string(), ValueType::from("galaxy"));
        let vars = VarCollection::define(vec![
            VarDefinition::from(("TEST_KEY", "default_value"))
                .with_mutability(Mutability::Immutable),
            VarDefinition::from(("PRJ_SPACE", "${HOME}")),
            VarDefinition::from(("SVR_NAME", "gflow")),
            VarDefinition::from(("MOD_SPACE", "${PRJ_SPACE}/${SVR_NAME}")),
            VarDefinition::from(("SVR_SPACE", "/home/${SVR_NAME}")),
        ]);
        let options = LocalizeOptions::new(global_dict);
        let temp_dir = tempdir().unwrap();
        let mod_value_path = temp_dir.path().join(MOD_VALUE_FILE);
        // 创建只包含SVR_NAME的mod_value.yml文件，不包含PRJ_SPACE，让PRJ_SPACE来自全局设置
        std::fs::write(&mod_value_path, "SVR_NAME: gflow").unwrap();

        let result = mix_used_value(options, &vars, &mod_value_path).assert();
        assert_eq!(
            result.get("TEST_KEY"),
            Some(
                &OriginValue::from("default_value")
                    .with_origin("mod-default")
                    .with_mutability(Mutability::Immutable),
            )
        );
        assert_eq!(
            result.get("PRJ_SPACE"),
            Some(&OriginValue::from("galaxy").with_origin("global"))
        );
        assert_eq!(
            result.get("SVR_SPACE"),
            Some(&OriginValue::from("/home/gflow").with_origin("mod-default"))
        );
        assert_eq!(
            result.get("MOD_SPACE"),
            Some(&OriginValue::from("galaxy/gflow").with_origin("mod-default"))
        );
    }

    #[test]
    fn test_build_used_value_with_user_value() {
        test_init();
        let temp_dir = tempdir().unwrap();
        let user_value_path = temp_dir.path().join(USER_VALUE_FILE);
        std::fs::write(&user_value_path, "TEST_KEY: user_value").unwrap();

        let vars = VarCollection::define(vec![VarDefinition::from(("TEST_KEY", "default_value"))]);
        let options = LocalizeOptions::new(OriginDict::new());
        let mod_value_path = temp_dir.path().join(MOD_VALUE_FILE);
        // 创建空的 mod_value.yml 文件
        std::fs::write(&mod_value_path, "").unwrap();

        let result = mix_used_value(options, &vars, &mod_value_path).unwrap();
        assert_eq!(
            result.get("TEST_KEY"),
            Some(&OriginValue::from("user_value").with_origin("mod-cust"))
        );
    }

    #[test]
    fn test_build_used_value_merge_precedence() {
        test_init();
        let temp_dir = tempdir().unwrap();
        let cust_value_path = temp_dir.path().join(USER_VALUE_FILE);
        std::fs::write(
            &cust_value_path,
            "TEST_KEY: user_value\nUSER_ONLY: user_only",
        )
        .unwrap();

        let mut global_dict = OriginDict::new();
        global_dict.insert("TEST_KEY".to_string(), ValueType::from("global_value"));
        global_dict.insert("GLOBAL_ONLY".to_string(), ValueType::from("global_only"));

        let vars = VarCollection::define(vec![
            VarDefinition::from(("TEST_KEY", "default_value")),
            VarDefinition::from(("DEFAULT_ONLY", "default_only")),
        ]);
        let options = LocalizeOptions::new(global_dict);
        let mod_value_path = temp_dir.path().join(MOD_VALUE_FILE);
        // 创建空的 mod_value.yml 文件
        std::fs::write(&mod_value_path, "").unwrap();

        let result = mix_used_value(options, &vars, &mod_value_path).unwrap();
        // 验证优先级: global > cust  > default
        assert_eq!(
            result.get("TEST_KEY"),
            Some(&OriginValue::from("global_value").with_origin("global"))
        );
        // 验证各层特有键都存在
        assert_eq!(
            result.get("GLOBAL_ONLY"),
            Some(&OriginValue::from("global_only").with_origin("global"))
        );
        assert_eq!(
            result.get("USER_ONLY"),
            Some(&OriginValue::from("user_only").with_origin("mod-cust"))
        );
        assert_eq!(
            result.get("DEFAULT_ONLY"),
            Some(&OriginValue::from("default_only").with_origin("mod-default"))
        );
    }

    #[test]
    fn test_empty_vars_returns_empty_dict() {
        test_init();
        let vars = VarCollection::define(vec![]);
        let options = LocalizeOptions::new(OriginDict::new());
        let temp_dir = tempdir().unwrap();
        let mod_value_path = temp_dir.path().join(MOD_VALUE_FILE);
        // 创建空的 mod_value.yml 文件
        std::fs::write(&mod_value_path, "").unwrap();

        let result = mix_used_value(options, &vars, &mod_value_path).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_complex_value_types() {
        test_init();

        let vars = VarCollection::define(vec![
            VarDefinition::from(("STRING_VAR", ValueType::from("default_string"))),
            VarDefinition::from(("NUMBER_VAR", ValueType::from(42))),
            VarDefinition::from(("BOOL_VAR", ValueType::from(true))),
        ]);
        let options = LocalizeOptions::new(OriginDict::new());
        let temp_dir = tempdir().unwrap();
        let mod_value_path = temp_dir.path().join(MOD_VALUE_FILE);
        // 创建空的 mod_value.yml 文件
        std::fs::write(&mod_value_path, "").unwrap();

        let result = mix_used_value(options, &vars, &mod_value_path).unwrap();

        assert_eq!(
            result.get("STRING_VAR"),
            Some(&OriginValue::from(ValueType::from("default_string")).with_origin("mod-default"))
        );
        assert_eq!(
            result.get("NUMBER_VAR"),
            Some(&OriginValue::from(ValueType::from(42)).with_origin("mod-default"))
        );
        assert_eq!(
            result.get("BOOL_VAR"),
            Some(&OriginValue::from(ValueType::from(true)).with_origin("mod-default"))
        );
    }

    #[test]
    fn test_env_variable_substitution() {
        test_init();
        unsafe {
            std::env::set_var("TEST_ENV_VAR", "substituted_value");
        }

        let vars = VarCollection::define(vec![
            VarDefinition::from(("ENV_VAR", "${TEST_ENV_VAR}")),
            VarDefinition::from(("MIXED_VAR", "prefix_${TEST_ENV_VAR}_suffix")),
        ]);
        let options = LocalizeOptions::new(OriginDict::new());
        let temp_dir = tempdir().unwrap();
        let mod_value_path = temp_dir.path().join(MOD_VALUE_FILE);
        // 创建空的 mod_value.yml 文件
        std::fs::write(&mod_value_path, "").unwrap();

        let result = mix_used_value(options, &vars, &mod_value_path).unwrap();

        assert_eq!(
            result.get("ENV_VAR"),
            Some(&OriginValue::from("substituted_value").with_origin("mod-default"))
        );
        assert_eq!(
            result.get("MIXED_VAR"),
            Some(&OriginValue::from("prefix_substituted_value_suffix").with_origin("mod-default"))
        );

        unsafe {
            std::env::remove_var("TEST_ENV_VAR");
        }
    }

    #[test]
    fn test_use_ver_final_answer() {
        test_init();

        let temp_dir = tempdir().unwrap();
        let mod_value_path = temp_dir.path().join(MOD_VALUE_FILE);

        let vars = VarCollection::load_yaml(&PathBuf::from("./src/data/vars.yml")).assert();
        //let mut global_dict = OriginDict::from(vars);
        // 创建 mod_value.yml 文件，使用给定的输入数据（扁平结构）
        let mod_value_content = r#"
"#;
        std::fs::write(&mod_value_path, mod_value_content).unwrap();

        let options = LocalizeOptions::new(OriginDict::new());
        let result = mix_used_value(options.clone(), &vars, &mod_value_path).unwrap();

        // 验证：没有环境变量时，use_ver 保持原样
        let use_ver_result = result.get_case_insensitive("use_ver").assert();
        assert_eq!(use_ver_result.value(), &ValueType::from("v0.12.6-beta"));
    }

    #[test]
    fn test_global_value_override_precedence() {
        test_init();
        let temp_dir = tempdir().unwrap();
        let user_value_path = temp_dir.path().join(USER_VALUE_FILE);
        std::fs::write(&user_value_path, "TEST_KEY: user_value").unwrap();

        let mut global_dict = OriginDict::new();
        global_dict.insert("TEST_KEY".to_string(), ValueType::from("global_value"));

        let vars = VarCollection::define(vec![VarDefinition::from(("TEST_KEY", "default_value"))]);
        let options = LocalizeOptions::new(global_dict);
        let mod_value_path = temp_dir.path().join(MOD_VALUE_FILE);
        // 创建空的 mod_value.yml 文件
        std::fs::write(&mod_value_path, "").unwrap();

        let result = mix_used_value(options, &vars, &mod_value_path).unwrap();
        // 全局值应该覆盖用户值和默认值
        assert_eq!(
            result.get("TEST_KEY"),
            Some(&OriginValue::from("global_value").with_origin("global"))
        );
    }

    #[test]
    fn test_export_env_file() {
        test_init();
        let mut dict = OriginDict::new();
        dict.insert("nginx_tag", ValueType::from("1.25-alpine"));
        dict.insert("http_port", ValueType::from(8080u64));
        dict.insert("db_host", ValueType::from("10.0.0.11"));
        dict.insert("db_password", ValueType::from("p@ss word#1"));
        dict.insert("debug", ValueType::from(true));

        let temp_dir = tempdir().unwrap();
        let out = temp_dir.path().join(".env");
        export_env_file(&dict, &out).unwrap();

        let content = std::fs::read_to_string(&out).unwrap();
        assert!(content.contains("NGINX_TAG=1.25-alpine"));
        assert!(content.contains("HTTP_PORT=8080"));
        assert!(content.contains("DB_HOST=10.0.0.11"));
        assert!(content.contains("DB_PASSWORD=\"p@ss word#1\""));
        assert!(content.contains("DEBUG=true"));
    }

    #[test]
    fn test_load_sys_opr_value_falls_back_to_legacy_sys_vars() {
        test_init();
        let temp_dir = tempdir().unwrap();
        std::fs::create_dir_all(temp_dir.path().join("sys")).unwrap();
        // 只有旧名 sys_vars.yml（模拟旧系统）
        std::fs::write(
            temp_dir.path().join("sys/sys_vars.yml"),
            "system:\n  - name: SERVICE_IMAGE\n    value: legacy-image\n",
        )
        .unwrap();

        let dict = load_sys_opr_value(temp_dir.path()).unwrap();

        assert_eq!(
            dict.get("SERVICE_IMAGE")
                .map(|v| v.value().to_string())
                .as_deref(),
            Some("legacy-image")
        );
        // 应生成 values/sys_value.yml
        assert!(temp_dir.path().join("values/sys_value.yml").exists());
    }

    #[test]
    fn test_load_sys_opr_value_errors_when_no_vars_file() {
        test_init();
        let temp_dir = tempdir().unwrap();
        std::fs::create_dir_all(temp_dir.path().join("sys")).unwrap();

        let result = load_sys_opr_value(temp_dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn test_load_value_file_allows_comment_only() {
        test_init();
        let temp_dir = tempdir().unwrap();
        let path = temp_dir.path().join("sys_value.yml");

        // 全注释（值文件模板未取消注释）-> 空覆盖，不报错
        std::fs::write(&path, "# HTTP_PORT: 8080\n# REPLICAS: 3\n").unwrap();
        assert_eq!(load_value_file(&path).assert().len(), 0);

        // 空文件同样视为空覆盖
        std::fs::write(&path, "\n").unwrap();
        assert_eq!(load_value_file(&path).assert().len(), 0);

        // 仅 YAML 文档标记（--- / ...）也视为空覆盖，而不是解析报错
        std::fs::write(&path, "---\n").unwrap();
        assert_eq!(load_value_file(&path).assert().len(), 0);
        std::fs::write(&path, "---\n...\n").unwrap();
        assert_eq!(load_value_file(&path).assert().len(), 0);

        // 有内容时正常解析
        std::fs::write(&path, "HTTP_PORT: 9090\n").unwrap();
        let dict = load_value_file(&path).assert();
        assert_eq!(
            dict.get("HTTP_PORT").map(|v| v.to_string()).as_deref(),
            Some("9090")
        );
    }
}
