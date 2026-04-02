use std::env;
use std::fs;
use std::io::{self, Read};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, SystemTime};

use chrono::{DateTime, Datelike, Local, Utc};
use serde_json::Value;

// ── ANSI Colors ────────────────────────────────────────
const BLUE: &str = "\x1b[38;2;0;153;255m";
const ORANGE: &str = "\x1b[38;2;255;176;85m";
const GREEN: &str = "\x1b[38;2;0;175;80m";
const CYAN: &str = "\x1b[38;2;86;182;194m";
const RED: &str = "\x1b[38;2;255;85;85m";
const YELLOW: &str = "\x1b[38;2;230;200;0m";
const WHITE: &str = "\x1b[38;2;220;220;220m";
const MAGENTA: &str = "\x1b[38;2;180;140;255m";
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";

// ── Helpers ────────────────────────────────────────────

fn format_tokens(num: u64) -> String {
    if num >= 1_000_000 {
        format!("{:.1}m", num as f64 / 1_000_000.0)
    } else if num >= 1_000 {
        format!("{}k", num / 1000)
    } else {
        format!("{num}")
    }
}

fn color_for_pct(pct: u32) -> &'static str {
    if pct >= 90 {
        RED
    } else if pct >= 70 {
        YELLOW
    } else if pct >= 50 {
        ORANGE
    } else {
        GREEN
    }
}

fn build_bar(pct: u32, width: usize) -> String {
    let pct = pct.min(100);
    let filled = (pct as usize * width + 50) / 100; // round, not truncate
    let empty = width - filled;
    let bar_color = color_for_pct(pct);

    let sp = "\u{200A}"; // hair space (thinnest)
    let filled_str = vec!["●"; filled].join(sp);
    let empty_str = vec!["○"; empty].join(sp);

    if filled > 0 && empty > 0 {
        format!("{bar_color}{filled_str}{sp}{DIM}{bar_color}{empty_str}{RESET}")
    } else if filled > 0 {
        format!("{bar_color}{filled_str}{RESET}")
    } else {
        format!("{DIM}{bar_color}{empty_str}{RESET}")
    }
}

fn parse_iso(s: &str) -> Option<DateTime<Utc>> {
    // Try RFC3339 first, then common variants
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&Utc));
    }
    // Try without fractional seconds: "2026-03-09T12:30:00Z"
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(
        s.trim_end_matches('Z'),
        "%Y-%m-%dT%H:%M:%S",
    ) {
        return Some(dt.and_utc());
    }
    // Try with fractional: "2026-03-09T12:30:00.123Z"
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(
        s.trim_end_matches('Z'),
        "%Y-%m-%dT%H:%M:%S%.f",
    ) {
        return Some(dt.and_utc());
    }
    None
}

fn format_reset_time(iso_str: &str, style: &str) -> Option<String> {
    let dt = parse_iso(iso_str)?;
    let local: DateTime<Local> = dt.into();

    let formatted = match style {
        "time" => local.format("%-I:%M%p").to_string(),
        "datetime" => local.format("%b %-d, %-I:%M%p").to_string(),
        _ => local.format("%b %-d").to_string(),
    };
    Some(formatted.to_lowercase())
}

fn get_git_info(cwd: &str) -> (String, String) {
    let branch = Command::new("git")
        .args(["-C", cwd, "symbolic-ref", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    if branch.is_empty() {
        return (String::new(), String::new());
    }

    let dirty = Command::new("git")
        .args(["-C", cwd, "status", "--porcelain"])
        .output()
        .ok()
        .filter(|o| !o.stdout.is_empty())
        .map(|_| "*".to_string())
        .unwrap_or_default();

    (branch, dirty)
}

fn get_oauth_token() -> Option<String> {
    // 1. Environment variable
    if let Ok(token) = env::var("CLAUDE_CODE_OAUTH_TOKEN") {
        if !token.is_empty() {
            return Some(token);
        }
    }

    // 2. macOS Keychain
    #[cfg(target_os = "macos")]
    {
        if let Some(token) = extract_token_from_command(
            Command::new("security")
                .args(["find-generic-password", "-s", "Claude Code-credentials", "-w"]),
        ) {
            return Some(token);
        }
    }

    // 3. Credentials file
    let home = env::var("HOME").unwrap_or_default();
    let creds_path = format!("{home}/.claude/.credentials.json");
    if let Some(token) = extract_token_from_file(&creds_path) {
        return Some(token);
    }

    // 4. Linux secret-tool
    #[cfg(target_os = "linux")]
    {
        if let Some(token) = extract_token_from_command(
            Command::new("secret-tool")
                .args(["lookup", "service", "Claude Code-credentials"]),
        ) {
            return Some(token);
        }
    }

    None
}

fn extract_token_from_command(cmd: &mut Command) -> Option<String> {
    let output = cmd.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let blob = String::from_utf8(output.stdout).ok()?;
    let json: Value = serde_json::from_str(blob.trim()).ok()?;
    json["claudeAiOauth"]["accessToken"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

fn extract_token_from_file(path: &str) -> Option<String> {
    let content = fs::read_to_string(path).ok()?;
    let json: Value = serde_json::from_str(&content).ok()?;
    json["claudeAiOauth"]["accessToken"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

fn read_cache(cache_file: &str) -> Option<Value> {
    fs::read_to_string(cache_file)
        .ok()
        .and_then(|c| serde_json::from_str::<Value>(&c).ok())
        .filter(|v| v.get("five_hour").is_some())
}

fn fetch_usage_data() -> Option<Value> {
    let cache_dir = "/tmp/claude";
    let cache_file = format!("{cache_dir}/statusline-usage-cache.json");
    let cache_max_age = 60u64;

    // Check cache freshness
    let cache_age = fs::metadata(&cache_file)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .map(|d| d.as_secs());

    if let Some(age) = cache_age {
        if age < cache_max_age {
            if let Some(cached) = read_cache(&cache_file) {
                return Some(cached);
            }
        }
    }

    // Fetch fresh data
    let token = match get_oauth_token() {
        Some(t) => t,
        None => return read_cache(&cache_file), // stale cache fallback
    };

    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(5))
        .build();

    let response = match agent
        .get("https://api.anthropic.com/api/oauth/usage")
        .set("Accept", "application/json")
        .set("Content-Type", "application/json")
        .set("Authorization", &format!("Bearer {token}"))
        .set("anthropic-beta", "oauth-2025-04-20")
        .set("User-Agent", "claude-statusline-rs/0.1.0")
        .call()
    {
        Ok(r) => r,
        Err(_) => return read_cache(&cache_file), // stale cache fallback on HTTP error
    };

    let body = match response.into_string() {
        Ok(b) => b,
        Err(_) => return read_cache(&cache_file),
    };

    let json: Value = match serde_json::from_str::<Value>(&body) {
        Ok(v) if v.get("five_hour").is_some() => v,
        _ => return read_cache(&cache_file), // rate limit error → stale cache
    };

    fs::create_dir_all(cache_dir).ok();
    fs::write(&cache_file, &body).ok();
    Some(json)
}

// ── Main ───────────────────────────────────────────────

fn main() {
    let mut input = String::new();
    if io::stdin().read_to_string(&mut input).is_err() || input.trim().is_empty() {
        print!("Claude");
        return;
    }

    let json: Value = match serde_json::from_str(&input) {
        Ok(v) => v,
        Err(_) => {
            print!("Claude");
            return;
        }
    };

    let sep = format!(" {DIM}│{RESET} ");

    // ── Extract all JSON fields (single parse) ────────
    let model_name = json["model"]["display_name"]
        .as_str()
        .unwrap_or("Claude");

    let size = {
        let s = json["context_window"]["context_window_size"]
            .as_u64()
            .unwrap_or(200000);
        if s == 0 { 200000 } else { s }
    };

    let input_tokens = json["context_window"]["current_usage"]["input_tokens"]
        .as_u64()
        .unwrap_or(0);
    let cache_create = json["context_window"]["current_usage"]["cache_creation_input_tokens"]
        .as_u64()
        .unwrap_or(0);
    let cache_read = json["context_window"]["current_usage"]["cache_read_input_tokens"]
        .as_u64()
        .unwrap_or(0);
    let current = input_tokens + cache_create + cache_read;

    let _used_tokens = format_tokens(current);
    let _total_tokens = format_tokens(size);
    let pct_used = if size > 0 {
        (current * 100 / size) as u32
    } else {
        0
    };

    // ── Thinking status ───────────────────────────────
    let home = env::var("HOME").unwrap_or_default();
    let settings_path = format!("{home}/.claude/settings.json");
    let thinking_on = fs::read_to_string(&settings_path)
        .ok()
        .and_then(|c| serde_json::from_str::<Value>(&c).ok())
        .and_then(|v| v["alwaysThinkingEnabled"].as_bool())
        .unwrap_or(false);

    // ── Directory + Git ───────────────────────────────
    let cwd = json["cwd"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default()
        });

    let dirname = Path::new(&cwd)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let (git_branch, git_dirty) = get_git_info(&cwd);

    // ── Session duration ──────────────────────────────
    let session_duration = json["session"]["start_time"]
        .as_str()
        .and_then(parse_iso)
        .map(|start| {
            let secs = Utc::now()
                .signed_duration_since(start)
                .num_seconds()
                .max(0) as u64;
            if secs >= 3600 {
                format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
            } else if secs >= 60 {
                format!("{}m", secs / 60)
            } else {
                format!("{secs}s")
            }
        });

    // ── LINE 1 ────────────────────────────────────────
    let pct_color = color_for_pct(pct_used);

    let mut out = String::with_capacity(512);
    out.push_str(&format!("{BLUE}{model_name}{RESET}"));
    out.push_str(&format!("{sep}✍\u{fe0f} {pct_color}{pct_used}%{RESET}"));
    out.push_str(&format!("{sep}{CYAN}{dirname}{RESET}"));

    if !git_branch.is_empty() {
        out.push_str(&format!(
            " {GREEN}({git_branch}{RED}{git_dirty}{GREEN}){RESET}"
        ));
    }

    if let Some(ref dur) = session_duration {
        out.push_str(&format!("{sep}{DIM}⏱ {RESET}{WHITE}{dur}{RESET}"));
    }

    out.push_str(&sep);
    if thinking_on {
        out.push_str(&format!("{MAGENTA}◐ thinking{RESET}"));
    } else {
        out.push_str(&format!("{DIM}◑ thinking{RESET}"));
    }

    // ── RATE LIMITS ───────────────────────────────────
    if let Some(usage) = fetch_usage_data() {
        let bar_width = 10;

        // 5-hour
        let five_pct = usage["five_hour"]["utilization"]
            .as_f64()
            .unwrap_or(0.0) as u32;
        let five_reset = usage["five_hour"]["resets_at"]
            .as_str()
            .and_then(|s| format_reset_time(s, "time"))
            .unwrap_or_default();
        let five_bar = build_bar(five_pct, bar_width);
        let five_color = color_for_pct(five_pct);

        out.push_str(&format!(
            "\n\n{WHITE}current{RESET} {five_bar} {five_color}{five_pct:3}%{RESET} {DIM}⟳{RESET} {WHITE}{five_reset}{RESET}"
        ));

        // 7-day
        let seven_pct = usage["seven_day"]["utilization"]
            .as_f64()
            .unwrap_or(0.0) as u32;
        let seven_reset = usage["seven_day"]["resets_at"]
            .as_str()
            .and_then(|s| format_reset_time(s, "datetime"))
            .unwrap_or_default();
        let seven_bar = build_bar(seven_pct, bar_width);
        let seven_color = color_for_pct(seven_pct);

        out.push_str(&format!(
            "\n{WHITE}weekly{RESET}  {seven_bar} {seven_color}{seven_pct:3}%{RESET} {DIM}⟳{RESET} {WHITE}{seven_reset}{RESET}"
        ));

        // Extra usage
        if usage["extra_usage"]["is_enabled"]
            .as_bool()
            .unwrap_or(false)
        {
            let extra_pct = usage["extra_usage"]["utilization"]
                .as_f64()
                .unwrap_or(0.0) as u32;
            let extra_used = usage["extra_usage"]["used_credits"]
                .as_f64()
                .unwrap_or(0.0)
                / 100.0;
            let extra_limit = usage["extra_usage"]["monthly_limit"]
                .as_f64()
                .unwrap_or(0.0)
                / 100.0;
            let extra_bar = build_bar(extra_pct, bar_width);
            let extra_color = color_for_pct(extra_pct);

            // Next month 1st
            let now = Local::now();
            let month_names = [
                "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov",
                "dec",
            ];
            let next_month_idx = if now.month() == 12 { 0 } else { now.month() as usize };
            let next_reset = format!("{} 1", month_names[next_month_idx]);

            out.push_str(&format!(
                "\n{WHITE}extra{RESET}   {extra_bar} {extra_color}${extra_used:.2}{DIM}/{RESET}{WHITE}${extra_limit:.2}{RESET}"
            ));
            out.push_str(&format!(
                "\n{DIM}resets {RESET}{WHITE}{next_reset}{RESET}"
            ));
        }
    }

    print!("{out}");
}
