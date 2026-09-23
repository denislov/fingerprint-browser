//! profiles messages.
use super::*;

impl Text {
    /// A page's name, with the note that it has not been built yet.
    pub fn nav_soon(&self, label: &str) -> String {
        match self.lang {
            Lang::En => format!("{label} (soon)"),
            Lang::Zh => format!("{label}（即将可用）"),
        }
    }

    /// The header above the profiles list, which says how many are shown.
    pub fn profiles_showing(&self, visible: usize, total: usize) -> String {
        match self.lang {
            Lang::En => format!("Showing {visible} of {total} profiles."),
            Lang::Zh => format!("显示 {visible} / {total} 个档案。"),
        }
    }

    /// The same, naming the term that is hiding the rest.
    pub fn profiles_showing_filtered(&self, visible: usize, total: usize, filter: &str) -> String {
        match self.lang {
            Lang::En => format!("Showing {visible} of {total} profiles, filtered by \"{filter}\"."),
            Lang::Zh => format!("显示 {visible} / {total} 个档案，筛选词为「{filter}」。"),
        }
    }

    /// With no filter on, the header says what a profile is instead of counting.
    pub fn profiles_total(&self, total: usize) -> String {
        match self.lang {
            Lang::En => format!("{total} profiles."),
            Lang::Zh => format!("共 {total} 个档案。"),
        }
    }

    /// The empty list that is empty because of the filter, not for lack of
    /// profiles - which is the only one that can be undone from the list.
    pub fn empty_no_match(&self, filter: &str, total: usize) -> String {
        match self.lang {
            Lang::En => {
                format!("No profile answers to \"{filter}\"; all {total} are hidden by it.")
            }
            Lang::Zh => format!("没有档案匹配「{filter}」——全部 {total} 个都被筛掉了。"),
        }
    }

    /// Confirming the removal of a profile, which says what survives it.
    pub fn delete_profile_confirm(&self, name: &str) -> String {
        match self.lang {
            Lang::En => {
                format!("\"{name}\" will be removed from the list. Its browser data stays on disk.")
            }
            Lang::Zh => format!("「{name}」将从列表中删除。它的浏览器数据仍保留在磁盘上。"),
        }
    }

    /// The accessible name of one profile row.
    ///
    /// A row is read as one thing rather than as the four cells it is drawn as:
    /// the name, then the state, then the engine under it. The state and the core
    /// are in the row's other columns, and a reader who cannot see the row is
    /// owed them in the order the row draws them.
    pub fn profile_row_aria(&self, name: &str, state: &str, core: &str) -> String {
        match self.lang {
            Lang::En => format!("{name}, {state}, core {core}"),
            Lang::Zh => format!("{name}，{state}，内核 {core}"),
        }
    }

    /// The accessible name of one row's overflow menu.
    ///
    /// It names the row, because a list of menus all called "More" is a list
    /// that says nothing when it is read aloud.
    pub fn row_more(&self, name: &str) -> String {
        match self.lang {
            Lang::En => format!("More actions for \"{name}\""),
            Lang::Zh => format!("「{name}」的更多操作"),
        }
    }

    /// The accessible name of the row's one visible action, which names the row
    /// for the same reason.
    pub fn row_action(&self, action: &str, name: &str) -> String {
        match self.lang {
            Lang::En => format!("{action} \"{name}\""),
            Lang::Zh => format!("{action}「{name}」"),
        }
    }

    /// A state carrying a message from the runtime, which is not translated.
    pub fn profile_state_message(&self, state: &str, message: &str) -> String {
        match self.lang {
            Lang::En => format!("{state}: {message}"),
            Lang::Zh => format!("{state}：{message}"),
        }
    }

    pub fn warning_line(&self, warning: &str) -> String {
        match self.lang {
            Lang::En => format!("warning: {warning}"),
            Lang::Zh => format!("警告：{warning}"),
        }
    }

    pub fn error_line(&self, error: &str) -> String {
        match self.lang {
            Lang::En => format!("error: {error}"),
            Lang::Zh => format!("错误：{error}"),
        }
    }

    pub fn log_written_to(&self, path: &str) -> String {
        match self.lang {
            Lang::En => format!("Also written to {path}"),
            Lang::Zh => format!("同时写入 {path}"),
        }
    }

    pub fn log_not_written(&self, error: &str) -> String {
        match self.lang {
            Lang::En => format!("Not written to a file: {error}"),
            Lang::Zh => format!("未写入文件：{error}"),
        }
    }

    /// The log page, empty because of the filter rather than for lack of lines.
    pub fn log_none_of_kind(&self, noun: &str, recorded: usize) -> String {
        match self.lang {
            Lang::En => format!(
                "No {noun} lines; {recorded} were recorded - switch the filter to All to see them"
            ),
            Lang::Zh => format!("没有{noun}行；已记录 {recorded} 行——把筛选切到「全部」即可看到"),
        }
    }

    /// The accessible name of a log row's expand control: how much is hidden,
    /// because "Show all" alone does not say whether it is worth opening.
    pub fn log_expand_aria(&self, hidden: usize) -> String {
        match self.lang {
            Lang::En => format!("Show the rest of this line ({hidden} more characters)"),
            Lang::Zh => format!("展开这一行的其余部分（还有 {hidden} 个字符）"),
        }
    }

    pub fn log_collapse_aria(&self) -> String {
        match self.lang {
            Lang::En => "Collapse this line".to_string(),
            Lang::Zh => "收起这一行".to_string(),
        }
    }

    /// One log line, whole, for the row's accessible name and for copying.
    ///
    /// The same shape the Copy button writes, so what is read aloud and what
    /// lands on the clipboard cannot drift apart.
    pub fn log_line(&self, who: &str, level: &str, message: &str) -> String {
        match self.lang {
            Lang::En => format!("{who} [{level}] {message}"),
            Lang::Zh => format!("{who} [{level}] {message}"),
        }
    }

    /// The panel's log view, newest first, with how many lines it holds.
    pub fn panel_log_title(&self, count: usize) -> String {
        match self.lang {
            Lang::En => format!("This profile, newest first ({count})"),
            Lang::Zh => format!("该档案，最新的在最前（{count}）"),
        }
    }

    /// A fingerprint reading that found claims the browser disagreed with.
    pub fn claims_not_reproduced(&self, count: usize) -> String {
        match self.lang {
            Lang::En => format!("{count} claim(s) the browser did not reproduce:"),
            Lang::Zh => format!("浏览器未能复现的声明有 {count} 项："),
        }
    }

    /// One of those claims: what was claimed, and what was read back.
    pub fn claim_line(&self, name: &str, expected: &str, observed: &str) -> String {
        match self.lang {
            Lang::En => format!("  {name}: expected {expected}, observed {observed}"),
            Lang::Zh => format!("  {name}：期望 {expected}，实际 {observed}"),
        }
    }

    /// The browser could not be asked at all. The reason is the runtime's.
    pub fn fingerprint_unreadable(&self, reason: &str) -> String {
        match self.lang {
            Lang::En => format!("Could not read the fingerprint: {reason}"),
            Lang::Zh => format!("无法读取指纹：{reason}"),
        }
    }

    pub fn effective_args(&self, count: usize) -> String {
        match self.lang {
            Lang::En => format!("Effective args ({count})"),
            Lang::Zh => format!("生效参数（{count}）"),
        }
    }

    /// How long ago something happened, to the second.
    pub fn elapsed_seconds(&self, seconds: u64) -> String {
        match self.lang {
            Lang::En => format!("{seconds}s ago"),
            Lang::Zh => format!("{seconds} 秒前"),
        }
    }

    /// The directory the desktop environment was asked to open.
    pub fn opened(&self, path: &str) -> String {
        match self.lang {
            Lang::En => format!("Opened {path}"),
            Lang::Zh => format!("已打开 {path}"),
        }
    }

    /// A row whose record is gone from storage: another window, or the same one
    /// after a delete.
    pub fn profile_gone(&self, id: &str) -> String {
        match self.lang {
            Lang::En => format!("profile {id} is no longer there"),
            Lang::Zh => format!("档案 {id} 已不存在"),
        }
    }

    /// The protocols that have no form, only a link.
    pub fn link_only_protocols(&self, kinds: &str) -> String {
        match self.lang {
            Lang::En => format!("{kinds} are not filled in here:"),
            Lang::Zh => format!("{kinds} 不在这里填写："),
        }
    }

    pub fn profile_created(&self, name: &str) -> String {
        match self.lang {
            Lang::En => format!("Created {name}"),
            Lang::Zh => format!("已创建 {name}"),
        }
    }

    /// The name a duplicate gets, before it is made unique by the service.
    pub fn profile_copy_name(&self, name: &str) -> String {
        match self.lang {
            Lang::En => format!("{name} copy"),
            Lang::Zh => format!("{name} 的副本"),
        }
    }

    pub fn next_profile_name(&self, number: usize) -> String {
        match self.lang {
            Lang::En => format!("Profile {number}"),
            Lang::Zh => format!("档案 {number}"),
        }
    }

    pub fn log_launching(&self, arguments: usize) -> String {
        match self.lang {
            Lang::En => format!("launching with {arguments} arguments"),
            Lang::Zh => format!("以 {arguments} 个参数启动"),
        }
    }

    /// The whole line, built in one place: the parts a browser is started with
    /// are optional, and stitching translated fragments together is how a
    /// sentence ends up grammatical in neither language.
    pub fn log_browser_started(
        &self,
        browser_pid: u32,
        cdp_port: u16,
        socks_port: Option<u16>,
        xray_pid: Option<u32>,
    ) -> String {
        match self.lang {
            Lang::En => {
                let mut line = format!("browser started (pid {browser_pid}, cdp port {cdp_port}");
                if let Some(port) = socks_port {
                    line.push_str(&format!(", socks port {port}"));
                }
                if let Some(pid) = xray_pid {
                    line.push_str(&format!(", xray pid {pid}"));
                }
                line.push(')');
                line
            }
            Lang::Zh => {
                let mut line = format!("浏览器已启动（pid {browser_pid}，CDP 端口 {cdp_port}");
                if let Some(port) = socks_port {
                    line.push_str(&format!("，SOCKS 端口 {port}"));
                }
                if let Some(pid) = xray_pid {
                    line.push_str(&format!("，Xray pid {pid}"));
                }
                line.push('）');
                line
            }
        }
    }

    pub fn log_crashed(&self, component: &str, message: &str) -> String {
        match self.lang {
            Lang::En => format!("{component} crashed: {message}"),
            Lang::Zh => format!("{component} 崩溃：{message}"),
        }
    }

    pub fn log_failed(&self, message: &str) -> String {
        match self.lang {
            Lang::En => format!("failed: {message}"),
            Lang::Zh => format!("失败：{message}"),
        }
    }

    pub fn log_write_failed(&self, error: &str) -> String {
        match self.lang {
            Lang::En => format!("the activity log could not be written: {error}"),
            Lang::Zh => format!("活动日志写入失败：{error}"),
        }
    }

    pub fn verification_busy(&self, id: &str) -> String {
        match self.lang {
            Lang::En => format!("profile {id} is already being verified"),
            Lang::Zh => format!("档案 {id} 正在校验中"),
        }
    }

    /// What a profile is busy with, for the sentence that refuses an operation
    /// needing it to itself. A phrase rather than a whole sentence, because the
    /// two refusals below put it in different places in the two languages.
    pub fn busy_starting(&self) -> &'static str {
        match self.lang {
            Lang::En => "starting up",
            Lang::Zh => "正在启动",
        }
    }

    pub fn busy_copying(&self) -> &'static str {
        match self.lang {
            Lang::En => "having its browser data copied",
            Lang::Zh => "正在复制浏览器数据",
        }
    }

    /// The refusal of an operation that needs one profile to itself, which a copy
    /// or a start it is already busy with does not leave.
    pub fn profile_busy(&self, name: &str, what: &str) -> String {
        match self.lang {
            Lang::En => format!("{name} is {what}; try again when that finishes."),
            Lang::Zh => format!("{name}{what}，请等它结束后再试。"),
        }
    }

    /// The refusal of a second configuration task while one is running.
    ///
    /// One sentence for all four rather than one per task: there is only ever one
    /// in flight, the reader has just clicked the button, and which of the four is
    /// running is visible in the window rather than in this sentence.
    pub fn maintenance_busy(&self) -> String {
        match self.lang {
            Lang::En => {
                "Another configuration task is still running; wait for it to finish.".to_string()
            }
            Lang::Zh => "另一个配置任务仍在进行，请等它结束。".to_string(),
        }
    }

    /// The refusal of an operation that needs the whole installation to itself.
    ///
    /// Named rather than counted: the reader has to know which profile to wait
    /// for, and a replacement that went ahead would leave a copy of that profile
    /// writing into directories the new configuration no longer describes.
    pub fn install_busy(&self, name: &str, what: &str) -> String {
        match self.lang {
            Lang::En => {
                format!("Cannot replace the configuration while {name} is {what}.")
            }
            Lang::Zh => format!("{name}{what}，无法替换配置。"),
        }
    }

    pub fn verification_claims_toast(&self, count: usize) -> String {
        match self.lang {
            Lang::En => format!("fingerprint read back with {count} claim(s) not confirmed"),
            Lang::Zh => format!("指纹已读回：有 {count} 项声明未被确认"),
        }
    }

    pub fn verification_claims_details(&self, count: usize) -> String {
        match self.lang {
            Lang::En => {
                format!("{count} claim(s) the browser did not reproduce; see Runtime Details")
            }
            Lang::Zh => format!("浏览器未能复现 {count} 项声明；详见运行详情"),
        }
    }

    pub fn fingerprint_read_failed(&self, reason: &str) -> String {
        match self.lang {
            Lang::En => format!("Fingerprint could not be read: {reason}"),
            Lang::Zh => format!("无法读取指纹：{reason}"),
        }
    }

    pub fn profile_not_found(&self, id: &str) -> String {
        match self.lang {
            Lang::En => format!("profile {id} not found"),
            Lang::Zh => format!("未找到档案 {id}"),
        }
    }

    pub fn verify_needs_browser(&self) -> String {
        match self.lang {
            Lang::En => "the browser must be running before it can be verified".to_string(),
            Lang::Zh => "浏览器必须在运行才能校验".to_string(),
        }
    }

    /// Why a start could not even be checked: no such proxy, or one that cannot
    /// be tested at all.
    pub fn start_not_checked(&self, profile: &str, reason: &str) -> String {
        match self.lang {
            Lang::En => {
                format!("Did not start {profile}: its proxy could not be checked. {reason}")
            }
            Lang::Zh => format!("未启动 {profile}：无法检查它使用的代理。{reason}"),
        }
    }

    /// A profile that is used by nobody, or by the profiles named.
    pub fn used_by(&self, count: usize, names: &str) -> String {
        match (self.lang, count) {
            (Lang::En, 1) => format!("used by {names}"),
            (Lang::En, n) => format!("used by {n} profiles"),
            (Lang::Zh, _) => format!("被 {names} 使用"),
        }
    }

    /// A verification result in one word, for the profile row.
    pub fn verification_short(&self, disagreements: Option<usize>) -> String {
        match (self.lang, disagreements) {
            (Lang::En, None) => self.fingerprint_confirmed_short.to_string(),
            (Lang::En, Some(1)) => "1 claim not confirmed".to_string(),
            (Lang::En, Some(n)) => format!("{n} claims not confirmed"),
            (Lang::Zh, None) => self.fingerprint_confirmed_short.to_string(),
            (Lang::Zh, Some(n)) => format!("{n} 项声明未确认"),
        }
    }

    /// Joins clauses into one sentence. Chinese does not put a space after its
    /// full stop, and the clauses already carry their own punctuation.
    pub fn join_sentences(&self, parts: &[String]) -> String {
        match self.lang {
            Lang::En => parts.join(" "),
            Lang::Zh => parts.concat(),
        }
    }

    /// The word between the last two items of a two-item list.
    pub fn list_conjunction(&self) -> &'static str {
        match self.lang {
            Lang::En => "and",
            Lang::Zh => "和",
        }
    }

    /// A list, capped: a toast is not the place for twenty names.
    pub fn listed_more(&self, shown: &str, more: usize) -> String {
        match self.lang {
            Lang::En => format!("{shown}, and {more} more"),
            Lang::Zh => format!("{shown}，另有 {more} 项"),
        }
    }

    /// Names as one phrase: `a`, `a and b`, `a, and 2 more`.
    ///
    /// The conjunction and the capped form are each language's own, which is why
    /// this is not a join at the call site: Chinese does not put spaces around its
    /// conjunction, and the "and N more" tail is written rather than assembled.
    pub fn names(&self, entries: &[String]) -> String {
        match entries {
            [] => String::new(),
            [only] => only.clone(),
            [first, second] => format!("{first} {} {second}", self.list_conjunction()),
            [first, rest @ ..] => self.listed_more(first, rest.len()),
        }
    }

    pub fn directory_not_created_yet(&self, path: &str) -> String {
        match self.lang {
            Lang::En => format!(
                "{path} is not a directory yet; start the profile once and the browser will create it"
            ),
            Lang::Zh => format!("{path} 还不是目录；先启动一次该档案，浏览器就会创建它"),
        }
    }
}
