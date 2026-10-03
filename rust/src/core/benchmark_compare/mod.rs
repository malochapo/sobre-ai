pub mod competitors;
pub mod metrics;
pub mod report;
pub mod system_info;

use std::path::Path;

use report::CompareReport;

pub fn run_compare(root: &Path, output_path: Option<&str>) -> CompareReport {
    let metrics = metrics::measure_all(root);
    let system = system_info::collect();
    let competitors = competitors::all_competitors();

    let report = CompareReport {
        metrics,
        system,
        competitors,
    };

    if let Some(out_path) = output_path {
        let md = report::generate_markdown(&report);
        if let Err(e) = std::fs::write(out_path, &md) {
            eprintln!("Failed to write {out_path}: {e}");
        } else {
            eprintln!("Wrote benchmark report to {out_path}");
        }
    }

    report
}

/// A small project for the tests. Measuring the whole crate took about 40 s a
/// test; the code paths are the same on a few files.
#[cfg(test)]
pub(crate) fn test_fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("fixture dir");
    let src = dir.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("main.rs"),
        r#"use std::collections::HashMap;

fn main() {
    let mut map: HashMap<String, Vec<i32>> = HashMap::new();
    map.insert("hello".to_string(), vec![1, 2, 3]);
    process(&map);
}

fn process(data: &HashMap<String, Vec<i32>>) {
    for (key, values) in data {
        let sum: i32 = values.iter().sum();
        println!("{key}: sum={sum}, count={}", values.len());
    }
}

pub struct Config {
    pub name: String,
    pub timeout: u64,
}

impl Config {
    pub fn new(name: &str) -> Self {
        Self { name: name.to_string(), timeout: 30 }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.name.is_empty() {
            return Err("name cannot be empty".to_string());
        }
        Ok(())
    }
}
"#,
    )
    .unwrap();
    std::fs::write(
        src.join("handler.ts"),
        r#"export interface Request { path: string; body?: string }

export function handle(req: Request): string {
  if (req.path === "/health") {
    return "ok";
  }
  return route(req);
}

function route(req: Request): string {
  return `not found: ${req.path}`;
}
"#,
    )
    .unwrap();
    std::fs::write(
        src.join("util.py"),
        r#"def parse(line: str) -> dict:
    key, _, value = line.partition("=")
    return {key.strip(): value.strip()}


class Store:
    def __init__(self):
        self.items = {}

    def put(self, key, value):
        self.items[key] = value
"#,
    )
    .unwrap();
    dir
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_compare_produces_valid_report() {
        let fixture = test_fixture();
        let report = run_compare(fixture.path(), None);
        assert!(report.metrics.project_benchmark.files_measured > 0);
        assert!(!report.competitors.is_empty());
        assert!(!report.system.lean_ctx_version.is_empty());
    }

    #[test]
    fn run_compare_writes_output_file() {
        let dir = tempfile::tempdir().unwrap();
        let out_path = dir.path().join("test_benchmarks.md");
        let out_str = out_path.to_string_lossy().to_string();

        let fixture = test_fixture();
        let report = run_compare(fixture.path(), Some(&out_str));
        assert!(out_path.exists());

        let content = std::fs::read_to_string(&out_path).unwrap();
        assert!(content.contains("Head-to-Head"));
        assert!(report.metrics.project_benchmark.files_measured > 0);
    }
}
