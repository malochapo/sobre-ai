#[test]
fn dispatch_inner_pre_resolves_paths() {
    let dispatch_mod = include_str!("../../src/server/dispatch/mod.rs");
    assert!(
        dispatch_mod.contains("self.resolve_path(raw)"),
        "dispatch_inner must resolve paths for registry-dispatched tools"
    );

    for key in ["path", "project_root", "root"] {
        assert!(
            dispatch_mod.contains(&format!("\"{key}\"")),
            "dispatch_inner must pre-resolve the '{key}' parameter"
        );
    }
}

#[test]
fn registry_tools_use_resolved_path() {
    let registry_tools = [
        (
            "sobre_tree",
            include_str!("../../src/tools/registered/sobre_tree.rs"),
        ),
        (
            "sobre_outline",
            include_str!("../../src/tools/registered/sobre_outline.rs"),
        ),
        (
            "sobre_review",
            include_str!("../../src/tools/registered/sobre_review.rs"),
        ),
    ];
    for (name, src) in registry_tools {
        assert!(
            src.contains("resolved_path(") || src.contains("resolve_tool_paths("),
            "{name}: registry tool must use ctx.resolved_path() or resolve_tool_paths() for path access"
        );
    }
}
