use std::collections::HashMap;

use crate::core::audit_trail::{AuditEntry, AuditEventType};

pub fn generate_report() -> String {
    let entries = crate::core::audit_trail::load_recent(10000);
    let chain = crate::core::audit_trail::verify_chain();

    let mut report = String::new();
    report.push_str("# lean-ctx Compliance Report\n\n");
    report.push_str(&format!("Generated: {}\n", chrono::Utc::now().to_rfc3339()));
    report.push_str(&format!("Audit Trail Entries: {}\n", entries.len()));
    report.push_str(&format!(
        "Chain Integrity: {}\n\n",
        if chain.valid { "VALID" } else { "BROKEN" }
    ));

    let mut by_agent: HashMap<String, Vec<&AuditEntry>> = HashMap::new();
    for e in &entries {
        by_agent.entry(e.agent_id.clone()).or_default().push(e);
    }

    report.push_str("## Per-Agent Summary\n\n");
    for (agent, agent_entries) in &by_agent {
        let tool_calls = agent_entries
            .iter()
            .filter(|e| matches!(e.event_type, AuditEventType::ToolCall))
            .count();
        let denials = agent_entries
            .iter()
            .filter(|e| matches!(e.event_type, AuditEventType::ToolDenied))
            .count();
        report.push_str(&format!("### Agent: {agent}\n"));
        report.push_str(&format!("- Tool calls: {tool_calls}\n"));
        report.push_str(&format!("- Denials: {denials}\n\n"));
    }

    let security_events: Vec<_> = entries
        .iter()
        .filter(|e| !matches!(e.event_type, AuditEventType::ToolCall))
        .collect();
    report.push_str(&format!(
        "## Security Events ({} total)\n\n",
        security_events.len()
    ));
    for e in security_events.iter().take(50) {
        report.push_str(&format!(
            "- [{}] {:?} tool={} agent={}\n",
            e.timestamp, e.event_type, e.tool, e.agent_id
        ));
    }

    report.push_str("\n\n");
    report.push_str(&crate::core::owasp_alignment::summary());

    report
}
