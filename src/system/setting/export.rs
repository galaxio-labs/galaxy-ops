use crate::internal_prelude::*;

use orion_vars::vars::EnvEvalable;

use super::LocalizeConf;

#[derive(Clone, Debug, Serialize, Deserialize, Getters, Default)]
#[getset(get = "pub")]
pub struct Setting {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    localize: Option<LocalizeConf>,
}

impl Setting {
    pub fn example() -> Self {
        Self {
            localize: Some(LocalizeConf::example()),
        }
    }
}

impl EnvEvalable<Setting> for Setting {
    fn env_eval(self, dict: &orion_vars::vars::EnvDict) -> Self {
        Self {
            localize: self.localize.map(|l| l.env_eval(dict)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::env::temp_dir;

    use super::*;
    use orion_error::dev::testing::TestAssert;
    use orion_vars::vars::{EnvDict, EnvEvalable, ValueType};

    #[test]
    fn test_setting_serialization() {
        let temp_dir = temp_dir();
        let save_path = temp_dir.join("setting.yml");
        let setting = Setting::example();
        orion_conf::ConfigIO::save_conf(&setting, &save_path).assert();
        println!("{}", std::fs::read_to_string(&save_path).unwrap());
        Setting::load_conf(&save_path).assert();
    }

    // 测试辅助函数
    fn create_test_env_dict() -> EnvDict {
        let mut dict = EnvDict::new();
        dict.insert("BEG_TAG".to_string(), ValueType::String("{{".to_string()));
        dict.insert("END_TAG".to_string(), ValueType::String("}}".to_string()));
        dict
    }

    // Setting 集成测试
    #[test]
    fn test_setting_env_eval_example() {
        // 使用 example() 方法创建实例，测试完整的 env_eval 链
        let setting = Setting::example();
        let env_dict = create_test_env_dict();
        let result = setting.env_eval(&env_dict);

        // 验证结构完整性
        assert!(result.localize().is_some());

        // 验证所有层级都存在且结构完整
        if let Some(localize) = result.localize() {
            assert!(localize.templatize_path().is_some());
            assert!(localize.templatize_cust().is_some());

            // 验证子结构的环境变量替换功能
            if let Some(path) = localize.templatize_path() {
                //assert!(path.includes().len() > 0);
                assert!(!path.excludes().is_empty());
            }

            if let Some(cust) = localize.templatize_cust() {
                assert!(!cust.label_beg().is_empty());
                assert!(!cust.label_end().is_empty());
            }
        }
    }

    #[test]
    fn test_setting_env_eval_none() {
        let setting = Setting { localize: None };

        let result = setting.env_eval(&EnvDict::default());
        assert!(result.localize().is_none());
    }

    #[test]
    fn test_setting_env_eval_empty_dict() {
        // 测试使用空的环境字典
        let setting = Setting::example();
        let result = setting.env_eval(&EnvDict::default());

        // 验证 example() 返回的结构在默认 env_dict 下保持完整
        assert!(result.localize().is_some());
        if let Some(localize) = result.localize() {
            assert!(localize.templatize_path().is_some());
            assert!(localize.templatize_cust().is_some());
        }
    }
}
