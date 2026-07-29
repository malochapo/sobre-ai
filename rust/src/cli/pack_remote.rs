//! Hosted-registry installs for `lean-ctx pack` (GL #406, GH #727).
//!
//! `pack install <ns>/<name>` / `pack update <ns>/<name>`: version resolution
//! against the registry index, sha256-verified download, the standard local
//! import gates, lockfile pinning — and depth-1 resolution of the declared
//! dependencies so one install command yields a complete, reproducible set.

// Inlined from the removed `pack_cmd`: these three helpers were its only
// remaining consumers. `pack_remote` is reached from `addon_deps` — it
// installs an addon's declared dependencies, which is why it outlives the
// `pack` command it was named after.


/// Install every non-optional declared dependency in `deps` (declared by the
/// root package `root_name`, depth 1, GH #727). Already-locked deps present in
/// the store are skipped offline; everything else resolves SemVer against the
/// registry, downloads through the standard verified import path, and is
/// pinned in the lockfile.
///
/// Returns the [`ResolvedDep`]s that actually landed — already-satisfied deps
/// at their **locked** version, freshly resolved ones at the picked version —
/// so the caller can expand `[mcp.env]` `{pack_dir:}` against the exact
/// versions on disk rather than a second, possibly-divergent resolution
/// (GH #727, Finding B). Takes the dependency slice + root name directly so a
/// local `lean-ctx-addon.toml` (no hosted `PackageManifest`) installs the same
/// way (Finding A).
pub(crate) fn install_declared_dependencies(
    deps: &[crate::core::context_package::manifest::PackageDependency],
    root_name: Option<&str>,
    base: &str,
    token: Option<&str>,
    project_root: &str,
    refresh: bool,
) -> Result<Vec<crate::core::context_package::deps::ResolvedDep>, String> {
    use crate::core::context_package::{LocalRegistry, deps, lockfile, remote};

    if deps.iter().all(|d| d.optional) {
        return Ok(Vec::new());
    }
    let registry = LocalRegistry::open()?;
    let root = std::path::Path::new(project_root);
    let mut resolved_deps = Vec::new();

    for dep in deps.iter().filter(|d| !d.optional) {
        if !refresh && let Some(resolved) = deps::already_satisfied(root, &registry, dep) {
            println!(
                "Dependency {}@{} already satisfied (locked, offline).",
                dep.name, resolved.version
            );
            resolved_deps.push(resolved);
            continue;
        }

        let resolved = deps::resolve_one(root_name, dep, base, token)?;
        let (ns, slug) = (&resolved.namespace, &resolved.slug);
        println!(
            "Installing dependency @{ns}/{slug}@{} (declared: `{} {}`)",
            resolved.version, dep.name, dep.version_req
        );

        let info = remote::VersionInfo {
            version: resolved.version.clone(),
            artifact_sha256: resolved.artifact_sha256.clone(),
            yanked: false,
        };
        let bytes = remote::download_verified(base, ns, slug, &info, token)?;
        let tmp = std::env::temp_dir().join(format!(
            "ctxpkg-dep-{}-{ns}-{slug}.ctxpkg",
            std::process::id()
        ));
        std::fs::write(&tmp, &bytes).map_err(|e| format!("stage dependency artifact: {e}"))?;
        let imported = registry.import_from_file(&tmp);
        std::fs::remove_file(&tmp).ok();
        let dep_manifest = imported.map_err(|e| format!("dependency `{}`: {e}", dep.name))?;

        if let Err(e) = lockfile::upsert(
            root,
            lockfile::LockedPackage {
                name: dep_manifest.name.clone(),
                version: dep_manifest.version.clone(),
                artifact_sha256: resolved.artifact_sha256.clone(),
                registry: base.to_string(),
            },
        ) {
            eprintln!("WARNING: could not update ctxpkg.lock: {e}");
        }
        println!(
            "  ✓ {}@{} installed + pinned",
            dep_manifest.name, dep_manifest.version
        );
        resolved_deps.push(resolved);
    }
    Ok(resolved_deps)
}
