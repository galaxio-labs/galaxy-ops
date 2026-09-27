use crate::internal_prelude::*;

use orion_vars::vars::EnvEvalable;

use super::{TemplateCustom, TemplateTargets};

#[derive(Clone, Debug, Serialize, Deserialize, Getters)]
#[getset(get = "pub")]
pub struct LocalizeConf {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    templatize_path: Option<TemplateTargets>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    templatize_cust: Option<TemplateCustom>,
}

impl LocalizeConf {
    pub fn new(
        templatize_path: Option<TemplateTargets>,
        templatize_cust: Option<TemplateCustom>,
    ) -> Self {
        Self {
            templatize_path,
            templatize_cust,
        }
    }
    pub fn example() -> Self {
        Self {
            templatize_path: Some(TemplateTargets::example()),
            templatize_cust: Some(TemplateCustom::example()),
        }
    }
}

impl EnvEvalable<LocalizeConf> for LocalizeConf {
    fn env_eval(self, dict: &orion_vars::vars::EnvDict) -> Self {
        Self {
            templatize_path: self.templatize_path.map(|t| t.env_eval(dict)),
            templatize_cust: self.templatize_cust.map(|t| t.env_eval(dict)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orion_vars::vars::{EnvDict, EnvEvalable, ValueType};

    // 测试辅助函数
    fn create_test_env_dict() -> EnvDict {
        let mut dict = EnvDict::new();
        dict.insert("BEG_TAG".to_string(), ValueType::String("{{".to_string()));
        dict.insert("END_TAG".to_string(), ValueType::String("}}".to_string()));
        dict
    }

    // LocalizeConf 集成测试
    #[test]
    fn test_localize_conf_env_eval_example() {
        // 使用 example() 方法创建实例，测试 env_eval 不会破坏结构
        let conf = LocalizeConf::example();
        let env_dict = create_test_env_dict();
        let result = conf.env_eval(&env_dict);

        // 验证结构完整性
        assert!(result.templatize_path().is_some());
        assert!(result.templatize_cust().is_some());
    }

    #[test]
    fn test_localize_conf_env_eval_none_values() {
        let conf = LocalizeConf {
            templatize_path: None,
            templatize_cust: None,
        };

        let result = conf.env_eval(&EnvDict::default());
        assert!(result.templatize_path().is_none());
        assert!(result.templatize_cust().is_none());
    }

    #[test]
    fn test_localize_conf_env_eval_empty_dict() {
        // 测试使用空的环境字典
        let conf = LocalizeConf::example();
        let result = conf.env_eval(&EnvDict::default());

        // 验证 example() 返回的结构在默认 env_dict 下保持完整
        assert!(result.templatize_path().is_some());
        assert!(result.templatize_cust().is_some());
    }
}
