//! `sobre terse` — read and set the compression level.
//!
//! Extracted from the former `cheatsheet_cmd`, whose name covered two
//! unrelated commands: the (removed) agent cheat sheet and this one.

pub fn cmd_compression(args: &[String]) {
    use crate::core::config::{CompressionLevel, Config};

    let action = args.first().map(std::string::String::as_str);
    // A number in [0, 1], or a level name standing for the middle of its band.
    let value = action.and_then(|a| {
        a.parse::<f64>()
            .ok()
            .filter(|v| (0.0..=1.0).contains(v))
            .or_else(|| CompressionLevel::from_str_label(a).map(CompressionLevel::value))
    });
    if let Some(value) = value {
        if let Err(e) = Config::update_global(|cfg| cfg.compression_level = Some(value)) {
            eprintln!("Error saving config: {e}");
            std::process::exit(1);
        }
        let effective = CompressionLevel::from_value(value);
        println!(
            "Compression level: {value:.2} ({}) — {}",
            effective.label(),
            effective.description()
        );
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
        match cfg.compression_level {
            Some(v) => println!("Compression level: {v:.2} ({})", effective.label()),
            None => println!("Compression level: adaptive ({} prose)", effective.label()),
        }
        println!();
        println!("Usage: lean-ctx compression <0.0-1.0|off|lite|standard|max|raw>");
        println!("       lean-ctx terse <...>  (alias)");
        println!();
        println!("  off      — {}", CompressionLevel::Off.description());
        println!("  lite     — {}", CompressionLevel::Lite.description());
        println!("  standard — {}", CompressionLevel::Standard.description());
        println!("  max      — {}", CompressionLevel::Max.description());
        println!();
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
