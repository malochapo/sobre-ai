#[cfg(feature = "dense")]
mod tests {
    use serde_json::Value;

    fn write_file(path: &std::path::Path, content: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir parent");
        }
        std::fs::write(path, content).expect("write file");
    }

    fn parse_json(s: &str) -> Value {
        serde_json::from_str(s).expect("valid json output")
    }

    #[test]
    #[cfg_attr(not(unix), ignore = "import resolution assumes forward-slash paths")]
    fn property_graph_outputs_are_deterministic_and_bounded() {
        let td = tempfile::tempdir().expect("tempdir");
        let root = td.path();

        write_file(
            &root.join("src/a.ts"),
            r#"import { b } from "./b";
export const a = () => b();
"#,
        );
        write_file(
            &root.join("src/b.ts"),
            r#"import { c } from "./c";
export const b = () => c();
"#,
        );
        write_file(&root.join("src/c.ts"), r#"export const c = () => "ok";"#);

        let root_s = root.to_string_lossy().to_string();

        // Build graph (deterministic file ordering + edge insertion), the
        // same scan `sobre_graph action=build` runs.
        sobre::core::graph_index::scan(&root_s);

        // The dependency chain should be deterministic.
        let chain = sobre::tools::sobre_graph_primitives::shortest_path(
            Some("src/a.ts"),
            Some("src/c.ts"),
            &root_s,
            Some("json"),
        );
        let chain_v = parse_json(&chain);
        assert_eq!(chain_v["connected"], true);
        assert_eq!(
            chain_v["path"],
            serde_json::json!(["src/a.ts", "src/b.ts", "src/c.ts"])
        );

        // Impact should be stable-sorted.
        let impact = sobre::tools::sobre_graph_impact::impact(
            Some("src/c.ts"),
            &root_s,
            Some(10),
            Some("json"),
        );
        let impact_v = parse_json(&impact);
        assert_eq!(impact_v["tool"], "sobre_graph");
        assert_eq!(impact_v["action"], "impact");
        assert_eq!(impact_v["affected_files_total"], 2);
        assert_eq!(
            impact_v["affected_files"],
            serde_json::json!(["src/a.ts", "src/b.ts"])
        );
        assert_eq!(impact_v["truncated"], false);
    }
}
