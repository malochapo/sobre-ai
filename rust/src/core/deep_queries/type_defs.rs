//! Type definition and export extraction from AST nodes.

#[cfg(feature = "tree-sitter")]
use tree_sitter::Node;

#[cfg(feature = "tree-sitter")]
use super::types::TypeDef;
#[cfg(feature = "tree-sitter")]
use super::{find_child_by_kind, node_text};

// ---------------------------------------------------------------------------
// Type Definitions
// ---------------------------------------------------------------------------

#[cfg(feature = "tree-sitter")]
pub(super) fn extract_types(root: Node, src: &str, ext: &str) -> Vec<TypeDef> {
    let mut types = Vec::new();
    // Pre-order walk using a heap stack instead of native recursion so a deep AST cannot overflow the worker
    // thread (SIGABRT). Children are reversed so they pop left-to-right.
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if let Some(td) = match_type_def(node, src, ext) {
            types.push(td);
        }

        let mark = stack.len();
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            stack.push(child);
        }
        stack[mark..].reverse();
    }
    types
}

#[cfg(feature = "tree-sitter")]
fn match_type_def(node: Node, src: &str, ext: &str) -> Option<TypeDef> {
    let name = match ext {
        "ts" | "tsx" | "js" | "jsx" => match_type_def_ts(node, src)?,
        "rs" => match_type_def_rust(node, src)?,
        "py" => match_type_def_python(node, src)?,
        "go" => match_type_def_go(node, src)?,
        "java" => match_type_def_java(node, src)?,
        "kt" | "kts" => match_type_def_kotlin(node, src)?,
        "gd" => match_type_def_gdscript(node, src)?,
        "cs" => match_type_def_csharp(node, src)?,
        "luau" => match_type_def_luau(node, src)?,
        _ => return None,
    };

    // Namespace is derived from the AST for C# and Kotlin (both carry an
    // explicit package/namespace declaration). Go's package is the file's
    // directory and is injected path-side in `type_ref_edges`; all other
    // languages stay `None` and fall back to the global resolution path.
    let namespace = match ext {
        "cs" => csharp_namespace_for(node, src),
        "kt" | "kts" => kotlin_namespace_for(node, src),
        _ => None,
    };

    Some(TypeDef {
        name,
        line: node.start_position().row + 1,
        end_line: node.end_position().row + 1,
        namespace,
    })
}

/// The namespace a C# type declaration lives in.
///
/// - Block-scoped: every `namespace_declaration` ancestor's `name` joined
///   outer→inner (handles nesting and dotted names: `namespace A.B { … }`).
/// - File-scoped (`namespace A.B;`): the declaration has no body, so the type
///   is a *sibling* of the `file_scoped_namespace_declaration` under the
///   compilation unit — found by scanning the root's direct children.
///
/// Returns `None` for types in the global namespace.
#[cfg(feature = "tree-sitter")]
fn csharp_namespace_for(node: Node, src: &str) -> Option<String> {
    // 1) Block-scoped namespaces: collect ancestors inner→outer, then reverse.
    let mut parts: Vec<String> = Vec::new();
    let mut current = node.parent();
    while let Some(p) = current {
        if p.kind() == "namespace_declaration"
            && let Some(name) = p.child_by_field_name("name")
        {
            parts.push(node_text(name, src).trim().to_string());
        }
        current = p.parent();
    }
    if !parts.is_empty() {
        parts.reverse();
        return Some(parts.join("."));
    }

    // 2) File-scoped namespace: a direct child of the compilation unit.
    let mut root = node;
    while let Some(p) = root.parent() {
        root = p;
    }
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        if child.kind() == "file_scoped_namespace_declaration"
            && let Some(name) = child.child_by_field_name("name")
        {
            return Some(node_text(name, src).trim().to_string());
        }
    }
    None
}

/// The Kotlin package a type declaration lives in.
///
/// A file declares its package once via a `package_header` (`package a.b.c`),
/// a direct child of the source-file root. The package applies to every type
/// in the file, so a single root scan suffices. Returns `None` for the default
/// (unnamed) package.
#[cfg(feature = "tree-sitter")]
fn kotlin_namespace_for(node: Node, src: &str) -> Option<String> {
    let mut root = node;
    while let Some(p) = root.parent() {
        root = p;
    }
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        if child.kind() == "package_header" {
            let name = find_child_by_kind(child, "qualified_identifier")
                .or_else(|| find_child_by_kind(child, "identifier"))?;
            let text = node_text(name, src).trim();
            return (!text.is_empty()).then(|| text.to_string());
        }
    }
    None
}

#[cfg(feature = "tree-sitter")]
fn match_type_def_ts(node: Node, src: &str) -> Option<String> {
    let name = match node.kind() {
        "class_declaration" | "abstract_class_declaration" => {
            find_child_by_kind(node, "type_identifier")
                .or_else(|| find_child_by_kind(node, "identifier"))?
        }
        "interface_declaration" | "type_alias_declaration" => {
            find_child_by_kind(node, "type_identifier")?
        }
        "enum_declaration" => find_child_by_kind(node, "identifier")?,
        _ => return None,
    };
    Some(node_text(name, src).to_string())
}

#[cfg(feature = "tree-sitter")]
fn match_type_def_rust(node: Node, src: &str) -> Option<String> {
    match node.kind() {
        "struct_item" | "enum_item" | "trait_item" | "type_item" => {
            let name = find_child_by_kind(node, "type_identifier")?;
            Some(node_text(name, src).to_string())
        }
        _ => None,
    }
}

#[cfg(feature = "tree-sitter")]
fn match_type_def_python(node: Node, src: &str) -> Option<String> {
    if node.kind() != "class_definition" {
        return None;
    }
    let name = find_child_by_kind(node, "identifier")?;
    Some(node_text(name, src).to_string())
}

#[cfg(feature = "tree-sitter")]
fn match_type_def_go(node: Node, src: &str) -> Option<String> {
    if node.kind() != "type_spec" {
        return None;
    }
    let name = find_child_by_kind(node, "type_identifier")?;
    Some(node_text(name, src).to_string())
}

#[cfg(feature = "tree-sitter")]
fn match_type_def_java(node: Node, src: &str) -> Option<String> {
    match node.kind() {
        "class_declaration"
        | "interface_declaration"
        | "enum_declaration"
        | "record_declaration"
        | "annotation_type_declaration" => {
            let name = find_child_by_kind(node, "identifier")?;
            Some(node_text(name, src).to_string())
        }
        _ => None,
    }
}

#[cfg(feature = "tree-sitter")]
fn match_type_def_kotlin(node: Node, src: &str) -> Option<String> {
    let field = match node.kind() {
        "class_declaration" | "object_declaration" => "name",
        "type_alias" => "type",
        _ => return None,
    };
    let name = node
        .child_by_field_name(field)
        .or_else(|| find_child_by_kind(node, "identifier"))?;
    Some(node_text(name, src).to_string())
}

#[cfg(feature = "tree-sitter")]
fn match_type_def_csharp(node: Node, src: &str) -> Option<String> {
    if !matches!(
        node.kind(),
        "class_declaration"
            | "interface_declaration"
            | "struct_declaration"
            | "enum_declaration"
            | "record_declaration"
    ) {
        return None;
    }
    // Every C# type declaration exposes its identifier via the `name` field.
    let name = node
        .child_by_field_name("name")
        .or_else(|| find_child_by_kind(node, "identifier"))?;
    Some(node_text(name, src).to_string())
}

/// Luau type aliases: `type X = …` and `export type X = …`.
#[cfg(feature = "tree-sitter")]
fn match_type_def_luau(node: Node, src: &str) -> Option<String> {
    if node.kind() != "type_definition" {
        return None;
    }
    let name = node
        .child_by_field_name("name")
        .or_else(|| find_child_by_kind(node, "identifier"))?;
    Some(node_text(name, src).to_string())
}

#[cfg(feature = "tree-sitter")]
fn match_type_def_gdscript(node: Node, src: &str) -> Option<String> {
    match node.kind() {
        // `class_name X` (script-level global) and inner `class X:` both define a class.
        "class_name_statement" | "class_definition" | "enum_definition" => {
            let name = find_child_by_kind(node, "name")?;
            Some(node_text(name, src).to_string())
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Exports
// ---------------------------------------------------------------------------

#[cfg(feature = "tree-sitter")]
pub(super) fn extract_exports(root: Node, src: &str, ext: &str) -> Vec<String> {
    let mut exports = Vec::new();
    crate::core::ast_walk::for_each_descendant(root, |node| {
        if is_exported_node(node, src, ext)
            && let Some(name) = get_declaration_name(node, src)
        {
            exports.push(name);
        }
    });
    exports
}

#[cfg(feature = "tree-sitter")]
fn is_exported_node(node: Node, src: &str, ext: &str) -> bool {
    match ext {
        "ts" | "tsx" | "js" | "jsx" => {
            node.kind() == "export_statement"
                || node
                    .parent()
                    .is_some_and(|p| p.kind() == "export_statement")
        }
        "rs" => node_text(node, src).trim_start().starts_with("pub "),
        "go" => {
            if let Some(name) = get_declaration_name(node, src) {
                name.starts_with(char::is_uppercase)
            } else {
                false
            }
        }
        "java" => node_text(node, src).trim_start().starts_with("public "),
        "kt" | "kts" => kotlin_declaration_exported(node, src),
        // C# top-level types default to `internal`; only an explicit `public`
        // modifier makes a declaration part of the cross-assembly public surface.
        "cs" => csharp_node_is_public(node, src),
        // GDScript has no visibility keyword; the `_name` convention marks privates.
        "gd" => find_child_by_kind(node, "name")
            .is_some_and(|name| !node_text(name, src).starts_with('_')),
        // Luau type aliases are module-local unless declared `export type`. The
        // `export` keyword may sit inside the node or as a preceding token.
        "luau" => {
            node_text(node, src).trim_start().starts_with("export")
                || src[..node.start_byte()]
                    .trim_end()
                    .rsplit(|c: char| !(c.is_alphanumeric() || c == '_'))
                    .next()
                    .is_some_and(|w| w == "export")
        }
        "py" => {
            if let Some(name) = get_declaration_name(node, src) {
                !name.starts_with('_')
            } else {
                false
            }
        }
        _ => false,
    }
}

#[cfg(feature = "tree-sitter")]
fn get_declaration_name(node: Node, src: &str) -> Option<String> {
    for kind in &[
        "identifier",
        "type_identifier",
        "property_identifier",
        "field_identifier",
    ] {
        if let Some(name_node) = find_child_by_kind(node, kind) {
            return Some(node_text(name_node, src).to_string());
        }
    }
    None
}

/// A C# declaration is "exported" when it carries a `public` access modifier.
/// Modifiers are direct `modifier` children, so attribute lists (`[Attr]`) and
/// leading trivia do not interfere with the check.
#[cfg(feature = "tree-sitter")]
fn csharp_node_is_public(node: Node, src: &str) -> bool {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "modifier" && node_text(child, src).trim() == "public" {
            return true;
        }
    }
    false
}

#[cfg(feature = "tree-sitter")]
fn kotlin_declaration_exported(node: Node, src: &str) -> bool {
    if let Some(modifiers) = find_child_by_kind(node, "modifiers") {
        !node_text(modifiers, src).contains("private")
    } else {
        !node_text(node, src).contains("private")
    }
}
