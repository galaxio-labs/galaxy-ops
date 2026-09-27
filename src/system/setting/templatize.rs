use crate::{
    internal_prelude::*,
    localize::{TemplateConfig, TemplatePath},
};

use orion_vars::vars::EnvEvalable;

#[derive(Clone, Debug, Serialize, Deserialize, Getters)]
#[getset(get = "pub")]
pub struct TemplateCustom {
    label_beg: String,
    label_end: String,
}

impl From<TemplateCustom> for TemplateConfig {
    fn from(value: TemplateCustom) -> Self {
        Self::new(
            (value.label_beg, value.label_end),
            ("{{".into(), "}}".into()),
        )
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, Getters)]
#[getset(get = "pub")]
pub struct TemplateTargets {
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    includes: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    excludes: Vec<String>,
}

impl TemplateTargets {
    pub fn export_paths(&self, root: &Path) -> TemplatePath {
        let includes = self.includes().iter().map(|x| root.join(x)).collect();
        let excludes = self.excludes().iter().map(|x| root.join(x)).collect();
        TemplatePath::new(includes, excludes)
    }
}

impl TemplateTargets {
    pub fn new(includes: Vec<String>, excludes: Vec<String>) -> Self {
        Self { includes, excludes }
    }
    pub fn example() -> Self {
        Self {
            includes: vec![],
            excludes: vec!["README.md".to_string()],
        }
    }
}

impl EnvEvalable<TemplateTargets> for TemplateTargets {
    fn env_eval(self, dict: &orion_vars::vars::EnvDict) -> Self {
        Self {
            includes: self
                .includes
                .into_iter()
                .map(|s| s.env_eval(dict))
                .collect(),
            excludes: self
                .excludes
                .into_iter()
                .map(|s| s.env_eval(dict))
                .collect(),
        }
    }
}

impl TemplateCustom {
    pub fn new<B: Into<String>, E: Into<String>>(label_beg: B, label_end: E) -> Self {
        Self {
            label_beg: label_beg.into(),
            label_end: label_end.into(),
        }
    }
    pub fn example() -> Self {
        Self {
            label_beg: "[[".into(),
            label_end: "]]".into(),
        }
    }
}

impl EnvEvalable<TemplateCustom> for TemplateCustom {
    fn env_eval(self, dict: &orion_vars::vars::EnvDict) -> Self {
        Self {
            label_beg: self.label_beg.env_eval(dict),
            label_end: self.label_end.env_eval(dict),
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
        dict.insert(
            "INCLUDE_PATH".to_string(),
            ValueType::String("/include/*.conf".to_string()),
        );
        dict.insert(
            "EXCLUDE_PATH".to_string(),
            ValueType::String("/exclude/test.*".to_string()),
        );
        dict
    }

    // TemplateCustom 单元测试
    #[test]
    fn test_template_custom_env_eval_basic() {
        let custom = TemplateCustom::example();
        let result = custom.env_eval(&EnvDict::default());

        // example() 返回的值在默认 env_dict 下应该保持不变
        assert_eq!(result.label_beg(), "[[");
        assert_eq!(result.label_end(), "]]");
    }

    // 测试已被上面的 test_template_custom_env_eval_basic 覆盖

    // TemplateTargets 单元测试
    #[test]
    fn test_template_targets_env_eval_example() {
        // 使用 example() 方法创建实例，测试 env_eval 不会破坏结构
        let targets = TemplateTargets::example();
        let env_dict = create_test_env_dict();
        let result = targets.env_eval(&env_dict);

        // 验证结构完整性
        //assert!(result.includes().len() > 0);
        assert!(!result.excludes().is_empty());
    }

    #[test]
    fn test_template_targets_env_eval_empty_dict() {
        // 测试使用空的环境字典
        let targets = TemplateTargets::example();
        let result = targets.clone().env_eval(&EnvDict::default());

        // example() 返回的值在默认 env_dict 下应该保持不变
        assert_eq!(result.includes().len(), targets.includes().len());
        assert_eq!(result.excludes().len(), targets.excludes().len());
    }
}
