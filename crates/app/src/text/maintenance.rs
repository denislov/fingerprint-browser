//! maintenance messages.
use super::*;

impl Text {
    /// The export path an empty field would write to.
    pub fn export_empty_writes(&self, path: &str) -> String {
        match self.lang {
            Lang::En => format!("Empty writes {path}"),
            Lang::Zh => format!("留空则写入 {path}"),
        }
    }

    pub fn export_read_failed(&self, error: &str) -> String {
        match self.lang {
            Lang::En => format!("The configuration could not be read. {error}"),
            Lang::Zh => format!("无法读取配置。{error}"),
        }
    }

    pub fn export_write_failed(&self, error: &str) -> String {
        match self.lang {
            Lang::En => format!("The backup could not be written. {error}"),
            Lang::Zh => format!("无法写入备份。{error}"),
        }
    }

    pub fn file_read_failed(&self, error: &str) -> String {
        match self.lang {
            Lang::En => format!("The file could not be read. {error}"),
            Lang::Zh => format!("无法读取文件。{error}"),
        }
    }

    /// Restoring would delete the rows of browsers that are still running.
    pub fn restore_running(&self, names: &str) -> String {
        match self.lang {
            Lang::En => format!(
                "Stop these profiles before restoring, so their browsers are not deleted from under them: {names}."
            ),
            Lang::Zh => {
                format!("还原前请先停止这些档案，否则它们正在运行的浏览器会被删除：{names}。")
            }
        }
    }

    /// The precondition a restore names when it refuses.
    pub fn restore_not_empty(&self, counts: &str) -> String {
        match self.lang {
            Lang::En => format!(
                "This installation already holds {counts}. Restoring replaces it, so it has to be confirmed."
            ),
            Lang::Zh => format!("本机已有{counts}。还原会替换它们，因此需要先确认。"),
        }
    }

    /// The file itself cannot become this installation, and nothing was changed.
    ///
    /// Said as a refusal rather than a list of what went wrong: the reason is the
    /// backend's own sentence, and the half the reader has to be told is that the
    /// old configuration is still there, because the failure happened before
    /// anything was replaced.
    pub fn restore_refused(&self, reason: &str) -> String {
        match self.lang {
            Lang::En => {
                format!("Nothing was restored: {reason}. The current configuration is unchanged.")
            }
            Lang::Zh => format!("未做任何还原：{reason}。当前配置保持不变。"),
        }
    }

    /// The browser-data copy would read a directory Chromium is still writing.
    pub fn copy_running(&self, names: &str) -> String {
        match self.lang {
            Lang::En => format!(
                "Stop these profiles first, so their browser data is not copied while it is being written: {names}."
            ),
            Lang::Zh => {
                format!("请先停止这些档案，否则会在它们仍在写入时拷贝浏览器数据：{names}。")
            }
        }
    }

    /// What a restore that was interrupted left, and what was done about it.
    ///
    /// The profiles are named because the recovery happens before the window
    /// opens: without the sentence, a directory that came back is
    /// indistinguishable from one that was never lost.
    pub fn data_recovered(&self, names: &str) -> String {
        match self.lang {
            Lang::En => {
                format!(
                    "An interrupted restore had set browser data aside; it was put back for {names}."
                )
            }
            Lang::Zh => format!("上次的数据恢复被中断，已为以下档案放回原有浏览器数据：{names}"),
        }
    }

    /// `1 core, 2 proxies and 3 profiles`: the phrase every report opens with.
    pub fn counts_phrase(&self, cores: usize, proxies: usize, profiles: usize) -> String {
        match self.lang {
            Lang::En => format!(
                "{cores} core{}, {proxies} prox{} and {profiles} profile{}",
                if cores == 1 { "" } else { "s" },
                if proxies == 1 { "y" } else { "ies" },
                if profiles == 1 { "" } else { "s" },
            ),
            Lang::Zh => format!("{cores} 个内核、{proxies} 个代理和 {profiles} 个档案"),
        }
    }

    pub fn export_wrote(&self, counts: &str, path: &str) -> String {
        match self.lang {
            Lang::En => format!("Wrote {counts} to {path}."),
            Lang::Zh => format!("已写入 {counts} 到 {path}。"),
        }
    }

    pub fn export_carries_credentials(&self) -> String {
        match self.lang {
            Lang::En => "The file carries the proxy credentials in plain text.".to_string(),
            Lang::Zh => "文件以明文保存代理凭据。".to_string(),
        }
    }

    pub fn export_had_no_credentials(&self) -> String {
        match self.lang {
            Lang::En => "It had no proxy credentials to leave out.".to_string(),
            Lang::Zh => "没有代理凭据需要省略。".to_string(),
        }
    }

    pub fn export_left_out(&self, proxies: usize) -> String {
        match self.lang {
            Lang::En => format!(
                "{proxies} prox{} had credentials, which were left out.",
                if proxies == 1 { "y" } else { "ies" },
            ),
            Lang::Zh => format!("有 {proxies} 个代理带有凭据，已省略。"),
        }
    }

    pub fn notes_differing(&self, names: &str) -> String {
        match self.lang {
            Lang::En => format!(
                "These were already here and differ from the file, so nothing was overwritten: {names}."
            ),
            Lang::Zh => format!("以下条目已存在且与文件不同，因此未覆盖：{names}。"),
        }
    }

    pub fn notes_missing_core(&self, names: &str) -> String {
        match self.lang {
            Lang::En => format!("Skipped, because the core they name is not here: {names}."),
            Lang::Zh => format!("已跳过，因为它们指定的内核不在本机：{names}。"),
        }
    }

    pub fn notes_missing_proxy(&self, names: &str) -> String {
        match self.lang {
            Lang::En => {
                format!("Imported with no proxy, because the proxy they name is not here: {names}.")
            }
            Lang::Zh => format!("已导入但没有代理，因为它们指定的代理不在本机：{names}。"),
        }
    }

    pub fn notes_repointed(&self, names: &str) -> String {
        match self.lang {
            Lang::En => format!(
                "Their browser data will use this machine's data directory, because the recorded one is not here: {names}."
            ),
            Lang::Zh => {
                format!("它们的浏览器数据将使用本机的数据目录，因为记录的位置不在本机：{names}。")
            }
        }
    }

    pub fn refused_not_stored(&self, names: &str) -> String {
        match self.lang {
            Lang::En => format!("Refused, and not stored: {names}."),
            Lang::Zh => format!("已被拒绝，未存入：{names}。"),
        }
    }

    pub fn credentials_left_out_clause(&self) -> String {
        match self.lang {
            Lang::En => "The file was written without proxy credentials, so a proxy it restored may need them typed in again.".to_string(),
            Lang::Zh => "该文件未写入代理凭据，因此还原后的代理可能需要重新输入密码。".to_string(),
        }
    }

    pub fn read_nothing_added(&self, source: &str) -> String {
        match self.lang {
            Lang::En => format!("Read {source}: nothing was added."),
            Lang::Zh => format!("已读取 {source}：没有新增。"),
        }
    }

    pub fn read_added(&self, source: &str, counts: &str) -> String {
        match self.lang {
            Lang::En => format!("Read {source}: {counts} were added."),
            Lang::Zh => format!("已读取 {source}：新增 {counts}。"),
        }
    }

    pub fn replaced(&self, counts: &str) -> String {
        match self.lang {
            Lang::En => format!("{counts} were replaced."),
            Lang::Zh => format!("替换掉了 {counts}。"),
        }
    }

    pub fn copy_nothing_out(&self) -> String {
        match self.lang {
            Lang::En => "Nothing was copied: no profile has browser data yet.".to_string(),
            Lang::Zh => "没有拷贝任何内容：还没有档案有浏览器数据。".to_string(),
        }
    }

    pub fn copy_nothing_in(&self, directory: &str) -> String {
        match self.lang {
            Lang::En => {
                format!("Nothing was copied: {directory} holds no browser data for these profiles.")
            }
            Lang::Zh => format!("没有拷贝任何内容：{directory} 里没有这些档案的浏览器数据。"),
        }
    }

    /// What a copy out or back in did, with the direction as a flag.
    pub fn copy_copied(&self, out: bool, profiles: usize, directory: &str, bytes: &str) -> String {
        match (self.lang, out) {
            (Lang::En, true) => {
                format!("Copied the browser data of {profiles} profiles to {directory} ({bytes}).")
            }
            (Lang::En, false) => format!(
                "Restored the browser data of {profiles} profiles from {directory} ({bytes})."
            ),
            (Lang::Zh, true) => {
                format!("已把 {profiles} 个档案的浏览器数据拷到 {directory}（{bytes}）。")
            }
            (Lang::Zh, false) => {
                format!("已从 {directory} 拷回 {profiles} 个档案的浏览器数据（{bytes}）。")
            }
        }
    }

    pub fn copy_skipped(&self, names: &str) -> String {
        match self.lang {
            Lang::En => format!("No browser data yet for: {names}."),
            Lang::Zh => format!("以下档案还没有浏览器数据：{names}。"),
        }
    }

    /// What a previous run left behind, in one line: the banner does not wrap.
    pub fn reclaim_left_running(&self, sessions: usize, names: &str) -> String {
        match self.lang {
            Lang::En => {
                let noun = if sessions == 1 {
                    "browser session"
                } else {
                    "browser sessions"
                };
                format!("a previous run left {sessions} {noun} running; {names}")
            }
            Lang::Zh => format!("上次运行留下了 {sessions} 个浏览器会话仍在运行：{names}"),
        }
    }

    pub fn reclaim_forced(&self) -> String {
        match self.lang {
            Lang::En => "had to be killed after ignoring the request to exit".to_string(),
            Lang::Zh => "在忽略退出请求后被强制结束".to_string(),
        }
    }

    pub fn reclaim_unnamed(&self) -> String {
        match self.lang {
            Lang::En => "an unnamed profile".to_string(),
            Lang::Zh => "未命名的档案".to_string(),
        }
    }

    pub fn reclaim_unresolved(&self, records: usize, names: &str) -> String {
        match self.lang {
            Lang::En => {
                let noun = if records == 1 {
                    "session record"
                } else {
                    "session records"
                };
                format!("{records} {noun} could not be reclaimed ({names})")
            }
            Lang::Zh => format!("{records} 条会话记录无法回收（{names}）"),
        }
    }

    pub fn reclaim_stale(&self, records: usize) -> String {
        match self.lang {
            Lang::En => {
                let noun = if records == 1 {
                    "stale session record"
                } else {
                    "stale session records"
                };
                format!("removed {records} {noun} whose processes had already exited")
            }
            Lang::Zh => format!("清除了 {records} 条进程已退出的过期会话记录"),
        }
    }

    /// One reclaimed session, named with the processes it had.
    pub fn reclaim_session(&self, who: &str, browser_pid: u32, xray_pid: Option<u32>) -> String {
        match (self.lang, xray_pid) {
            (Lang::En, Some(xray)) => {
                format!("{who} (browser pid {browser_pid}, xray pid {xray})")
            }
            (Lang::En, None) => format!("{who} (browser pid {browser_pid})"),
            (Lang::Zh, Some(xray)) => {
                format!("{who}（浏览器 pid {browser_pid}，Xray pid {xray}）")
            }
            (Lang::Zh, None) => format!("{who}（浏览器 pid {browser_pid}）"),
        }
    }

    pub fn reclaim_reason(&self, who: &str, reason: &str) -> String {
        match self.lang {
            Lang::En => format!("{who}: {reason}"),
            Lang::Zh => format!("{who}：{reason}"),
        }
    }
}
