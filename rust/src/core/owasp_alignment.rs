//! Maps lean-ctx security features to the OWASP Top 10 for Agentic Applications (2025).
//! Used by `lean-ctx audit` CLI and `/v1/audit/events` endpoint.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct OwaspMapping {
    pub owasp_id: &'static str,
    pub owasp_title: &'static str,
    pub risk_description: &'static str,
    pub lean_ctx_mitigations: Vec<Mitigation>,
    pub coverage: Coverage,
}

#[derive(Debug, Clone, Serialize)]
pub struct Mitigation {
    pub feature: &'static str,
    pub module: &'static str,
    pub description: &'static str,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
pub enum Coverage {
    Full,
    Partial,
    Minimal,
}

pub fn alignment() -> Vec<OwaspMapping> {
    vec![
        OwaspMapping {
            owasp_id: "OWASP-AGENT-01",
            owasp_title: "Excessive Agency",
            risk_description: "Agent performs actions beyond intended scope or without proper authorization",
            lean_ctx_mitigations: vec![Mitigation {
                feature: "Shell Gate",
                module: "core/shell_gate/mod.rs",
                description: "Hard blocks on the constructs that hide a command from inspection (eval, exec, source, command substitution at command position)",
            }],
            coverage: Coverage::Full,
        },
        OwaspMapping {
            owasp_id: "OWASP-AGENT-02",
            owasp_title: "Prompt Injection",
            risk_description: "Malicious instructions injected via data processed by the agent",
            lean_ctx_mitigations: vec![
                Mitigation {
                    feature: "Context Compression",
                    module: "core/terse/mod.rs",
                    description: "Deterministic compression reduces attack surface in injected content",
                },
                Mitigation {
                    feature: "Secret Detection",
                    module: "core/secret_detection.rs",
                    description: "Pre-read scanning detects and optionally redacts sensitive patterns",
                },
                Mitigation {
                    feature: "I/O Boundary",
                    module: "core/io_boundary.rs",
                    description: "Content filtering and secret-like path blocking before agent consumption",
                },
            ],
            coverage: Coverage::Partial,
        },
        OwaspMapping {
            owasp_id: "OWASP-AGENT-03",
            owasp_title: "Sensitive Information Disclosure",
            risk_description: "Agent exposes confidential data through outputs or tool interactions",
            lean_ctx_mitigations: vec![
                Mitigation {
                    feature: "PathJail",
                    module: "core/pathjail.rs",
                    description: "Filesystem jail prevents reads outside project root",
                },
                Mitigation {
                    feature: "I/O Boundary",
                    module: "core/io_boundary.rs",
                    description: "Secret-like path detection (.env, .ssh, credentials)",
                },
                Mitigation {
                    feature: "Secret Detection",
                    module: "core/secret_detection.rs",
                    description: "Regex-based detection of API keys, tokens, passwords in file content",
                },
                Mitigation {
                    feature: "Proxy Header Allowlist",
                    module: "proxy/forward/headers.rs",
                    description: "Prevents leaking Set-Cookie and other sensitive headers",
                },
                Mitigation {
                    feature: "Memory Boundary",
                    module: "core/memory_boundary.rs",
                    description: "Cross-project access control with audit trail",
                },
            ],
            coverage: Coverage::Full,
        },
        OwaspMapping {
            owasp_id: "OWASP-AGENT-04",
            owasp_title: "Denial of Service",
            risk_description: "Agent overwhelms system resources or causes service disruption",
            lean_ctx_mitigations: vec![
                Mitigation {
                    feature: "Rate Limiter",
                    module: "core/a2a/rate_limiter.rs",
                    description: "Per-agent per-tool rate limiting",
                },
                Mitigation {
                    feature: "Memory Guard",
                    module: "core/config/memory.rs",
                    description: "RAM usage caps and idle cleanup",
                },
                Mitigation {
                    feature: "Loop Detection",
                    module: "core/loop_detection.rs",
                    description: "Detects and throttles repetitive tool call patterns",
                },
                Mitigation {
                    feature: "Tool Timeout",
                    module: "engine/mod.rs",
                    description: "120s timeout on tool execution prevents indefinite hangs",
                },
            ],
            coverage: Coverage::Full,
        },
        OwaspMapping {
            owasp_id: "OWASP-AGENT-05",
            owasp_title: "Supply Chain Vulnerabilities",
            risk_description: "Compromised tools, plugins, or dependencies affect agent behavior",
            lean_ctx_mitigations: vec![
                Mitigation {
                    feature: "Audit Trail",
                    module: "core/audit_trail.rs",
                    description: "SHA-256 chained append-only log of all tool calls and security events",
                },
                Mitigation {
                    feature: "Agent Identity",
                    module: "core/agent_identity.rs",
                    description: "Per-agent Ed25519 keypairs for cryptographic identity",
                },
            ],
            coverage: Coverage::Partial,
        },
        OwaspMapping {
            owasp_id: "OWASP-AGENT-06",
            owasp_title: "Insufficient Logging and Monitoring",
            risk_description: "Lack of visibility into agent actions and security events",
            lean_ctx_mitigations: vec![
                Mitigation {
                    feature: "Audit Trail",
                    module: "core/audit_trail.rs",
                    description: "Every tool call logged with agent ID, input hash, output tokens",
                },
                Mitigation {
                    feature: "Compliance Reports",
                    module: "cli/audit_report.rs",
                    description: "CLI command to generate aggregated compliance reports",
                },
                Mitigation {
                    feature: "Context OS Events",
                    module: "core/context_os/context_bus.rs",
                    description: "Append-only SQLite event log of tool calls and session \n                                  mutations, with an in-process broadcast for live readers",
                },
                Mitigation {
                    feature: "Proxy Metrics",
                    module: "proxy/metrics.rs",
                    description: "Atomic counters for requests, tokens saved, bytes compressed",
                },
            ],
            coverage: Coverage::Full,
        },
        OwaspMapping {
            owasp_id: "OWASP-AGENT-07",
            owasp_title: "Insecure Code Execution",
            risk_description: "Agent executes arbitrary or unsafe code without proper sandboxing",
            lean_ctx_mitigations: vec![
                Mitigation {
                    feature: "Sandbox Level 0",
                    module: "core/sandbox.rs",
                    description: "Subprocess isolation with env_clear and timeout",
                },
                Mitigation {
                    feature: "Sandbox Level 1 (macOS)",
                    module: "core/sandbox_seatbelt.rs",
                    description: "OS-level Seatbelt profiles restricting filesystem and network",
                },
                Mitigation {
                    feature: "Sandbox Level 1 (Linux)",
                    module: "core/sandbox_landlock.rs",
                    description: "Landlock LSM restricting filesystem access",
                },
                Mitigation {
                    feature: "Command Blocklist",
                    module: "core/shell_gate/mod.rs",
                    description: "Dangerous command patterns blocked before execution",
                },
            ],
            coverage: Coverage::Full,
        },
        OwaspMapping {
            owasp_id: "OWASP-AGENT-08",
            owasp_title: "Broken Access Control",
            risk_description: "Agent accesses resources or performs actions beyond its permissions",
            lean_ctx_mitigations: vec![
                Mitigation {
                    feature: "PathJail",
                    module: "core/pathjail.rs",
                    description: "All path arguments jailed to project root",
                },
                Mitigation {
                    feature: "Boundary Policy",
                    module: "core/memory_boundary.rs",
                    description: "Cross-project access control configurable per policy",
                },
            ],
            coverage: Coverage::Full,
        },
        OwaspMapping {
            owasp_id: "OWASP-AGENT-09",
            owasp_title: "Improper Multi-Agent Orchestration",
            risk_description: "Coordination failures between agents lead to conflicts or data corruption",
            lean_ctx_mitigations: vec![
                Mitigation {
                    feature: "Per-Agent Ledger",
                    module: "core/context_ledger.rs",
                    description: "Isolated context tracking per agent, preventing cross-contamination",
                },
                Mitigation {
                    feature: "Agent Registry",
                    module: "core/agents.rs",
                    description: "HTTP-backed registration with heartbeat and auto-deregistration",
                },
                Mitigation {
                    feature: "Atomic Writes",
                    module: "core/context_ledger.rs",
                    description: "Crash-safe temp+rename writes for all JSON stores",
                },
            ],
            coverage: Coverage::Full,
        },
        OwaspMapping {
            owasp_id: "OWASP-AGENT-10",
            owasp_title: "Insufficient Governance",
            risk_description: "Lack of organizational policies and controls over agent behavior",
            lean_ctx_mitigations: vec![
                Mitigation {
                    feature: "Compliance Reports",
                    module: "cli/audit_report.rs",
                    description: "Aggregated reports: reads, compressions, denials, budget usage",
                },
                Mitigation {
                    feature: "Auto-Reroot Protection",
                    module: "tools/server_paths.rs",
                    description: "Opt-in control over project root changes, audited",
                },
                Mitigation {
                    feature: "Config-Driven",
                    module: "core/config/mod.rs",
                    description: "All security features configurable via config.toml",
                },
            ],
            coverage: Coverage::Full,
        },
    ]
}

/// Returns a compact summary suitable for CLI output.
pub fn summary() -> String {
    let mappings = alignment();
    let mut out = String::from("OWASP Top 10 for Agentic Applications — lean-ctx Alignment\n");
    out.push_str(&"=".repeat(60));
    out.push('\n');
    for m in &mappings {
        let icon = match m.coverage {
            Coverage::Full => "●",
            Coverage::Partial => "◐",
            Coverage::Minimal => "○",
        };
        out.push_str(&format!(
            "\n{icon} {} — {}\n  Mitigations: {}\n",
            m.owasp_id,
            m.owasp_title,
            m.lean_ctx_mitigations
                .iter()
                .map(|m| m.feature)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let full = mappings
        .iter()
        .filter(|m| m.coverage == Coverage::Full)
        .count();
    let partial = mappings
        .iter()
        .filter(|m| m.coverage == Coverage::Partial)
        .count();
    out.push_str(&format!(
        "\nCoverage: {full}/10 Full, {partial}/10 Partial\n"
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alignment_covers_all_ten() {
        let a = alignment();
        assert_eq!(a.len(), 10);
        for (i, m) in a.iter().enumerate() {
            assert_eq!(m.owasp_id, format!("OWASP-AGENT-{:02}", i + 1));
            assert!(!m.lean_ctx_mitigations.is_empty());
        }
    }

    /// Every `module:` in this file must name a real file under `rust/src`.
    ///
    /// These mappings are a hand-written table of compliance claims with no
    /// link to the code they describe, so a deleted module leaves its
    /// mitigation standing and the file keeps asserting a control that no
    /// longer exists. Twice already: `Shell Allowlist — opt-in command
    /// allowlist restricting which binaries agents can execute` survived the
    /// allowlist's removal, and `TaskStore File Locks` outlived
    /// `core/a2a/task.rs`.
    ///
    /// The path is only the cheap half of the check. A mitigation whose file
    /// still exists can still describe something the file no longer does —
    /// verify the *description* by hand when you touch the module it names.
    #[test]
    fn every_mitigation_names_a_real_module() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut missing: Vec<String> = Vec::new();
        for mapping in alignment() {
            for m in &mapping.lean_ctx_mitigations {
                if !src.join(m.module).exists() {
                    missing.push(format!(
                        "{} / {} -> src/{}",
                        mapping.owasp_id, m.feature, m.module
                    ));
                }
            }
        }
        assert!(
            missing.is_empty(),
            "{} OWASP mitigation(s) name a module that does not exist: {missing:#?}\n\
             Either the path drifted (a module became a directory: `foo.rs` -> \
             `foo/mod.rs`) or the subject was deleted, in which case delete the \
             mitigation rather than repointing it — an alignment table that \
             claims a control we removed is worse than one that claims less.",
            missing.len(),
        );
    }

    #[test]
    fn summary_contains_all_ids() {
        let s = summary();
        for i in 1..=10 {
            assert!(s.contains(&format!("OWASP-AGENT-{i:02}")));
        }
    }
}
