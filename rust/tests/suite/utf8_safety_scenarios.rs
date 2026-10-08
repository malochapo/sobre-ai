fn make_cyrillic_string(target_bytes: usize) -> String {
    let base = "план реализации — спецификация документа в ";
    let mut s = String::new();
    while s.len() < target_bytes + 200 {
        s.push_str(base);
    }
    s
}

fn make_mixed_multibyte(target_bytes: usize) -> String {
    let base = "日本語テスト → résumé → план → 中文 → emoji🎉 end ";
    let mut s = String::new();
    while s.len() < target_bytes + 200 {
        s.push_str(base);
    }
    s
}

fn make_emoji_heavy(target_bytes: usize) -> String {
    let base = "🎉🚀💡🔥✨🎯🧪📦🔧⚡";
    let mut s = String::new();
    while s.len() < target_bytes + 200 {
        s.push_str(base);
    }
    s
}

#[test]
fn hash_fast_cyrillic_no_panic() {
    let s = make_cyrillic_string(20_000);
    assert!(
        s.len() > 16 * 1024,
        "string must exceed hash_fast threshold"
    );
    let hash = sobre::server::helpers::hash_fast(&s);
    assert!(!hash.is_empty());
}

#[test]
fn hash_fast_emoji_no_panic() {
    let s = make_emoji_heavy(20_000);
    assert!(s.len() > 16 * 1024);
    let hash = sobre::server::helpers::hash_fast(&s);
    assert!(!hash.is_empty());
}

#[test]
fn hash_fast_mixed_multibyte_no_panic() {
    let s = make_mixed_multibyte(20_000);
    assert!(s.len() > 16 * 1024);
    let hash = sobre::server::helpers::hash_fast(&s);
    assert!(!hash.is_empty());
}

#[test]
fn hash_fast_boundary_at_4096() {
    let mut s = String::new();
    while s.len() < 4094 {
        s.push('a');
    }
    s.push('в');
    assert_eq!(s.as_bytes()[4094], 0xd0);
    assert_eq!(s.as_bytes()[4095], 0xb2);
    while s.len() < 20_000 {
        s.push('x');
    }
    let hash = sobre::server::helpers::hash_fast(&s);
    assert!(!hash.is_empty());
}

#[test]
fn hash_fast_boundary_at_4095() {
    let mut s = String::new();
    while s.len() < 4095 {
        s.push('a');
    }
    s.push('в');
    while s.len() < 20_000 {
        s.push('x');
    }
    let hash = sobre::server::helpers::hash_fast(&s);
    assert!(!hash.is_empty());
}

#[test]
fn hash_fast_consistent() {
    let s = make_cyrillic_string(20_000);
    let h1 = sobre::server::helpers::hash_fast(&s);
    let h2 = sobre::server::helpers::hash_fast(&s);
    assert_eq!(h1, h2, "hash_fast must be deterministic");
}

#[test]
fn hash_fast_small_string_no_truncation() {
    let s = "маленькая строка";
    assert!(s.len() <= 16 * 1024);
    let hash = sobre::server::helpers::hash_fast(s);
    assert!(!hash.is_empty());
}

#[test]
fn redev1l_exact_scenario() {
    let mut s = String::new();
    s.push_str("8.8-plan.md 781L\n");
    s.push_str("# EYE-343 §8.8 Discovery-producer + BMS-camera-claim — план реализации\n");
    s.push_str("\n> Спецификация: [`docs/korobka-arch.md` §8.8]");
    while s.len() < 20_000 {
        s.push_str("\nДополнительная строка с кириллицей для тестирования буфера.");
    }
    let hash = sobre::server::helpers::hash_fast(&s);
    assert!(
        !hash.is_empty(),
        "must not panic on ReDev1L's exact file pattern"
    );
}
