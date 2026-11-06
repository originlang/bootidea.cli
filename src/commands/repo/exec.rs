//! 批量调度。
//!
//! 目标按“共享共同 Git 目录”分组：同一组的 worktree 串行执行，组之间按
//! `--jobs` 并发。结果按目标顺序存放，输出不会因并发而交错。

use crate::commands::repo::select::Target;
use anyhow::Result;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// 单个目标的结果。
#[derive(Debug)]
pub struct Report<T> {
    /// 对应目标在输入中的下标。
    pub index: usize,
    /// 成功时的输出。
    pub output: Option<T>,
    /// 失败原因。
    pub error: Option<String>,
    /// 因 `--fail-fast` 未执行。
    pub skipped: bool,
}

impl<T> Report<T> {
    /// 是否成功。
    pub fn ok(&self) -> bool {
        self.error.is_none() && !self.skipped
    }
}

/// 并发执行 `op`，返回与 `targets` 等长、按下标排列的结果。
pub fn run<T, F>(targets: &[Target], jobs: usize, fail_fast: bool, op: F) -> Vec<Report<T>>
where
    T: Send,
    F: Fn(&Target) -> Result<T> + Sync,
{
    let units = partition(targets);
    let worker_count = jobs.max(1).min(units.len().max(1));
    let mut slots: Vec<Option<Report<T>>> = (0..targets.len()).map(|_| None).collect();

    {
        let queue = Mutex::new(VecDeque::from(units));
        let slots = Mutex::new(&mut slots);
        let stop = AtomicBool::new(false);
        std::thread::scope(|scope| {
            for _ in 0..worker_count {
                scope.spawn(|| {
                    loop {
                        if fail_fast && stop.load(Ordering::Relaxed) {
                            break;
                        }
                        let Some(unit) = queue.lock().expect("队列锁").pop_front() else {
                            break;
                        };
                        for index in unit {
                            let report = match op(&targets[index]) {
                                Ok(output) => Report {
                                    index,
                                    output: Some(output),
                                    error: None,
                                    skipped: false,
                                },
                                Err(error) => {
                                    if fail_fast {
                                        stop.store(true, Ordering::Relaxed);
                                    }
                                    Report {
                                        index,
                                        output: None,
                                        error: Some(format!("{error:#}")),
                                        skipped: false,
                                    }
                                }
                            };
                            slots.lock().expect("结果锁")[index] = Some(report);
                        }
                    }
                });
            }
        });
    }

    slots
        .into_iter()
        .enumerate()
        .map(|(index, slot)| {
            slot.unwrap_or(Report {
                index,
                output: None,
                error: None,
                skipped: true,
            })
        })
        .collect()
}

/// 按共同 Git 目录分单元；没有登记信息的路径各自独立。
fn partition(targets: &[Target]) -> Vec<Vec<usize>> {
    let mut units: Vec<(Option<PathBuf>, Vec<usize>)> = Vec::new();
    for (index, target) in targets.iter().enumerate() {
        let Some(key) = &target.common_git_dir else {
            units.push((None, vec![index]));
            continue;
        };
        match units
            .iter_mut()
            .find(|(existing, _)| existing.as_ref() == Some(key))
        {
            Some((_, members)) => members.push(index),
            None => units.push((Some(key.clone()), vec![index])),
        }
    }
    units.into_iter().map(|(_, members)| members).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::atomic::AtomicUsize;

    fn target(name: &str, common: Option<&str>) -> Target {
        Target {
            id: None,
            name: name.into(),
            path: PathBuf::from(format!("/work/{name}")),
            common_git_dir: common.map(PathBuf::from),
        }
    }

    #[test]
    fn shared_git_dir_runs_serially() {
        let targets = vec![
            target("main", Some("/git/repo")),
            target("feature", Some("/git/repo")),
            target("other", Some("/git/other")),
        ];
        let units = partition(&targets);
        assert_eq!(units, vec![vec![0, 1], vec![2]]);
    }

    #[test]
    fn concurrency_keeps_order_and_reports_failures() {
        let targets = vec![target("a", None), target("b", None), target("c", None)];
        let reports = run(&targets, 3, false, |target| {
            if target.name == "b" {
                anyhow::bail!("模拟失败");
            }
            Ok(target.name.clone())
        });
        assert_eq!(reports.len(), 3);
        assert_eq!(reports[0].output.as_deref(), Some("a"));
        assert!(!reports[1].ok());
        assert!(
            reports[1]
                .error
                .as_deref()
                .unwrap_or("")
                .contains("模拟失败")
        );
        assert_eq!(reports[2].output.as_deref(), Some("c"));
    }

    #[test]
    fn fail_fast_stops_scheduling_new_units() {
        let targets = (0..6)
            .map(|n| target(&format!("r{n}"), None))
            .collect::<Vec<_>>();
        let runs = AtomicUsize::new(0);
        let reports = run::<String, _>(&targets, 1, true, |target| {
            runs.fetch_add(1, Ordering::Relaxed);
            anyhow::bail!("{} 失败", target.name);
        });
        assert_eq!(runs.load(Ordering::Relaxed), 1);
        assert_eq!(reports.iter().filter(|report| report.skipped).count(), 5);
    }

    #[test]
    fn unregistered_targets_are_never_parallel_with_each_other() {
        let targets = vec![target("a", None), target("b", None)];
        let units = partition(&targets);
        assert_eq!(units, vec![vec![0], vec![1]]);
        assert_eq!(Path::new("/work/a"), targets[0].path);
    }
}
