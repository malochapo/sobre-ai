pub fn compress_ps(output: &str) -> Option<String> {
    let lines: Vec<&str> = output.lines().collect();
    if lines.len() < 2 {
        return None;
    }

    let header = lines[0];
    let procs: Vec<&str> = lines[1..]
        .iter()
        .filter(|l| !l.trim().is_empty())
        .copied()
        .collect();
    let total = procs.len();

    if total <= 10 {
        return None;
    }

    let mut high_cpu: Vec<&str> = Vec::new();
    let mut high_mem: Vec<&str> = Vec::new();

    for &line in &procs {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() >= 4 {
            let cpu: f64 = cols.get(2).and_then(|s| s.parse().ok()).unwrap_or(0.0);
            let mem: f64 = cols.get(3).and_then(|s| s.parse().ok()).unwrap_or(0.0);
            if cpu > 1.0 {
                high_cpu.push(line);
            }
            if mem > 1.0 {
                high_mem.push(line);
            }
        }
    }

    let mut out = format!("ps: {total} processes\n{header}\n");

    if !high_cpu.is_empty() {
        out.push_str(&format!("--- high CPU ({}) ---\n", high_cpu.len()));
        for l in high_cpu.iter().take(15) {
            out.push_str(l);
            out.push('\n');
        }
    }
    if !high_mem.is_empty() {
        out.push_str(&format!("--- high MEM ({}) ---\n", high_mem.len()));
        for l in high_mem.iter().take(15) {
            out.push_str(l);
            out.push('\n');
        }
    }

    Some(out.trim_end().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ps_compresses_large_output() {
        let mut lines = vec![
            "USER       PID %CPU %MEM    VSZ   RSS TTY      STAT START   TIME COMMAND".to_string(),
        ];
        for i in 0..50 {
            lines.push(format!(
                "user     {:>5}  0.0  0.1  12345  1234 ?        S    10:00   0:00 process_{i}",
                1000 + i
            ));
        }
        lines.push(
            "root      9999 95.0  8.5 999999 99999 ?        R    10:00   5:00 heavy_proc"
                .to_string(),
        );
        let output = lines.join("\n");
        let result = compress_ps(&output).expect("should compress");
        assert!(result.contains("51 processes"));
        assert!(result.contains("heavy_proc"));
    }

    #[test]
    fn ps_skips_small_output() {
        let output = "USER PID %CPU %MEM\nroot 1 0.0 0.1 init";
        assert!(compress_ps(output).is_none());
    }
}
