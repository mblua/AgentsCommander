use super::*;

#[test]
fn is_newer_basic() {
    assert!(is_newer("0.9.17", "0.9.16"));
    assert!(is_newer("1.0.0", "0.9.16"));
    assert!(is_newer("0.10.0", "0.9.16"));
    assert!(!is_newer("0.9.16", "0.9.16"));
    assert!(!is_newer("0.9.15", "0.9.16"));
}

#[test]
fn is_newer_ignores_prerelease_and_build() {
    assert!(is_newer("1.0.0-rc.1", "0.9.16")); // numeric core wins
    assert!(!is_newer("0.9.16+build.5", "0.9.16"));
}

#[test]
fn is_newer_fails_closed_on_garbage() {
    assert!(!is_newer("not-a-version", "0.9.16"));
    assert!(!is_newer("0.9.17", "garbage"));
    assert!(!is_newer("", ""));
}

#[test]
fn parse_latest_extracts_tag() {
    assert_eq!(
        parse_latest(r#"{"latest":"1.2.3","beta":"2.0.0-rc.1"}"#),
        Some("1.2.3".to_string())
    );
    assert_eq!(parse_latest(r#"{"no-latest":"x"}"#), None);
    assert_eq!(parse_latest("not json"), None);
    // FIX-2: an empty `latest` is treated as no-result, not cached as "".
    assert_eq!(parse_latest(r#"{"latest":""}"#), None);
}

#[test]
fn cli_notice_honors_toggle_and_version() {
    let newer = UpdateCache {
        last_checked_at: "2026-06-23T06:00:00Z".parse().unwrap(),
        latest_version: "0.9.20".into(),
    };
    let same = UpdateCache {
        last_checked_at: "2026-06-23T06:00:00Z".parse().unwrap(),
        latest_version: "0.9.16".into(),
    };
    // Enabled + newer -> notice carrying the version + upgrade command.
    let notice = cli_notice(&newer, "0.9.16", true).expect("expected a notice");
    assert!(notice.contains("0.9.20"));
    assert!(notice.contains("npm i -g @mblua/agentscommander"));
    // FIX-1: toggle OFF -> silent even with a newer cache.
    assert_eq!(cli_notice(&newer, "0.9.16", false), None);
    // Enabled but not newer -> silent.
    assert_eq!(cli_notice(&same, "0.9.16", true), None);
}

#[test]
fn should_check_gates_on_age() {
    let now = "2026-06-23T12:00:00Z".parse::<DateTime<Utc>>().unwrap();
    assert!(should_check(&None, now));
    let fresh = UpdateCache {
        last_checked_at: "2026-06-23T06:00:00Z".parse().unwrap(), // 6h ago
        latest_version: "0.9.16".into(),
    };
    assert!(!should_check(&Some(fresh), now));
    let stale = UpdateCache {
        last_checked_at: "2026-06-22T06:00:00Z".parse().unwrap(), // 30h ago
        latest_version: "0.9.16".into(),
    };
    assert!(should_check(&Some(stale), now));
    let future = UpdateCache {
        last_checked_at: "2026-06-24T06:00:00Z".parse().unwrap(), // clock skew
        latest_version: "0.9.16".into(),
    };
    assert!(should_check(&Some(future), now));
}

// ---- T1: pure decision-logic tests (zero Tauri, zero network) ----------

#[test]
fn plan_check_skip_use_fetch() {
    let now = "2026-06-23T12:00:00Z".parse::<DateTime<Utc>>().unwrap();
    let fresh = UpdateCache {
        last_checked_at: "2026-06-23T06:00:00Z".parse().unwrap(), // 6h ago
        latest_version: "0.9.16".into(),
    };
    // Toggle off -> Skip, even with a fresh cache.
    assert_eq!(
        plan_check(false, &Some(fresh.clone()), now),
        CheckPlan::Skip
    );
    // Enabled + no cache -> Fetch.
    assert_eq!(plan_check(true, &None, now), CheckPlan::Fetch);
    // Enabled + fresh cache (throttle hit) -> UseCached.
    assert_eq!(plan_check(true, &Some(fresh), now), CheckPlan::UseCached);
    // Enabled + stale cache (30h) -> Fetch.
    let stale = UpdateCache {
        last_checked_at: "2026-06-22T06:00:00Z".parse().unwrap(),
        latest_version: "0.9.16".into(),
    };
    assert_eq!(plan_check(true, &Some(stale), now), CheckPlan::Fetch);
}

#[test]
fn resolve_latest_all_paths() {
    let cache = Some(UpdateCache {
        last_checked_at: "2026-06-23T06:00:00Z".parse().unwrap(),
        latest_version: "0.9.10".into(),
    });
    // Skip: nothing known, no write.
    assert_eq!(resolve_latest(CheckPlan::Skip, None, &cache), (None, false));
    // UseCached: cached value, no write (throttle hit).
    assert_eq!(
        resolve_latest(CheckPlan::UseCached, None, &cache),
        (Some("0.9.10".into()), false)
    );
    // Fetch success: fresh value, write-back true (write-only-on-success).
    assert_eq!(
        resolve_latest(CheckPlan::Fetch, Some("0.9.20".into()), &cache),
        (Some("0.9.20".into()), true)
    );
    // Fetch fail: fall back to cache, no write.
    assert_eq!(
        resolve_latest(CheckPlan::Fetch, None, &cache),
        (Some("0.9.10".into()), false)
    );
}

#[test]
fn resolve_latest_fetch_fail_no_cache() {
    // Fetch fail with no cache -> nothing known, no write (no false toast).
    assert_eq!(resolve_latest(CheckPlan::Fetch, None, &None), (None, false));
}

// ---- #1925: the remote blocking-menu download ---------------------------

mod remote_blocking_menus_1925;

/// #2133 (P7) - the agent-help download over an injected temp `settings_path`.
mod remote_agent_help_2133;
