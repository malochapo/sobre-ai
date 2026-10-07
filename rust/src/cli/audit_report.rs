use std::collections::HashMap;

use crate::core::audit_trail::AuditEntry;

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
        report.push_str(&format!("### Agent: {agent}\n"));
        report.push_str(&format!("- Tool calls: {}\n\n", agent_entries.len()));
    }

    report
}
