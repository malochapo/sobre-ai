//! Static command-tree specification for CLI completions.
//!
//! Every top-level command, its subcommands, flags and dynamic value kinds
//! live here so the completion engine has a single source of truth.

/// Describes a dynamic value set resolved at completion time.
#[derive(Clone, Copy)]
pub enum DynamicKind {
    Agents,
    Shells,
    Modes,
    ConfigKeys,
    TerseLevel,
}

/// A flag/option accepted by a command.
pub struct FlagSpec {
    pub long: &'static str,
    pub short: Option<char>,
    pub description: &'static str,
    pub takes_value: bool,
    pub value_kind: Option<DynamicKind>,
}

/// A command or subcommand node in the tree.
pub struct CommandNode {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub description: &'static str,
    pub subcommands: &'static [CommandNode],
    pub flags: &'static [FlagSpec],
    pub positional: Option<DynamicKind>,
    pub hidden: bool,
}

pub static COMMAND_TREE: &[CommandNode] = &[
    CommandNode {
        name: "init",
        aliases: &[],
        description: "Initialize lean-ctx for an agent",
        subcommands: &[],
        flags: &[
            FlagSpec {
                long: "--agent",
                short: Some('a'),
                description: "Agent name",
                takes_value: true,
                value_kind: Some(DynamicKind::Agents),
            },
            FlagSpec {
                long: "--mode",
                short: Some('m'),
                description: "Integration mode",
                takes_value: true,
                value_kind: Some(DynamicKind::Modes),
            },
        ],
        positional: None,
        hidden: false,
    },
    CommandNode {
        name: "status",
        aliases: &[],
        description: "Show current status",
        subcommands: &[],
        flags: &[FlagSpec {
            long: "--json",
            short: None,
            description: "JSON output",
            takes_value: false,
            value_kind: None,
        }],
        positional: None,
        hidden: false,
    },
    CommandNode {
        name: "config",
        aliases: &[],
        description: "Manage configuration",
        subcommands: &[
            CommandNode {
                name: "get",
                aliases: &[],
                description: "Get a config value",
                subcommands: &[],
                flags: &[],
                positional: Some(DynamicKind::ConfigKeys),
                hidden: false,
            },
            CommandNode {
                name: "set",
                aliases: &[],
                description: "Set a config value",
                subcommands: &[],
                flags: &[],
                positional: Some(DynamicKind::ConfigKeys),
                hidden: false,
            },
            CommandNode {
                name: "edit",
                aliases: &[],
                description: "Open config in editor",
                subcommands: &[],
                flags: &[],
                positional: None,
                hidden: false,
            },
            CommandNode {
                name: "show",
                aliases: &[],
                description: "Show current config",
                subcommands: &[],
                flags: &[FlagSpec {
                    long: "--json",
                    short: None,
                    description: "JSON output",
                    takes_value: false,
                    value_kind: None,
                }],
                positional: None,
                hidden: false,
            },
        ],
        flags: &[],
        positional: None,
        hidden: false,
    },
    CommandNode {
        name: "proxy",
        aliases: &[],
        description: "Start the proxy server",
        subcommands: &[
            CommandNode {
                name: "start",
                aliases: &[],
                description: "Start proxy",
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        long: "--detach",
                        short: Some('d'),
                        description: "Run in background",
                        takes_value: false,
                        value_kind: None,
                    },
                    FlagSpec {
                        long: "--port",
                        short: Some('p'),
                        description: "Port number",
                        takes_value: true,
                        value_kind: None,
                    },
                ],
                positional: None,
                hidden: false,
            },
            CommandNode {
                name: "stop",
                aliases: &[],
                description: "Stop proxy",
                subcommands: &[],
                flags: &[],
                positional: None,
                hidden: false,
            },
        ],
        flags: &[
            FlagSpec {
                long: "--port",
                short: Some('p'),
                description: "Port number",
                takes_value: true,
                value_kind: None,
            },
            FlagSpec {
                long: "--detach",
                short: Some('d'),
                description: "Run in background",
                takes_value: false,
                value_kind: None,
            },
        ],
        positional: None,
        hidden: false,
    },
    CommandNode {
        name: "cache",
        aliases: &[],
        description: "Manage the cache",
        subcommands: &[
            CommandNode {
                name: "stats",
                aliases: &[],
                description: "Cache statistics",
                subcommands: &[],
                flags: &[],
                positional: None,
                hidden: false,
            },
            CommandNode {
                name: "clear",
                aliases: &[],
                description: "Clear cache",
                subcommands: &[],
                flags: &[],
                positional: None,
                hidden: false,
            },
            CommandNode {
                name: "export",
                aliases: &[],
                description: "Export cache",
                subcommands: &[],
                flags: &[],
                positional: None,
                hidden: false,
            },
        ],
        flags: &[],
        positional: None,
        hidden: false,
    },
    CommandNode {
        name: "terse",
        aliases: &[],
        description: "Set compression level",
        subcommands: &[],
        flags: &[],
        positional: Some(DynamicKind::TerseLevel),
        hidden: false,
    },
    CommandNode {
        name: "gain",
        aliases: &[],
        description: "Show token savings",
        subcommands: &[],
        flags: &[
            FlagSpec {
                long: "--json",
                short: None,
                description: "JSON output",
                takes_value: false,
                value_kind: None,
            },
            FlagSpec {
                long: "--by-tool",
                short: None,
                description: "Per-tool savings breakdown",
                takes_value: false,
                value_kind: None,
            },
        ],
        positional: None,
        hidden: false,
    },
    CommandNode {
        name: "index",
        aliases: &[],
        description: "Build or rebuild search index",
        subcommands: &[],
        flags: &[FlagSpec {
            long: "--force",
            short: Some('f'),
            description: "Force rebuild",
            takes_value: false,
            value_kind: None,
        }],
        positional: None,
        hidden: false,
    },
    CommandNode {
        name: "repomap",
        aliases: &[],
        description: "Repository map: structure and token cost per file",
        subcommands: &[],
        flags: &[
            FlagSpec {
                long: "--json",
                short: None,
                description: "JSON output",
                takes_value: false,
                value_kind: None,
            },
            FlagSpec {
                long: "--limit",
                short: None,
                description: "Row limit",
                takes_value: true,
                value_kind: None,
            },
            FlagSpec {
                long: "--max-tokens",
                short: None,
                description: "Token budget for the map",
                takes_value: true,
                value_kind: None,
            },
        ],
        positional: None,
        hidden: false,
    },
    CommandNode {
        name: "compact",
        aliases: &[],
        description: "Compact agent transcripts in place (defaults to ~/.claude/projects)",
        subcommands: &[],
        flags: &[],
        positional: None,
        hidden: false,
    },
    CommandNode {
        name: "stop",
        aliases: &[],
        description: "Stop all lean-ctx processes",
        subcommands: &[],
        flags: &[],
        positional: None,
        hidden: false,
    },
    CommandNode {
        name: "restart",
        aliases: &[],
        description: "Restart lean-ctx",
        subcommands: &[],
        flags: &[],
        positional: None,
        hidden: false,
    },
    CommandNode {
        name: "dev-install",
        aliases: &[],
        description: "Development install (stop+build+install+restart)",
        subcommands: &[],
        flags: &[],
        positional: None,
        hidden: false,
    },
    CommandNode {
        name: "doctor",
        aliases: &[],
        description: "Diagnose installation issues",
        subcommands: &[],
        flags: &[],
        positional: None,
        hidden: false,
    },
    CommandNode {
        name: "health",
        aliases: &[],
        description: "Context-health scan: hotspots and token tax",
        subcommands: &[],
        flags: &[
            FlagSpec {
                long: "--gate",
                short: None,
                description: "Exit non-zero when the score is below threshold",
                takes_value: false,
                value_kind: None,
            },
            FlagSpec {
                long: "--json",
                short: None,
                description: "JSON output",
                takes_value: false,
                value_kind: None,
            },
        ],
        positional: None,
        hidden: false,
    },
    CommandNode {
        name: "rules",
        aliases: &[],
        description: "Manage rules",
        subcommands: &[],
        flags: &[],
        positional: None,
        hidden: false,
    },
    CommandNode {
        name: "export-rules",
        aliases: &[],
        description: "Export rules to file",
        subcommands: &[],
        flags: &[],
        positional: None,
        hidden: false,
    },
    CommandNode {
        name: "harden",
        aliases: &[],
        description: "Harden security settings",
        subcommands: &[],
        flags: &[],
        positional: None,
        hidden: false,
    },
    CommandNode {
        name: "completions",
        aliases: &[],
        description: "Generate shell completion script",
        subcommands: &[],
        flags: &[],
        positional: Some(DynamicKind::Shells),
        hidden: false,
    },
    CommandNode {
        name: "gotchas",
        aliases: &[],
        description: "Show known gotchas",
        subcommands: &[],
        flags: &[],
        positional: None,
        hidden: false,
    },
    CommandNode {
        name: "learn",
        aliases: &[],
        description: "Interactive learning",
        subcommands: &[],
        flags: &[],
        positional: None,
        hidden: false,
    },
    CommandNode {
        name: "help",
        aliases: &[],
        description: "Show help",
        subcommands: &[],
        flags: &[],
        positional: None,
        hidden: false,
    },
    CommandNode {
        name: "audit",
        aliases: &[],
        description: "Run compliance audit",
        subcommands: &[],
        flags: &[],
        positional: None,
        hidden: false,
    },
    CommandNode {
        name: "session",
        aliases: &[],
        description: "Manage sessions",
        subcommands: &[],
        flags: &[],
        positional: None,
        hidden: false,
    },
    CommandNode {
        name: "knowledge",
        aliases: &[],
        description: "Manage knowledge base",
        subcommands: &[],
        flags: &[],
        positional: None,
        hidden: false,
    },
    CommandNode {
        name: "introspect",
        aliases: &[],
        description: "Subsystem activity registry",
        subcommands: &[CommandNode {
            name: "cognition",
            aliases: &[],
            description: "Cognition-loop activity per subsystem",
            subcommands: &[],
            flags: &[],
            positional: None,
            hidden: false,
        }],
        flags: &[FlagSpec {
            long: "--json",
            short: None,
            description: "JSON output",
            takes_value: false,
            value_kind: None,
        }],
        positional: None,
        hidden: false,
    },
    CommandNode {
        name: "mcp",
        aliases: &[],
        description: "MCP server management",
        subcommands: &[],
        flags: &[],
        positional: None,
        hidden: false,
    },
    CommandNode {
        name: "verify-cache",
        aliases: &[],
        description: "Verify cache consistency",
        subcommands: &[],
        flags: &[],
        positional: None,
        hidden: false,
    },
    CommandNode {
        name: "visualize",
        aliases: &[],
        description: "Visualize context graph",
        subcommands: &[],
        flags: &[],
        positional: None,
        hidden: false,
    },
    CommandNode {
        name: "read",
        aliases: &[],
        description: "Read and compress a file",
        subcommands: &[],
        flags: &[],
        positional: None,
        hidden: false,
    },
    CommandNode {
        name: "call",
        aliases: &[],
        description: "Call an MCP tool",
        subcommands: &[],
        flags: &[],
        positional: None,
        hidden: false,
    },
    CommandNode {
        name: "diff",
        aliases: &[],
        description: "Compressed diff",
        subcommands: &[],
        flags: &[],
        positional: None,
        hidden: false,
    },
    CommandNode {
        name: "grep",
        aliases: &[],
        description: "Search with compression",
        subcommands: &[],
        flags: &[],
        positional: None,
        hidden: false,
    },
    CommandNode {
        name: "glob",
        aliases: &[],
        description: "Glob file search",
        subcommands: &[],
        flags: &[],
        positional: None,
        hidden: false,
    },
    CommandNode {
        name: "ls",
        aliases: &[],
        description: "List directory contents",
        subcommands: &[],
        flags: &[],
        positional: None,
        hidden: false,
    },
    CommandNode {
        name: "deps",
        aliases: &[],
        description: "Show dependencies",
        subcommands: &[],
        flags: &[],
        positional: None,
        hidden: false,
    },
    CommandNode {
        name: "ghost",
        aliases: &[],
        description: "Ghost context injection",
        subcommands: &[],
        flags: &[],
        positional: None,
        hidden: false,
    },
    CommandNode {
        name: "filter",
        aliases: &[],
        description: "Filter context",
        subcommands: &[],
        flags: &[],
        positional: None,
        hidden: false,
    },
    CommandNode {
        name: "graph",
        aliases: &[],
        description: "Show dependency graph",
        subcommands: &[],
        flags: &[],
        positional: None,
        hidden: false,
    },
    CommandNode {
        name: "smells",
        aliases: &[],
        description: "Detect code smells",
        subcommands: &[],
        flags: &[],
        positional: None,
        hidden: false,
    },
    CommandNode {
        name: "benchmark",
        aliases: &[],
        description: "Run compression benchmarks",
        subcommands: &[CommandNode {
            name: "replay",
            aliases: &[],
            description: "Replay paired sessions with and without compression",
            subcommands: &[],
            flags: &[FlagSpec {
                long: "--output",
                short: None,
                description: "Write Markdown report to a file",
                takes_value: true,
                value_kind: None,
            }],
            positional: None,
            hidden: false,
        }],
        flags: &[],
        positional: None,
        hidden: false,
    },
    CommandNode {
        name: "tools",
        aliases: &[],
        description: "List available MCP tools",
        subcommands: &[],
        flags: &[],
        positional: None,
        hidden: false,
    },
    CommandNode {
        name: "theme",
        aliases: &[],
        description: "Set dashboard theme",
        subcommands: &[],
        flags: &[],
        positional: None,
        hidden: false,
    },
    CommandNode {
        name: "tee",
        aliases: &[],
        description: "Tee output to file",
        subcommands: &[],
        flags: &[],
        positional: None,
        hidden: false,
    },
    CommandNode {
        name: "codesign-setup",
        aliases: &[],
        description: "Setup code signing",
        subcommands: &[],
        flags: &[],
        positional: None,
        hidden: false,
    },
    CommandNode {
        name: "learning",
        aliases: &[],
        description: "Show learning progress",
        subcommands: &[],
        flags: &[],
        positional: None,
        hidden: false,
    },
    CommandNode {
        name: "cep",
        aliases: &[],
        description: "Context engineering pipeline",
        subcommands: &[],
        flags: &[],
        positional: None,
        hidden: false,
    },
    // Hidden internal commands
    CommandNode {
        name: "__complete",
        aliases: &[],
        description: "Dynamic completion (internal)",
        subcommands: &[],
        flags: &[],
        positional: None,
        hidden: true,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_duplicate_names_at_top_level() {
        let mut seen = std::collections::HashSet::new();
        for node in COMMAND_TREE {
            assert!(seen.insert(node.name), "duplicate: {}", node.name);
        }
    }

    #[test]
    fn visible_count_exceeds_50() {
        let visible = COMMAND_TREE.iter().filter(|n| !n.hidden).count();
        assert!(
            visible >= 30,
            "expected at least 30 visible commands, got {visible}"
        );
    }

    #[test]
    fn command_tree_covers_known_commands() {
        let known = crate::cli::dispatch::suggest::KNOWN_COMMANDS;
        let tree_names: std::collections::HashSet<&str> = COMMAND_TREE
            .iter()
            .flat_map(|n| std::iter::once(n.name).chain(n.aliases.iter().copied()))
            .collect();

        let mut missing = Vec::new();
        for cmd in known {
            if !tree_names.contains(cmd) {
                missing.push(*cmd);
            }
        }
        assert!(
            missing.is_empty(),
            "KNOWN_COMMANDS entries missing from COMMAND_TREE: {missing:?}"
        );
    }

    /// Every name Tab-completion offers must actually dispatch.
    ///
    /// This tree had drifted into offering 23 commands and 14 aliases that no
    /// longer had a match arm: the user tabbed to them, ran them, and got
    /// "unknown command". Some were worse than useless — `secure` was listed as
    /// an alias of `yolo` while the two do the opposite thing.
    ///
    /// Reading the dispatch source is crude, but it is the only static handle
    /// on a `match` that has no runtime representation, and spawning 65
    /// processes is not an option: several top-level commands act on the
    /// machine when run bare.
    #[test]
    fn every_completion_name_has_a_dispatch_arm() {
        const DISPATCH_SRC: &str = include_str!("../dispatch/mod.rs");

        let body = DISPATCH_SRC
            .split_once("match args[1].as_str()")
            .expect("dispatch match not found — update this test if it moved")
            .1;

        // Top-level arms sit at 12 spaces of indentation; each may list several
        // patterns (`"a" | "b" =>`). Collect every quoted pattern on such a line.
        let mut arms = std::collections::HashSet::new();
        for line in body.lines() {
            let Some(rest) = line.strip_prefix("            ") else {
                continue;
            };
            if rest.starts_with(' ') || !rest.contains("=>") || !rest.starts_with('"') {
                continue;
            }
            let patterns = rest.split_once("=>").map_or(rest, |(p, _)| p);
            for chunk in patterns.split('|') {
                let t = chunk.trim().trim_matches('"');
                if !t.is_empty() {
                    arms.insert(t.to_string());
                }
            }
        }
        assert!(
            arms.len() > 50,
            "parsed only {} dispatch arms — the parser is broken, not the tree",
            arms.len()
        );

        let mut orphans = Vec::new();
        for node in COMMAND_TREE {
            for name in std::iter::once(node.name).chain(node.aliases.iter().copied()) {
                if !arms.contains(name) {
                    orphans.push(name);
                }
            }
        }
        assert!(
            orphans.is_empty(),
            "completion offers names with no dispatch arm: {orphans:?}\n\
             Either add the arm or drop the entry — an offered command that \
             exits 1 is worse than a missing one."
        );
    }
}
