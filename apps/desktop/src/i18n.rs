use nexus_domain::{PermissionMode, ThinkingEffort};
use nexus_protocol::HarnessProbe;
use std::{collections::BTreeMap, sync::LazyLock};

// Chinese source text is the catalog key and the fallback for missing translations.
static ENGLISH: LazyLock<BTreeMap<String, String>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../locales/en.json")).expect("valid English catalog")
});

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Language {
    #[default]
    Chinese,
    English,
}

impl Language {
    pub(crate) fn from_setting(value: &str) -> Self {
        match value {
            "en" => Self::English,
            _ => Self::Chinese,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Chinese => "zh-CN",
            Self::English => "en",
        }
    }

    pub(crate) fn text(self, source: &'static str) -> &'static str {
        match self {
            Self::Chinese => source,
            Self::English => ENGLISH.get(source).map(String::as_str).unwrap_or(source),
        }
    }

    pub(crate) fn permission_mode(self, mode: PermissionMode) -> &'static str {
        self.text(match mode {
            PermissionMode::Ask => "请求授权",
            PermissionMode::AutoEdit => "自动编辑",
            PermissionMode::Yolo => "YOLO",
        })
    }

    pub(crate) fn effort(self, effort: ThinkingEffort) -> &'static str {
        self.text(match effort {
            ThinkingEffort::Default => "模型默认",
            ThinkingEffort::None => "无",
            ThinkingEffort::Off => "关闭",
            ThinkingEffort::Minimal => "最低",
            ThinkingEffort::Low => "低",
            ThinkingEffort::Medium => "中",
            ThinkingEffort::High => "高",
            ThinkingEffort::XHigh => "极高（XHigh）",
            ThinkingEffort::Max => "最高（Max）",
            ThinkingEffort::Ultra => "超高（Ultra）",
            ThinkingEffort::Auto => "自动",
        })
    }

    pub(crate) fn format(self, source: &'static str, arguments: &[(&str, String)]) -> String {
        let mut remaining = self.text(source);
        let mut result = String::with_capacity(remaining.len());
        // Scan only the template: braces in user-provided values stay untouched.
        while let Some((prefix, rest)) = remaining.split_once('{') {
            result.push_str(prefix);
            let Some((name, suffix)) = rest.split_once('}') else {
                result.push('{');
                result.push_str(rest);
                return result;
            };
            if let Some((_, value)) = arguments.iter().find(|(key, _)| *key == name) {
                result.push_str(value);
            } else {
                result.push('{');
                result.push_str(name);
                result.push('}');
            }
            remaining = suffix;
        }
        result.push_str(remaining);
        result
    }
}

// Keep both presentations of transient app status so switching languages also
// updates existing status. Raw diagnostics are preserved in either language.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct LocalizedText {
    chinese: String,
    english: String,
}

impl LocalizedText {
    pub(crate) fn new(source: &'static str, arguments: &[(&str, String)]) -> Self {
        Self::translated(|language| language.format(source, arguments))
    }

    pub(crate) fn translated(format: impl Fn(Language) -> String) -> Self {
        Self {
            chinese: format(Language::Chinese),
            english: format(Language::English),
        }
    }

    pub(crate) fn render(&self, language: Language) -> &str {
        match language {
            Language::Chinese => &self.chinese,
            Language::English => &self.english,
        }
    }
}

impl From<&'static str> for LocalizedText {
    fn from(source: &'static str) -> Self {
        Self::new(source, &[])
    }
}

impl From<String> for LocalizedText {
    fn from(value: String) -> Self {
        Self {
            chinese: value.clone(),
            english: value,
        }
    }
}

pub(crate) fn probe_status(probe: &HarnessProbe) -> LocalizedText {
    LocalizedText::translated(|language| {
        if language == Language::Chinese {
            return probe.message.clone();
        }
        let source = if !probe.available {
            "{harness} 不可用，请在设置中检查可执行文件。"
        } else if !probe.authenticated {
            "{harness} 尚未登录，请登录 CLI 或配置 Provider Profile。"
        } else {
            "{harness} 已就绪。"
        };
        language.format(source, &[("harness", probe.harness.to_string())])
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn english_catalog_preserves_every_template_argument() {
        fn placeholders(template: &str) -> BTreeSet<&str> {
            template
                .split('{')
                .skip(1)
                .map(|part| part.split_once('}').expect("closed placeholder").0)
                .collect()
        }
        for (source, translation) in ENGLISH.iter() {
            assert!(
                !translation.trim().is_empty(),
                "empty translation: {source}"
            );
            assert_eq!(placeholders(source), placeholders(translation), "{source}");
        }
    }

    #[test]
    fn language_formats_app_copy_and_preserves_user_values_and_raw_diagnostics() {
        let message = LocalizedText::new(
            "永久删除“{title}”？",
            &[("title", "设置 {count} café".into())],
        );
        assert_eq!(
            message.render(Language::Chinese),
            "永久删除“设置 {count} café”？"
        );
        assert_eq!(
            message.render(Language::English),
            "Permanently delete “设置 {count} café”?"
        );
        let diagnostic = LocalizedText::from("设置 {error}".to_owned());
        for locale in [Language::Chinese, Language::English] {
            assert_eq!(diagnostic.render(locale), "设置 {error}");
            assert_eq!(
                locale.text("Untranslated fallback"),
                "Untranslated fallback"
            );
        }
        assert_eq!(Language::English.text("设置"), "Settings");
        assert_eq!(Language::Chinese.text("设置"), "设置");
        assert_eq!(Language::English.text("保存"), "Save");
        assert_eq!(Language::Chinese.text("保存"), "保存");
        assert_eq!(Language::English.text("完成任务成果"), "Complete task work");
        assert_eq!(Language::Chinese.text("完成任务成果"), "完成任务成果");
        assert_eq!(
            Language::English.effort(ThinkingEffort::Default),
            "Model default"
        );
    }
}
