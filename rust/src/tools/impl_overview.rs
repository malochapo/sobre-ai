use crate::core::graph_provider::{self, GraphProvider};

/// The session-start project overview: structure by directory and graph
/// hotspots. While the graph is still being indexed, a partial overview
/// built from what is on disk now.
pub(crate) fn handle(path: Option<&str>) -> String {
    let project_root = path.map_or_else(|| ".".to_string(), std::string::ToString::to_string);

    let Some(open) = graph_provider::open_or_build(&project_root) else {
        crate::core::index_orchestrator::ensure_all_background(&project_root);
        return partial_overview(&project_root);
    };
    let gp = &open;

    let mut output = Vec::new();
    let last_scan = gp.last_scan();
    let scan_age = chrono::NaiveDateTime::parse_from_str(&last_scan, "%Y-%m-%d %H:%M:%S")
        .ok()
        .map(|t| {
            let elapsed = chrono::Local::now().naive_local().signed_duration_since(t);
            if elapsed.num_hours() < 1 {
                format!("{}m ago", elapsed.num_minutes())
            } else if elapsed.num_hours() < 24 {
                format!("{}h ago", elapsed.num_hours())
            } else {
                format!("{}d ago", elapsed.num_days())
            }
        })
        .unwrap_or_default();
    let scan_info = if scan_age.is_empty() {
        String::new()
    } else {
        format!("  scanned {scan_age}")
    };
    // The header counts file-level (import/type_ref) edges; the
    // function-level call graph is a separate artifact. When one has been
    // built (the sobre_graph call actions persist it lazily), surface it too —
    // otherwise a single-file project reads "0 edges" while it has call edges.
    let call_edges =
        crate::core::call_graph::CallGraph::load(&project_root).map_or(0, |g| g.edges.len());
    let call_info = if call_edges > 0 {
        format!("  {call_edges} call edges")
    } else {
        String::new()
    };
    output.push(format!(
        "PROJECT OVERVIEW  {} files  {} edges{call_info}{scan_info}",
        gp.file_count(),
        gp.edge_count().unwrap_or(0)
    ));
    output.push(String::new());

    let mut by_dir: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();

    for path in gp.file_paths() {
        let dir = std::path::Path::new(&path)
            .parent()
            .map_or_else(|| ".".to_string(), |p| p.to_string_lossy().to_string());
        by_dir.entry(dir).or_default().push(short_path(&path));
    }

    for (dir, files) in &by_dir {
        let dir_display = if dir.len() > 50 {
            let start = truncate_start_char_boundary(dir, 47);
            format!("...{}", &dir[start..])
        } else {
            dir.clone()
        };

        if files.len() <= 5 {
            output.push(format!("{dir_display}/  {}", files.join(" ")));
        } else {
            output.push(format!(
                "{dir_display}/  {} +{} more",
                files[..3].join(" "),
                files.len() - 3
            ));
        }
    }

    append_graph_hotspots_section(&mut output, &project_root, gp);

    output.join("\n")
}

fn append_graph_hotspots_section(output: &mut Vec<String>, project_root: &str, gp: &GraphProvider) {
    let rows = graph_hotspot_rows(project_root, gp);
    if rows.is_empty() {
        return;
    }
    let n = rows.len();
    output.push(String::new());
    output.push(format!("[graph: {n} architectural hotspots]"));
    for (path, imp, cal) in rows {
        let p = short_path(&path);
        if cal > 0 {
            output.push(format!("  {p} ({imp} imports, {cal} calls)"));
        } else {
            output.push(format!("  {p} ({imp} imports)"));
        }
    }
}

fn graph_hotspot_rows(project_root: &str, gp: &GraphProvider) -> Vec<(String, usize, usize)> {
    if let Ok(graph) = crate::core::property_graph::CodeGraph::open(project_root) {
        let sql = "
            WITH edge_files AS (
              SELECT e.kind AS kind, ps.path AS fp
              FROM edges e
              JOIN nodes ns ON e.source_id = ns.id
              JOIN paths ps ON ps.id = ns.file_id
              WHERE e.kind IN ('imports', 'calls')
              UNION ALL
              SELECT e.kind, pt.path
              FROM edges e
              JOIN nodes nt ON e.target_id = nt.id
              JOIN paths pt ON pt.id = nt.file_id
              WHERE e.kind IN ('imports', 'calls')
            )
            SELECT fp,
                   SUM(CASE WHEN kind = 'imports' THEN 1 ELSE 0 END) AS imp,
                   SUM(CASE WHEN kind = 'calls' THEN 1 ELSE 0 END) AS cal
            FROM edge_files
            GROUP BY fp
            ORDER BY (imp + cal) DESC
            LIMIT 5
        ";
        let conn = graph.connection();
        if let Ok(mut stmt) = conn.prepare(sql) {
            let mapped = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)? as usize,
                    row.get::<_, i64>(2)? as usize,
                ))
            });
            if let Ok(iter) = mapped {
                let collected: Vec<_> = iter.filter_map(std::result::Result::ok).collect();
                if !collected.is_empty() {
                    return collected;
                }
            }
        }
    }
    import_hotspots_from_edges(gp, 5)
}

fn import_hotspots_from_edges(gp: &GraphProvider, limit: usize) -> Vec<(String, usize, usize)> {
    use std::collections::HashMap;

    let mut imp: HashMap<String, usize> = HashMap::new();
    for e in gp.edges_by_kind("import") {
        *imp.entry(e.from.clone()).or_insert(0) += 1;
        *imp.entry(e.to.clone()).or_insert(0) += 1;
    }
    let mut v: Vec<(String, usize, usize)> =
        imp.into_iter().map(|(p, c)| (p, c, 0_usize)).collect();
    v.sort_by_key(|x| std::cmp::Reverse(x.1 + x.2));
    v.truncate(limit);
    v
}

fn short_path(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() <= 2 {
        return path.to_string();
    }
    parts[parts.len() - 2..].join("/")
}

/// Find a byte offset at most `max_tail_bytes` from the end of `s`
/// that falls on a valid UTF-8 char boundary.
fn truncate_start_char_boundary(s: &str, max_tail_bytes: usize) -> usize {
    if max_tail_bytes >= s.len() {
        return 0;
    }
    let mut start = s.len() - max_tail_bytes;
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    start
}

/// Builds an immediately-useful overview while the knowledge graph is still
/// being indexed in the background. Instead of only telling the user to
/// "try again in 1-2 minutes", we return what is already available: a shallow
/// directory tree and the detected project markers — plus a note that the
/// richer graph-based view will follow.
fn partial_overview(project_root: &str) -> String {
    let mut out = Vec::new();
    out.push("PROJECT OVERVIEW (partial — knowledge graph indexing in background)".to_string());
    out.push(format!("Project: {project_root}"));

    let markers = detected_markers(project_root);
    if !markers.is_empty() {
        out.push(format!("Markers: {}", markers.join(", ")));
    }
    out.push(String::new());

    // Shallow tree (depth 2) of what's on disk right now; never of a broad
    // root such as the home directory.
    let (tree, _) = if crate::tools::walk_guard::deny_unsafe_walk_root(project_root).is_none() {
        crate::tools::sobre_tree::handle(project_root, 2, false, true)
    } else {
        (String::new(), 0)
    };
    if !tree.trim().is_empty() {
        out.push("STRUCTURE (depth 2):".to_string());
        out.push(tree);
        out.push(String::new());
    }

    out.push(
        "The graph-based overview (structure, hotspots) will be available shortly — \
         call sobre_compose again without a task to get it."
            .to_string(),
    );
    out.join("\n")
}

fn detected_markers(project_root: &str) -> Vec<String> {
    const MARKERS: &[&str] = &[
        ".git",
        "Cargo.toml",
        "package.json",
        "go.mod",
        "pyproject.toml",
        "pom.xml",
        "build.gradle",
    ];
    let root = std::path::Path::new(project_root);
    MARKERS
        .iter()
        .filter(|m| root.join(m).exists())
        .map(|m| (*m).to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_start_ascii() {
        let s = "abcdefghij"; // 10 bytes
        assert_eq!(truncate_start_char_boundary(s, 5), 5);
        assert_eq!(&s[5..], "fghij");
    }

    #[test]
    fn truncate_start_multibyte_chinese() {
        // "文档/examples/extensions/custom-provider-anthropic" = multi-byte prefix
        let s = "文档/examples/extensions/custom-provider-anthropic";
        let start = truncate_start_char_boundary(s, 47);
        assert!(s.is_char_boundary(start));
        let tail = &s[start..];
        assert!(tail.len() <= 47);
    }

    #[test]
    fn truncate_start_all_multibyte() {
        let s = "这是一个很长的中文目录路径用于测试字符边界处理";
        let start = truncate_start_char_boundary(s, 20);
        assert!(s.is_char_boundary(start));
    }

    #[test]
    fn truncate_start_larger_than_string() {
        let s = "short";
        assert_eq!(truncate_start_char_boundary(s, 100), 0);
    }

    #[test]
    fn truncate_start_emoji() {
        let s = "/home/user/🎉🎉🎉/src/components/deeply/nested";
        let start = truncate_start_char_boundary(s, 30);
        assert!(s.is_char_boundary(start));
    }

    /// When a persisted call graph exists, the overview header must
    /// surface its edge count — a single-file project has 0 file-level edges
    /// but may well have call edges, and the two surfaces must not contradict.
    #[test]
    fn overview_header_surfaces_persisted_call_edges() {
        use crate::core::call_graph::{CallEdge, CallGraph};
        use crate::core::graph_index::{FileEntry, ProjectIndex};

        let _lock = crate::core::data_dir::test_env_lock();
        let tmp = tempfile::tempdir().expect("tempdir");
        let data = tempfile::tempdir().expect("data dir");
        crate::test_env::set_var("SOBRE_DATA_DIR", data.path().to_str().unwrap());

        let root = tmp.path();
        std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"demo\"\n").unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/main.rs"),
            "fn greet() {}\nfn main() { greet(); }\n",
        )
        .unwrap();
        let root_str = root.to_str().unwrap();

        // Pre-build a ProjectIndex so open_existing finds it immediately
        // without needing the static BUILD_GATE (which parallel tests may hold).
        let mut idx = ProjectIndex::new(root_str);
        idx.files.insert(
            "src/main.rs".to_string(),
            FileEntry {
                path: "src/main.rs".to_string(),
                hash: "h".to_string(),
                language: "rs".to_string(),
                line_count: 2,
                token_count: 10,
                exports: vec![],
                summary: String::new(),
            },
        );
        idx.save().expect("save pre-built index");

        let mut graph = CallGraph::new(root_str);
        graph.edges.push(CallEdge {
            caller_file: "src/main.rs".into(),
            caller_symbol: "main".into(),
            caller_line: 2,
            callee_name: "greet".into(),
        });
        graph.save().expect("persist call graph");

        let out = handle(Some(root_str));
        assert!(
            !out.contains("(partial"),
            "graph-backed overview must not report as partial"
        );
        assert!(
            out.contains("1 call edges"),
            "overview header must show persisted call edges, got:\n{out}"
        );

        crate::test_env::remove_var("SOBRE_DATA_DIR");
    }
}
