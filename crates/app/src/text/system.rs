//! system messages.
use super::*;

impl Text {
    /// An argument the command line does not have.
    pub fn cli_unknown_argument(&self, argument: &str) -> String {
        match self.lang {
            Lang::En => format!("unknown argument: {argument}"),
            Lang::Zh => format!("未知参数：{argument}"),
        }
    }

    /// An option that needs a value and did not get one.
    pub fn cli_missing_value(&self, option: &str) -> String {
        match self.lang {
            Lang::En => format!("{option} needs a value"),
            Lang::Zh => format!("{option} 需要一个值"),
        }
    }

    /// An option that only means something alongside another.
    pub fn cli_not_applicable(&self, option: &str) -> String {
        match self.lang {
            Lang::En => format!("{option} is only used by --diagnostics"),
            Lang::Zh => format!("{option} 只能与 --diagnostics 一起使用"),
        }
    }

    /// The window stays on screen because the desktop would not take the tray
    /// icon that is the only way back to a hidden one.
    pub fn no_tray_keeps_window(&self, error: &str) -> String {
        match self.lang {
            Lang::En => format!(
                "The window stays open: this desktop would not take a tray icon ({error}), \
                 and without one there would be no way back to a hidden window."
            ),
            Lang::Zh => format!(
                "窗口保持打开：当前桌面无法创建托盘图标（{error}），没有托盘就无法从隐藏状态恢复。"
            ),
        }
    }

    /// Another copy of this program already holds the data directory.
    ///
    /// The pid and the build are what the holding run wrote about itself, and are
    /// missing when that line could not be read - which does not change the
    /// refusal, only how much of it can be said.
    pub fn instance_busy(&self, holder: Option<(u32, &str)>) -> String {
        let tail_en = "Use its window: this one will not start, because two windows would \
             share one data directory - the same database, the same profiles, and one of \
             them would stop the browsers the other is running.";
        let tail_zh = "请使用它的窗口：本副本不会启动，因为两个窗口会共用同一个数据目录\
             ——同一个数据库、同一批档案，其中一个还会停掉另一个正在运行的浏览器。";
        match (self.lang, holder) {
            (Lang::En, Some((pid, build))) => {
                format!(
                    "Another copy of this program is already running (pid {pid}, {build}). {tail_en}"
                )
            }
            (Lang::En, None) => {
                format!("Another copy of this program is already running. {tail_en}")
            }
            (Lang::Zh, Some((pid, build))) => {
                format!("本程序已有一个副本在运行（pid {pid}，{build}）。{tail_zh}")
            }
            (Lang::Zh, None) => format!("本程序已有一个副本在运行。{tail_zh}"),
        }
    }

    /// The data directory could not be locked at all, which is a fact about the
    /// directory rather than about another copy of the program.
    pub fn instance_unavailable(&self, error: &str) -> String {
        match self.lang {
            Lang::En => format!(
                "the data directory could not be locked, so this run will not start: {error}"
            ),
            Lang::Zh => format!("无法锁定数据目录，因此本次运行不会启动：{error}"),
        }
    }

    /// The first line of the activity log: what this run is and where it keeps
    /// its files, which is what a problem report is asked for first.
    pub fn run_started(&self, build: &str, platform: &str, data_dir: &str) -> String {
        match self.lang {
            Lang::En => format!("This run: {build} on {platform}; data directory {data_dir}."),
            Lang::Zh => format!("本次运行：{build}，平台 {platform}；数据目录 {data_dir}。"),
        }
    }

    /// A directory, and how many entries are in it. The report counts a
    /// directory rather than listing it: the profiles under one are the user's
    /// browsing, not a fact about the installation.
    pub fn diag_directory(&self, entries: usize) -> String {
        match self.lang {
            Lang::En => format!(
                "directory, {entries} {}",
                if entries == 1 { "entry" } else { "entries" }
            ),
            Lang::Zh => format!("目录，{entries} 项"),
        }
    }

    pub fn diag_bytes(&self, bytes: u64) -> String {
        match self.lang {
            Lang::En => format!("{bytes} {}", if bytes == 1 { "byte" } else { "bytes" }),
            Lang::Zh => format!("{bytes} 字节"),
        }
    }

    /// The permission bits, where the platform has them. Four digits: whether a
    /// file can be executed is the question this line is usually asked.
    pub fn diag_mode(&self, mode: &str) -> String {
        match self.lang {
            Lang::En => format!("mode {mode}"),
            Lang::Zh => format!("权限 {mode}"),
        }
    }

    pub fn diag_unreadable(&self, error: &str) -> String {
        match self.lang {
            Lang::En => format!("could not be read: {error}"),
            Lang::Zh => format!("无法读取：{error}"),
        }
    }

    /// The one thing `--diagnostics` prints when it works.
    pub fn diag_written(&self, path: &str) -> String {
        match self.lang {
            Lang::En => format!("Wrote a diagnostics report to {path}."),
            Lang::Zh => format!("诊断报告已写入 {path}。"),
        }
    }

    /// The line under the diagnostics button, naming the exact file this press
    /// would write - the button has no path field, so this is where the reader
    /// finds out where the file goes.
    pub fn diag_card_note(&self, path: &str) -> String {
        match self.lang {
            Lang::En => format!("Writes to {path}."),
            Lang::Zh => format!("将写入 {path}。"),
        }
    }

    /// How much of the log the report carries, and of how much.
    pub fn diag_log_lines(&self, shown: usize, total: usize) -> String {
        match self.lang {
            Lang::En => format!("the last {shown} of {total} lines, oldest first"),
            Lang::Zh => format!("共 {total} 行，显示最后 {shown} 行，由旧到新"),
        }
    }

    pub fn diag_create_failed(&self, directory: &str, error: &str) -> String {
        match self.lang {
            Lang::En => format!("could not create {directory}: {error}"),
            Lang::Zh => format!("无法创建 {directory}：{error}"),
        }
    }

    pub fn diag_write_failed(&self, path: &str, error: &str) -> String {
        match self.lang {
            Lang::En => format!("could not write {path}: {error}"),
            Lang::Zh => format!("无法写入 {path}：{error}"),
        }
    }
}
