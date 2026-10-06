use super::super::{
    REDIRECT_SCRIPT_GENERIC, generate_compact_rewrite_script, make_executable,
    resolve_binary_path_for_bash, write_file, write_wrapper_file,
};

pub(super) fn install_standard_hook_scripts(
    hooks_dir: &std::path::Path,
    home: &std::path::Path,
    rewrite_name: &str,
    redirect_name: &str,
) {
    let _ = std::fs::create_dir_all(hooks_dir);

    // #719: never re-stamp a working portable wrapper with an absolute path.
    let binary = resolve_binary_path_for_bash();
    let rewrite_path = hooks_dir.join(rewrite_name);
    let rewrite_script = generate_compact_rewrite_script(&binary);
    write_wrapper_file(&rewrite_path, &rewrite_script, home);
    make_executable(&rewrite_path);

    let redirect_path = hooks_dir.join(redirect_name);
    write_file(&redirect_path, REDIRECT_SCRIPT_GENERIC);
    make_executable(&redirect_path);
}
