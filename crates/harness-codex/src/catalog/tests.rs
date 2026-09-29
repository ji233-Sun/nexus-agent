use super::*;

#[test]
fn model_page_preserves_ids_defaults_and_all_effort_values() {
    let result = serde_json::json!({
        "data": [
            {
                "id": "gpt-visible",
                "displayName": "GPT Visible",
                "hidden": false,
                "isDefault": true,
                "defaultReasoningEffort": "low",
                "supportedReasoningEfforts": [
                    {"reasoningEffort": "low", "description": "Fast"},
                    {"reasoningEffort": "ultra", "description": "Ultra"}
                ]
            },
            {
                "id": "gpt-hidden",
                "displayName": "GPT Hidden",
                "hidden": true,
                "supportedReasoningEfforts": []
            }
        ],
        "nextCursor": "next-page"
    });

    let (models, cursor) = parse_model_page(&result).unwrap();

    assert_eq!(cursor.as_deref(), Some("next-page"));
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, "gpt-visible");
    assert_eq!(models[0].display_name, "GPT Visible");
    assert_eq!(models[0].provider, None);
    assert!(models[0].is_default);
    assert_eq!(
        models[0].default_reasoning_effort,
        Some(ThinkingEffort::Low)
    );
    assert_eq!(
        models[0].supported_reasoning_efforts[1].effort,
        ThinkingEffort::Ultra
    );
}
