use rmcp::model::{
    GetPromptRequestParams, GetPromptResult, Prompt, PromptArgument, PromptMessage, Role,
};

fn required_arg(name: &str, desc: &str) -> PromptArgument {
    PromptArgument::new(name)
        .with_description(desc)
        .with_required(true)
}

pub fn list_prompts() -> Vec<Prompt> {
    vec![
        Prompt::new(
            "context-focus",
            Some("Set task intent and optimize context for a specific task"),
            Some(vec![required_arg("task", "What you are working on")]),
        ),
        Prompt::new(
            "context-review",
            Some("Review current context state: items, pressure, budget, recommendations"),
            None,
        ),
        Prompt::new(
            "context-budget",
            Some("Set the token budget for this session"),
            Some(vec![required_arg(
                "tokens",
                "Max tokens for the context window",
            )]),
        ),
    ]
}

pub fn get_prompt(
    params: &GetPromptRequestParams,
    ledger: &crate::core::context_ledger::ContextLedger,
) -> Option<GetPromptResult> {
    match params.name.as_str() {
        "context-focus" => {
            let task = params
                .arguments
                .as_ref()
                .and_then(|a| a.get("task"))
                .and_then(|v| v.as_str())
                .unwrap_or("general development");
            Some(get_context_focus(task, ledger))
        }
        "context-review" => Some(get_context_review(ledger)),
        "context-budget" => {
            let tokens = params
                .arguments
                .as_ref()
                .and_then(|a| a.get("tokens"))
                .and_then(|v| v.as_str())
                .unwrap_or("128000");
            Some(get_context_budget(tokens))
        }
        _ => None,
    }
}

fn get_context_focus(
    task: &str,
    ledger: &crate::core::context_ledger::ContextLedger,
) -> GetPromptResult {
    let pressure = ledger.pressure();
    let msg = format!(
        "Focus context on task: {task}\n\
         Current state: {} files, {:.0}% pressure\n\
         Files matching this task's intent targets should be read as 'full', others compressed.",
        ledger.entries.len(),
        pressure.utilization * 100.0,
    );
    GetPromptResult::new(vec![PromptMessage::new_text(Role::Assistant, msg)])
}

fn get_context_review(ledger: &crate::core::context_ledger::ContextLedger) -> GetPromptResult {
    let summary = ledger.format_summary();
    let adjusted = ledger.adjusted_total_saved();
    let bounce_info = match crate::core::bounce_tracker::global().lock() {
        Ok(bt) => bt.format_summary(),
        _ => String::new(),
    };
    let msg =
        format!("Context Review:\n{summary}\nAdjusted savings: {adjusted} tokens\n{bounce_info}");
    GetPromptResult::new(vec![PromptMessage::new_text(Role::Assistant, msg)])
}

fn get_context_budget(tokens: &str) -> GetPromptResult {
    let msg = format!(
        "Set budget: Configure the context window to {tokens} tokens. \
         Use ctx_session(action=\"budget\", value=\"{tokens}\")."
    );
    GetPromptResult::new(vec![PromptMessage::new_text(Role::Assistant, msg)])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two prompts were deleted because their whole body told the agent to
    /// call a tool that no longer exists. Guard the survivors.
    #[test]
    fn prompts_name_only_registered_tools() {
        let ledger = crate::core::context_ledger::ContextLedger::new();
        let mut seen: Vec<(String, String)> = Vec::new();
        for p in list_prompts() {
            let mut text = p.description.clone().unwrap_or_default();
            let mut params = GetPromptRequestParams::new(p.name.clone());
            let mut args = serde_json::Map::new();
            for a in p.arguments.clone().unwrap_or_default() {
                args.insert(a.name.clone(), serde_json::Value::String("x".into()));
            }
            params.arguments = Some(args);
            if let Some(result) = get_prompt(&params, &ledger) {
                text.push_str(&format!("{result:?}"));
            }
            let dead = crate::server::registry::unregistered_tool_mentions(&text);
            assert!(dead.is_empty(), "prompt {} names {dead:?}", p.name);
            seen.push((p.name.clone(), text));
        }
        // The body is read through `Debug`: prove it carries the message text,
        // or this test would pass on an empty string.
        let review = seen
            .iter()
            .find(|(n, _)| n == "context-review")
            .expect("context-review");
        assert!(
            review.1.contains("Context Review"),
            "prompt body not captured: {}",
            review.1
        );
    }

    #[test]
    fn list_returns_three_prompts() {
        let prompts = list_prompts();
        // `context-reset` and `context-pin` were removed: each one's whole
        // body was an instruction to call `ctx_control`, which no longer
        // exists, so nothing truthful was left to say.
        assert_eq!(prompts.len(), 3);
    }

    #[test]
    fn context_focus_has_task_arg() {
        let prompts = list_prompts();
        let focus = prompts.iter().find(|p| p.name == "context-focus").unwrap();
        let args = focus.arguments.as_ref().unwrap();
        assert_eq!(args[0].name, "task");
    }

    #[test]
    fn get_unknown_prompt_returns_none() {
        let ledger = crate::core::context_ledger::ContextLedger::new();
        let params = GetPromptRequestParams::new("unknown-prompt");
        assert!(get_prompt(&params, &ledger).is_none());
    }
}
