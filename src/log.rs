//! 极简日志。
//!
//! 命令都是短命进程，stderr 直接给人看，不需要结构化字段或异步队列。级别只
//! 决定哪些行出现，格式固定为 `级别 消息`，避免和 stdout 上的 JSON 混在一起。

use crate::config::LogLevel;
use std::sync::atomic::{AtomicU8, Ordering};

static LEVEL: AtomicU8 = AtomicU8::new(2);

/// 设定当前级别。
pub fn init(level: LogLevel) {
    LEVEL.store(rank(level), Ordering::Relaxed);
}

/// 是否输出某个级别。
pub fn enabled(level: LogLevel) -> bool {
    rank(level) <= LEVEL.load(Ordering::Relaxed)
}

/// 调试信息。
pub fn debug(message: impl AsRef<str>) {
    emit(LogLevel::Debug, "debug", message.as_ref());
}

/// 提示信息，走 stderr。
pub fn info(message: impl AsRef<str>) {
    emit(LogLevel::Info, "info", message.as_ref());
}

/// 警告。
pub fn warn(message: impl AsRef<str>) {
    emit(LogLevel::Warn, "warn", message.as_ref());
}

fn emit(level: LogLevel, label: &str, message: &str) {
    if enabled(level) {
        eprintln!("{label:<5} {message}");
    }
}

fn rank(level: LogLevel) -> u8 {
    match level {
        LogLevel::Error => 0,
        LogLevel::Warn => 1,
        LogLevel::Info => 2,
        LogLevel::Debug => 3,
        LogLevel::Trace => 4,
    }
}
