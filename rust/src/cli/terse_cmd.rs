//! `sobre terse` — read and set the compression level.
//!
//! Extracted from the former `cheatsheet_cmd`, whose name covered two
//! unrelated commands: the (removed) agent cheat sheet and this one.

pub fn cmd_compression(args: &[String]) {
    use crate::core::config::{CompressionLevel, Config};

    let action = args.first().map(std::string::String::as_str);
    if let Some(level @ ("off" | "lite" | "standard" | "max")) = action {
        if let Err(e) = Config::update_global(|cfg| {
            cfg.compression_level = match level {
                "lite" => CompressionLevel::Lite,
                "standard" => CompressionLevel::Standard,
                "max" => CompressionLevel::Max,
                _ => CompressionLevel::Off,
            };
        }) {
            eprintln!("Error saving config: {e}");
            std::process::exit(1);
        }
        let effective = CompressionLevel::from_str_label(level).unwrap_or(CompressionLevel::Off);
        println!("Compression level: {level} — {}", effective.description());
        let home = dirs::home_dir().unwrap_or_default();
        let result = crate::rules_inject::inject_all_rules(&home);
        if !result.updated.is_empty() {
            println!(
                "Updated {} rules file(s) with compression prompt.",
                result.updated.len()
            );
        }
        println!("Restart your agent/IDE for changes to take effect.");
    } else {
        let cfg = Config::load();
        let effective = CompressionLevel::effective(&cfg);
        println!("Compression level: {}", effective.label());
        println!();
        println!("Usage: lean-ctx compression <off|lite|standard|max>");
        println!("       lean-ctx terse <off|lite|standard|max>  (alias)");
        println!();
        println!("  off      — {}", CompressionLevel::Off.description());
        println!("  lite     — {}", CompressionLevel::Lite.description());
        println!("  standard — {}", CompressionLevel::Standard.description());
        println!("  max      — {}", CompressionLevel::Max.description());
        println!();
        println!("Override per session:  LEAN_CTX_COMPRESSION=standard");
        println!("Override per project:  compression_level = \"standard\" in .lean-ctx.toml");
        println!();

        let (ta, od, crp, tm) = effective.to_components();
        let ta_name = match ta {
            crate::core::config::TerseAgent::Off => "off",
            crate::core::config::TerseAgent::Lite => "lite",
            crate::core::config::TerseAgent::Full => "full",
            crate::core::config::TerseAgent::Ultra => "ultra",
        };
        let od_name = match od {
            crate::core::config::OutputDensity::Normal => "normal",
            crate::core::config::OutputDensity::Terse => "terse",
            crate::core::config::OutputDensity::Ultra => "ultra",
        };
        println!("Active components:");
        println!("  Agent prompt:    {ta_name}");
        println!("  Output density:  {od_name}");
        println!("  CRP mode:        {crp}");
        println!("  Terse session:   {tm}");
    }
}
