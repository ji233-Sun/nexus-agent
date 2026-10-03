mod composer;
mod issues;
mod model_picker;
mod navigation;
mod review;
mod settings;
mod user_ask;

use super::*;
use crate::presenter::tests::fixture;
use nexus_domain::{ModelReasoningEffort, UserAskOption, UserAskQuestion};
use nexus_protocol::Event;

fn omp_model(provider: &str, id: &str) -> ModelDescriptor {
    ModelDescriptor {
        source: nexus_domain::ModelSource::OmpCli,
        availability: nexus_domain::ModelAvailability::Available,
        id: id.into(),
        display_name: "Shared Model".into(),
        provider: Some(provider.into()),
        is_default: false,
        supported_reasoning_efforts: vec![ModelReasoningEffort {
            effort: ThinkingEffort::XHigh,
            description: String::new(),
        }],
        default_reasoning_effort: None,
    }
}

fn user_ask_ui_questions() -> Vec<UserAskQuestion> {
    vec![
        UserAskQuestion {
            id: "target".into(),
            prompt: "选择修改范围".into(),
            answer_mode: UserAskAnswerMode::Choice {
                multiple: false,
                allow_custom: false,
            },
            options: vec![
                UserAskOption {
                    id: "library".into(),
                    label: "核心库".into(),
                    description: Some("只修改共享 crate".into()),
                },
                UserAskOption {
                    id: "workspace".into(),
                    label: "整个工作区".into(),
                    description: None,
                },
            ],
        },
        UserAskQuestion {
            id: "checks".into(),
            prompt: "选择需要执行的检查".into(),
            answer_mode: UserAskAnswerMode::Choice {
                multiple: true,
                allow_custom: false,
            },
            options: vec![
                UserAskOption {
                    id: "tests".into(),
                    label: "测试".into(),
                    description: None,
                },
                UserAskOption {
                    id: "clippy".into(),
                    label: "Clippy".into(),
                    description: None,
                },
            ],
        },
        UserAskQuestion {
            id: "scope".into(),
            prompt: "选择预设或填写其他范围".into(),
            answer_mode: UserAskAnswerMode::Choice {
                multiple: false,
                allow_custom: true,
            },
            options: vec![UserAskOption {
                id: "focused".into(),
                label: "当前模块".into(),
                description: None,
            }],
        },
        UserAskQuestion {
            id: "note".into(),
            prompt: "补充说明".into(),
            answer_mode: UserAskAnswerMode::Text,
            options: Vec::new(),
        },
    ]
}

fn click_debug(cx: &mut gpui::VisualTestContext, selector: &'static str) {
    let point = cx.debug_bounds(selector).unwrap().center();
    cx.simulate_click(point, Default::default());
}
