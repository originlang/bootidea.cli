//! repo 子命令共用的小工具：路径规范化、UTC 时间戳、glob 匹配与表格宽度。

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// 规范化路径：尽量 canonicalize，失败时退回绝对路径；去掉 Windows 的 `\\?\` 前缀。
pub fn normalize(path: &Path) -> PathBuf {
    let resolved = path
        .canonicalize()
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default().join(path));
    let text = resolved.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) => PathBuf::from(rest),
        None => resolved,
    }
}

/// 当前时间的 RFC 3339 表示，统一 UTC（`2026-11-06T08:00:00Z`）。
pub fn now_rfc3339() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0);
    format_rfc3339(seconds)
}

/// 秒级 Unix 时间戳转 RFC 3339；负值按 1970 处理。
fn format_rfc3339(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let rest = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        (rest % 3600) / 60,
        rest % 60
    )
}

/// 天数（相对 1970-01-01）转公历年月日，Howard Hinnant 的 civil_from_days。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// glob 匹配，`*` 匹配段内任意字符，`?` 匹配单个字符，`**` 匹配任意多段。
///
/// 同时用于扫描排除规则（相对根目录路径）和批量选择的登记名称匹配。名称
/// 只有一个路径段，因此 `**` 在其中退化为 `*`。
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let pattern: Vec<&str> = pattern.split('/').collect();
    let text: Vec<&str> = text.split('/').collect();
    match_segments(&pattern, &text)
}

fn match_segments(pattern: &[&str], text: &[&str]) -> bool {
    match pattern.first() {
        None => text.is_empty(),
        Some(&"**") => (0..=text.len()).any(|skip| match_segments(&pattern[1..], &text[skip..])),
        Some(segment) => {
            !text.is_empty()
                && match_segment(segment, text[0])
                && match_segments(&pattern[1..], &text[1..])
        }
    }
}

fn match_segment(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    match_chars(&pattern, &text)
}

fn match_chars(pattern: &[char], text: &[char]) -> bool {
    match pattern.first() {
        None => text.is_empty(),
        Some('*') => {
            match_chars(&pattern[1..], text)
                || (!text.is_empty() && match_chars(pattern, &text[1..]))
        }
        Some('?') => !text.is_empty() && match_chars(&pattern[1..], &text[1..]),
        Some(expected) => text.first() == Some(expected) && match_chars(&pattern[1..], &text[1..]),
    }
}

/// 终端显示宽度：CJK 与全角字符按两列计，用于对齐表格。
pub fn display_width(text: &str) -> usize {
    text.chars()
        .map(|ch| {
            let code = ch as u32;
            let wide = matches!(code,
                0x1100..=0x115F
                | 0x2E80..=0x303E
                | 0x3041..=0x33FF
                | 0x3400..=0x4DBF
                | 0x4E00..=0x9FFF
                | 0xA000..=0xA4CF
                | 0xAC00..=0xD7A3
                | 0xF900..=0xFAFF
                | 0xFE30..=0xFE4F
                | 0xFF00..=0xFF60
                | 0xFFE0..=0xFFE6);
            if wide { 2 } else { 1 }
        })
        .sum()
}

/// 按显示宽度左对齐补空格。
pub fn pad(text: &str, width: usize) -> String {
    let current = display_width(text);
    let mut out = String::from(text);
    for _ in current..width {
        out.push(' ');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_matches_known_instants() {
        assert_eq!(format_rfc3339(0), "1970-01-01T00:00:00Z");
        // 2026-11-06T15:03:35Z
        assert_eq!(format_rfc3339(1_793_977_415), "2026-11-06T15:03:35Z");
        assert_eq!(format_rfc3339(1_609_459_199), "2020-12-31T23:59:59Z");
    }

    #[test]
    fn glob_handles_wildcards_and_nested_segments() {
        assert!(glob_match("boot*", "bootidea.cli"));
        assert!(glob_match("*.cli", "bootidea.cli"));
        assert!(!glob_match("boot?", "bootidea.cli"));
        assert!(glob_match("archive/**", "archive/2020/a"));
        assert!(glob_match("archive/**", "archive"));
        assert!(glob_match("**/target", "a/b/target"));
        assert!(!glob_match("a/*", "a/b/c"));
        assert!(glob_match("*", "anything"));
        assert!(!glob_match("*", "a/b"));
    }

    #[test]
    fn width_counts_cjk_as_two_columns() {
        assert_eq!(display_width("abc"), 3);
        assert_eq!(display_width("名称"), 4);
        assert_eq!(pad("名", 4), "名  ");
    }
}
