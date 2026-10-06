use std::collections::HashMap;
use std::path::PathBuf;

use super::model::{CepStats, DayStats, StatsStore};

fn stats_dir() -> Option<PathBuf> {
    crate::core::data_dir::lean_ctx_data_dir().ok()
}

fn stats_path() -> Option<PathBuf> {
    stats_dir().map(|d| d.join("stats.json"))
}

pub(super) fn load_from_disk() -> StatsStore {
    let Some(path) = stats_path() else {
        return StatsStore::default();
    };

    match std::fs::read_to_string(&path) {
        Ok(content) => match serde_json::from_str(&content) {
            Ok(store) => store,
            // Corrupt display cache (#706): NEVER silently reset — the next
            // flush would overwrite the good file with an empty store and the
            // whole history would be gone. Quarantine the corrupt bytes so
            // they stay recoverable, warn loudly, and start fresh.
            Err(e) => {
                quarantine_corrupt(&path, &e);
                StatsStore::default()
            }
        },
        Err(_) => StatsStore::default(),
    }
}

/// Moves an unparseable `stats.json` aside to `stats.json.corrupt` (#706)
/// instead of letting the next flush overwrite it with an empty store. An
/// existing quarantine file is never overwritten — the OLDER corrupt copy is
/// the one closest to the lost history, so it wins. Pure display-cache
/// hygiene: the savings ledger (`savings/ledger.jsonl`) remains the
/// append-only source of truth either way.
fn quarantine_corrupt(path: &std::path::Path, err: &serde_json::Error) {
    let quarantine = path.with_extension("json.corrupt");
    let preserved = if quarantine.exists() {
        false
    } else {
        std::fs::rename(path, &quarantine).is_ok()
    };
    tracing::warn!(
        "stats.json is corrupt ({err}) — starting a fresh stats store. {}",
        if preserved {
            format!(
                "The corrupt file was preserved at {} for recovery; \
                 `lean-ctx doctor` will flag it",
                quarantine.display()
            )
        } else {
            format!(
                "A previous corrupt copy already exists at {} and was kept \
                 (older = closer to the lost history)",
                quarantine.display()
            )
        }
    );
}

fn write_to_disk(store: &StatsStore) -> bool {
    let Some(dir) = stats_dir() else { return false };

    if !dir.exists() && std::fs::create_dir_all(&dir).is_err() {
        return false;
    }

    let path = dir.join("stats.json");
    let tmp = dir.join(".stats.json.tmp");
    serde_json::to_string(store).is_ok_and(|json| {
        std::fs::write(&tmp, json).is_ok() && std::fs::rename(&tmp, &path).is_ok()
    })
}

pub(super) fn locked_write(store: &StatsStore) {
    let Some(dir) = stats_dir() else { return };
    let lock_path = dir.join(".stats.lock");
    let _lock = acquire_file_lock(&lock_path);
    if _lock.is_none() {
        return;
    }
    let _ = write_to_disk(store);
}

pub(super) fn merge_and_save(current: &StatsStore, baseline: &StatsStore) -> Option<StatsStore> {
    let dir = stats_dir()?;
    let lock_path = dir.join(".stats.lock");
    let _lock = acquire_file_lock(&lock_path)?;

    let disk = load_from_disk();
    let merged = apply_deltas(&disk, current, baseline);
    write_to_disk(&merged).then_some(merged)
}

struct FileLockGuard(PathBuf);

impl Drop for FileLockGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn acquire_file_lock(lock_path: &std::path::Path) -> Option<FileLockGuard> {
    for _ in 0..20 {
        if std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(lock_path)
            .is_ok()
        {
            return Some(FileLockGuard(lock_path.to_path_buf()));
        }
        if let Ok(meta) = std::fs::metadata(lock_path)
            && let Ok(modified) = meta.modified()
            && modified.elapsed().unwrap_or_default().as_secs() > 5
        {
            let _ = std::fs::remove_file(lock_path);
            continue;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    None
}

pub(super) fn apply_deltas(
    disk: &StatsStore,
    current: &StatsStore,
    baseline: &StatsStore,
) -> StatsStore {
    let mut merged = disk.clone();

    let delta_commands = current
        .total_commands
        .saturating_sub(baseline.total_commands);
    let delta_input = current
        .total_input_tokens
        .saturating_sub(baseline.total_input_tokens);
    let delta_output = current
        .total_output_tokens
        .saturating_sub(baseline.total_output_tokens);

    merged.total_commands = merged.total_commands.saturating_add(delta_commands);
    merged.total_input_tokens = merged.total_input_tokens.saturating_add(delta_input);
    merged.total_output_tokens = merged.total_output_tokens.saturating_add(delta_output);
    merged.first_inject_tokens_saved = merged.first_inject_tokens_saved.saturating_add(
        current
            .first_inject_tokens_saved
            .saturating_sub(baseline.first_inject_tokens_saved),
    );
    merged.reread_tokens_saved = merged.reread_tokens_saved.saturating_add(
        current
            .reread_tokens_saved
            .saturating_sub(baseline.reread_tokens_saved),
    );
    merged.active_tool_result_tokens_saved = merged.active_tool_result_tokens_saved.saturating_add(
        current
            .active_tool_result_tokens_saved
            .saturating_sub(baseline.active_tool_result_tokens_saved),
    );
    merged.last_tool_result_turn = merged
        .last_tool_result_turn
        .max(current.last_tool_result_turn);
    merged.stream_tracked_results = merged.stream_tracked_results.saturating_add(
        current
            .stream_tracked_results
            .saturating_sub(baseline.stream_tracked_results),
    );

    for (cmd, stats) in &current.commands {
        let base = baseline.commands.get(cmd);
        let dc = stats.count.saturating_sub(base.map_or(0, |b| b.count));
        let di = stats
            .input_tokens
            .saturating_sub(base.map_or(0, |b| b.input_tokens));
        let do_ = stats
            .output_tokens
            .saturating_sub(base.map_or(0, |b| b.output_tokens));
        if dc > 0 || di > 0 || do_ > 0 {
            let entry = merged.commands.entry(cmd.clone()).or_default();
            entry.count += dc;
            entry.input_tokens += di;
            entry.output_tokens += do_;
        }
    }

    // Tags are metadata rather than counters: carry the newest classification
    // through disk merges and sibling-directory display aggregation.
    for (cmd, class) in &current.command_classes {
        merged.command_classes.insert(cmd.clone(), *class);
    }

    merge_daily(&mut merged.daily, &current.daily, &baseline.daily);

    if let Some(ref ts) = current.last_use {
        match merged.last_use {
            Some(ref existing) if existing >= ts => {}
            _ => merged.last_use = Some(ts.clone()),
        }
    }
    if merged.first_use.is_none() {
        merged.first_use.clone_from(&current.first_use);
    } else if let Some(ref cur_first) = current.first_use
        && let Some(ref merged_first) = merged.first_use
        && cur_first < merged_first
    {
        merged.first_use = Some(cur_first.clone());
    }

    merge_cep(&mut merged.cep, &current.cep, &baseline.cep);

    merged
}

pub(super) fn merge_daily(merged: &mut Vec<DayStats>, current: &[DayStats], baseline: &[DayStats]) {
    let base_map: HashMap<String, &DayStats> =
        baseline.iter().map(|d| (d.date.clone(), d)).collect();

    for day in current {
        let base = base_map.get(&day.date);
        let dc = day.commands.saturating_sub(base.map_or(0, |b| b.commands));
        let di = day
            .input_tokens
            .saturating_sub(base.map_or(0, |b| b.input_tokens));
        let do_ = day
            .output_tokens
            .saturating_sub(base.map_or(0, |b| b.output_tokens));
        if dc == 0 && di == 0 && do_ == 0 {
            continue;
        }
        if let Some(existing) = merged.iter_mut().find(|d| d.date == day.date) {
            existing.commands += dc;
            existing.input_tokens += di;
            existing.output_tokens += do_;
            // Prefer the most recent known version for the day (#307).
            if !day.version.is_empty() {
                existing.version.clone_from(&day.version);
            }
        } else {
            merged.push(DayStats {
                date: day.date.clone(),
                commands: dc,
                input_tokens: di,
                output_tokens: do_,
                version: day.version.clone(),
            });
        }
    }

    if merged.len() > super::MAX_DAILY_HISTORY_DAYS {
        merged.sort_by_key(|item| item.date.clone());
        merged.drain(..merged.len() - super::MAX_DAILY_HISTORY_DAYS);
    }
}

fn merge_cep(merged: &mut CepStats, current: &CepStats, baseline: &CepStats) {
    merged.sessions += current.sessions.saturating_sub(baseline.sessions);
    merged.total_cache_hits += current
        .total_cache_hits
        .saturating_sub(baseline.total_cache_hits);
    merged.total_cache_reads += current
        .total_cache_reads
        .saturating_sub(baseline.total_cache_reads);
    merged.total_tokens_original += current
        .total_tokens_original
        .saturating_sub(baseline.total_tokens_original);
    merged.total_tokens_compressed += current
        .total_tokens_compressed
        .saturating_sub(baseline.total_tokens_compressed);

    for (mode, count) in &current.modes {
        let base_count = baseline.modes.get(mode).copied().unwrap_or(0);
        let delta = count.saturating_sub(base_count);
        if delta > 0 {
            *merged.modes.entry(mode.clone()).or_insert(0) += delta;
        }
    }

    let base_scores_len = baseline.scores.len();
    if current.scores.len() > base_scores_len {
        for snapshot in &current.scores[base_scores_len..] {
            merged.scores.push(snapshot.clone());
        }
    }
    if merged.scores.len() > 100 {
        merged.scores.drain(..merged.scores.len() - 100);
    }

    if current.last_session_pid.is_some() {
        merged.last_session_pid = current.last_session_pid;
        merged.last_session_original = current.last_session_original;
        merged.last_session_compressed = current.last_session_compressed;
        merged.last_session_cache_hits = current.last_session_cache_hits;
        merged.last_session_cache_reads = current.last_session_cache_reads;
    }
}
