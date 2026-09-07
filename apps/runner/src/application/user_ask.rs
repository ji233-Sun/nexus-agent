use nexus_domain::{
    UserAskAnswer, UserAskAnswerMode, UserAskAnswerValue, UserAskQuestion, UserAskStatus,
};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, MutexGuard},
};
use uuid::Uuid;

#[derive(Clone, Default)]
pub(crate) struct PendingUserAsks(Arc<Mutex<UserAskRegistry>>);

#[derive(Default)]
struct UserAskRegistry {
    requests: HashMap<Uuid, PendingUserAsk>,
    native_request_ids: HashSet<String>,
}

struct PendingUserAsk {
    native_request_id: String,
    questions: Vec<UserAskQuestion>,
    answer_queued: bool,
    answer_sent: bool,
}

pub(crate) struct UserAskInput {
    pub(crate) request_id: Uuid,
    pub(crate) native_request_id: String,
    pub(crate) answers: Vec<UserAskAnswer>,
}

impl PendingUserAsks {
    pub(crate) fn register(
        &self,
        native_request_id: String,
        questions: Vec<UserAskQuestion>,
    ) -> Result<Uuid, String> {
        validate_questions(&questions)?;
        if native_request_id.trim().is_empty() {
            return Err("Harness 返回了空的原生 User Ask 请求 ID。".into());
        }
        let mut registry = self.lock();
        if !registry
            .native_request_ids
            .insert(native_request_id.clone())
        {
            return Err("Harness 重复发送了同一个 User Ask 请求。".into());
        }
        let request_id = Uuid::new_v4();
        registry.requests.insert(
            request_id,
            PendingUserAsk {
                native_request_id,
                questions,
                answer_queued: false,
                answer_sent: false,
            },
        );
        Ok(request_id)
    }

    pub(crate) fn claim_answer(
        &self,
        request_id: Uuid,
        answers: Vec<UserAskAnswer>,
    ) -> Result<UserAskInput, String> {
        let mut registry = self.lock();
        let request = registry
            .requests
            .get_mut(&request_id)
            .ok_or_else(|| "User Ask 请求不存在或已经结束。".to_owned())?;
        if request.answer_queued {
            return Err("User Ask 回答已经提交，请等待 Harness 确认。".into());
        }
        let answers = validate_answers(&request.questions, answers)?;
        request.answer_queued = true;
        Ok(UserAskInput {
            request_id,
            native_request_id: request.native_request_id.clone(),
            answers,
        })
    }

    pub(crate) fn is_answer_queued(&self, request_id: Uuid) -> bool {
        self.lock()
            .requests
            .get(&request_id)
            .is_some_and(|request| request.answer_queued && !request.answer_sent)
    }

    pub(crate) fn mark_answer_sent(&self, request_id: Uuid) -> bool {
        let mut registry = self.lock();
        let Some(request) = registry.requests.get_mut(&request_id) else {
            return false;
        };
        if !request.answer_queued || request.answer_sent {
            return false;
        }
        request.answer_sent = true;
        true
    }

    pub(crate) fn finish(&self, request_id: Uuid) -> bool {
        self.lock().requests.remove(&request_id).is_some()
    }

    pub(crate) fn finish_native(
        &self,
        native_request_id: &str,
        status: UserAskStatus,
    ) -> Option<Uuid> {
        let mut registry = self.lock();
        let request_id = registry.requests.iter().find_map(|(request_id, request)| {
            (request.native_request_id == native_request_id
                && (status != UserAskStatus::Answered || request.answer_sent))
                .then_some(*request_id)
        })?;
        registry.requests.remove(&request_id);
        Some(request_id)
    }

    pub(crate) fn finish_all(&self) -> Vec<Uuid> {
        self.lock().requests.drain().map(|(id, _)| id).collect()
    }

    fn lock(&self) -> MutexGuard<'_, UserAskRegistry> {
        self.0.lock().unwrap_or_else(|error| error.into_inner())
    }
}

fn validate_questions(questions: &[UserAskQuestion]) -> Result<(), String> {
    if questions.is_empty() {
        return Err("Harness 返回了不包含问题的 User Ask 请求。".into());
    }
    let mut question_ids = HashSet::new();
    for question in questions {
        if question.id.trim().is_empty()
            || question.prompt.trim().is_empty()
            || !question_ids.insert(question.id.as_str())
        {
            return Err("Harness 返回了无效或重复的 User Ask 问题。".into());
        }
        match question.answer_mode {
            UserAskAnswerMode::Text if !question.options.is_empty() => {
                return Err("文本问题不能包含选择项。".into());
            }
            UserAskAnswerMode::Choice { .. } => {
                let mut option_ids = HashSet::new();
                if question.options.is_empty()
                    || question.options.iter().any(|option| {
                        option.id.trim().is_empty()
                            || option.label.trim().is_empty()
                            || !option_ids.insert(option.id.as_str())
                    })
                {
                    return Err("Harness 返回了无效或重复的 User Ask 选项。".into());
                }
            }
            UserAskAnswerMode::Text => {}
        }
    }
    Ok(())
}

fn validate_answers(
    questions: &[UserAskQuestion],
    answers: Vec<UserAskAnswer>,
) -> Result<Vec<UserAskAnswer>, String> {
    let mut answers_by_question = HashMap::new();
    for answer in answers {
        let question_id = answer.question_id.clone();
        if question_id.trim().is_empty()
            || answers_by_question.insert(question_id, answer).is_some()
        {
            return Err("User Ask 回答包含空白或重复的问题 ID。".into());
        }
    }
    if answers_by_question.len() != questions.len() {
        return Err("User Ask 回答必须覆盖请求中的每个问题。".into());
    }

    let mut ordered = Vec::with_capacity(questions.len());
    for question in questions {
        let answer = answers_by_question
            .remove(&question.id)
            .ok_or_else(|| "User Ask 回答缺少请求中的问题。".to_owned())?;
        validate_answer(question, &answer.value)?;
        ordered.push(answer);
    }
    Ok(ordered)
}

fn validate_answer(question: &UserAskQuestion, answer: &UserAskAnswerValue) -> Result<(), String> {
    match (&question.answer_mode, answer) {
        (UserAskAnswerMode::Text, UserAskAnswerValue::Text(value)) if !value.trim().is_empty() => {
            Ok(())
        }
        (
            UserAskAnswerMode::Choice {
                multiple,
                allow_custom: _,
            },
            UserAskAnswerValue::Selected(option_ids),
        ) => {
            if option_ids.is_empty() || (!multiple && option_ids.len() != 1) {
                return Err("User Ask 选择数量不符合问题能力。".into());
            }
            let mut selected = HashSet::new();
            if option_ids.iter().any(|option_id| {
                !selected.insert(option_id.as_str())
                    || !question
                        .options
                        .iter()
                        .any(|option| option.id == *option_id)
            }) {
                return Err("User Ask 回答包含无效或重复的选项。".into());
            }
            Ok(())
        }
        (
            UserAskAnswerMode::Choice {
                allow_custom: true, ..
            },
            UserAskAnswerValue::Text(value),
        ) if !value.trim().is_empty() => Ok(()),
        _ => Err("User Ask 回答类型不符合问题能力。".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_domain::UserAskOption;

    fn questions() -> Vec<UserAskQuestion> {
        vec![
            UserAskQuestion {
                id: "target".into(),
                prompt: "Target?".into(),
                answer_mode: UserAskAnswerMode::Choice {
                    multiple: false,
                    allow_custom: false,
                },
                options: vec![UserAskOption {
                    id: "tests".into(),
                    label: "Tests".into(),
                    description: None,
                }],
            },
            UserAskQuestion {
                id: "note".into(),
                prompt: "Note?".into(),
                answer_mode: UserAskAnswerMode::Text,
                options: Vec::new(),
            },
        ]
    }

    #[test]
    fn answers_are_validated_and_reordered_by_question_id() {
        let pending = PendingUserAsks::default();
        let request_id = pending.register("native-1".into(), questions()).unwrap();
        let input = pending
            .claim_answer(
                request_id,
                vec![
                    UserAskAnswer {
                        question_id: "note".into(),
                        value: UserAskAnswerValue::Text("Keep it small".into()),
                    },
                    UserAskAnswer {
                        question_id: "target".into(),
                        value: UserAskAnswerValue::Selected(vec!["tests".into()]),
                    },
                ],
            )
            .unwrap();

        assert_eq!(
            input
                .answers
                .iter()
                .map(|answer| answer.question_id.as_str())
                .collect::<Vec<_>>(),
            ["target", "note"]
        );
        assert_eq!(
            pending.finish_native("native-1", UserAskStatus::Answered),
            None
        );
        assert!(pending.mark_answer_sent(request_id));
        assert_eq!(
            pending.finish_native("native-1", UserAskStatus::Answered),
            Some(request_id)
        );
        assert!(pending.claim_answer(request_id, Vec::new()).is_err());
    }

    #[test]
    fn native_terminal_status_can_expire_an_unanswered_request() {
        let pending = PendingUserAsks::default();
        let request_id = pending.register("native-1".into(), questions()).unwrap();

        assert_eq!(
            pending.finish_native("native-1", UserAskStatus::Expired),
            Some(request_id)
        );
        assert!(pending.claim_answer(request_id, Vec::new()).is_err());
    }

    #[test]
    fn invalid_answers_leave_the_request_available_for_correction() {
        let pending = PendingUserAsks::default();
        let request_id = pending.register("native-1".into(), questions()).unwrap();
        assert!(
            pending
                .claim_answer(
                    request_id,
                    vec![UserAskAnswer {
                        question_id: "target".into(),
                        value: UserAskAnswerValue::Selected(vec!["unknown".into()]),
                    }],
                )
                .is_err()
        );
        assert!(
            pending
                .claim_answer(
                    request_id,
                    vec![
                        UserAskAnswer {
                            question_id: "target".into(),
                            value: UserAskAnswerValue::Selected(vec!["tests".into()]),
                        },
                        UserAskAnswer {
                            question_id: "note".into(),
                            value: UserAskAnswerValue::Text("fixed".into()),
                        },
                    ],
                )
                .is_ok()
        );
    }
}
