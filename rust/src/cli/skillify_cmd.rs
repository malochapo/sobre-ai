//! `lean-ctx skillify` — distill recurring session patterns into .cursor/rules (#290).

use crate::tools::impl_skillify;

pub(crate) fn cmd_skillify(args: &[String]) {
    let project_root = super::common::detect_project_root(args);
    let action = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .map_or("mine", String::as_str);

    if matches!(action, "help" | "--help" | "-h") {
        print_help();
        return;
    }

    // The promote action takes the next positional after the action as the slug.
    let slug = args
        .iter()
        .filter(|a| !a.starts_with("--"))
        .nth(1)
        .map(String::as_str);

    let out = impl_skillify::handle(&project_root, action, slug);
    println!("{out}");
}

fn print_help() {
    eprintln!(
        "lean-ctx skillify — codify recurring session patterns into .cursor/rules\n\
         \n\
         USAGE:\n    \
             lean-ctx skillify <action> [args]\n\
         \n\
         ACTIONS:\n    \
             mine               Distill diary + knowledge into rules (default)\n    \
             list               Show generated skillify rules\n    \
             status             Show config + candidate/rule counts\n    \
             promote <slug>     Copy a project rule to ~/.cursor/rules\n    \
             help               Show this help"
    );
}
