//! cores messages.
use super::*;

impl Text {
    /// Confirming the removal of a core, which names what will be gone.
    pub fn delete_core_confirm(&self, name: &str, version: &str) -> String {
        match self.lang {
            Lang::En => format!("\"{name}\" ({version}) will be removed."),
            Lang::Zh => format!("将删除「{name}」（{version}）。"),
        }
    }

    /// Refusing it, and saying what to do first.
    pub fn core_in_use(&self, name: &str, used_by: &str) -> String {
        match self.lang {
            Lang::En => format!(
                "\"{name}\" is used by {used_by}. Point those profiles at another core first."
            ),
            Lang::Zh => format!("「{name}」正被 {used_by} 使用。请先把这些档案指向别的内核。"),
        }
    }

    pub fn missing_core(&self, id: &str) -> String {
        match self.lang {
            Lang::En => format!("(missing core {id})"),
            Lang::Zh => format!("（内核 {id} 已缺失）"),
        }
    }

    /// A core's compatibility line: which generation it belongs to, and whether
    /// the engine honours the switch that separates them.
    ///
    /// The two halves are not a translated fragment of the state model: the
    /// generation is a name (`Chrome 144+`) and the second half is a consequence
    /// in the reader's own language. The state never spells the English
    /// "honoured" and this page never reads one.
    pub fn core_compatibility(&self, generation: &str, exclusions_honoured: bool) -> String {
        match (self.lang, exclusions_honoured) {
            (Lang::En, true) => format!("{generation} · excludes spoofing switches"),
            (Lang::En, false) => format!("{generation} · ignores exclusion switches"),
            (Lang::Zh, true) => format!("{generation} · 可排除指纹开关"),
            (Lang::Zh, false) => format!("{generation} · 会忽略排除开关"),
        }
    }

    /// What the profile form says about the core in force: which generation it
    /// is, and whether the exclusions the form offers will be applied.
    pub fn core_generation_note(&self, generation: &str, exclusions_honoured: bool) -> String {
        match (self.lang, exclusions_honoured) {
            (Lang::En, true) => format!(
                "{generation}; this engine honours the exclusion switch, so the exclusions below are applied"
            ),
            (Lang::En, false) => format!(
                "{generation}; this engine ignores the exclusion switch, so the exclusions below have no effect"
            ),
            (Lang::Zh, true) => {
                format!("{generation}；该引擎支持排除开关，下面的排除项会生效")
            }
            (Lang::Zh, false) => {
                format!("{generation}；该引擎会忽略排除开关，下面的排除项不会生效")
            }
        }
    }

    /// Confirming a copy, naming what went to the clipboard.
    pub fn core_path_copied(&self, path: &str) -> String {
        match self.lang {
            Lang::En => format!("Copied {path}."),
            Lang::Zh => format!("已复制 {path}。"),
        }
    }

    /// The whole of that reading, for the tooltip on the compatibility cell.
    ///
    /// What the difference does to a profile, not what the switch is called: the
    /// reader is choosing a core, and the consequence is the part that decides.
    pub fn core_exclusions_help(&self, major: u32, exclusions_honoured: bool) -> String {
        match (self.lang, exclusions_honoured) {
            (Lang::En, true) => format!(
                "Major {major} honours the exclusion switch, so a profile on this core can turn individual surfaces off."
            ),
            (Lang::En, false) => format!(
                "Major {major} ignores the exclusion switch: a profile on this core keeps the whole spoofed set, and the form does not offer exclusions."
            ),
            (Lang::Zh, true) => {
                format!("主版本 {major} 支持排除开关：用它启动的档案可以单独关掉某些指纹项。")
            }
            (Lang::Zh, false) => format!(
                "主版本 {major} 会忽略排除开关：用它启动的档案保留全部指纹伪装，表单也不会提供排除选项。"
            ),
        }
    }

    /// The tooltip on a core's path: the whole path, which the column truncates.
    pub fn core_path_help(&self, path: &str) -> String {
        match self.lang {
            Lang::En => format!("{path} - use the menu beside it to copy the path"),
            Lang::Zh => format!("{path}——可用旁边的按钮复制路径"),
        }
    }

    pub fn core_added(&self, name: &str, version: &str, major: u32) -> String {
        match self.lang {
            Lang::En => format!("Added {name} ({version}, major {major})"),
            Lang::Zh => format!("已添加 {name}（{version}，主版本 {major}）"),
        }
    }

    pub fn core_saved(&self, name: &str, version: &str, major: u32) -> String {
        match self.lang {
            Lang::En => format!("Saved {name} ({version}, major {major})"),
            Lang::Zh => format!("已保存 {name}（{version}，主版本 {major}）"),
        }
    }

    pub fn core_refreshed(&self, name: &str, version: &str, major: u32) -> String {
        match self.lang {
            Lang::En => format!("{name} is {version} (major {major})"),
            Lang::Zh => format!("{name} 是 {version}（主版本 {major}）"),
        }
    }

    pub fn core_deleted(&self, name: &str) -> String {
        match self.lang {
            Lang::En => format!("Deleted core {name}"),
            Lang::Zh => format!("已删除内核 {name}"),
        }
    }

    pub fn core_not_found(&self, id: &str) -> String {
        match self.lang {
            Lang::En => format!("core {id} not found"),
            Lang::Zh => format!("未找到内核 {id}"),
        }
    }

    pub fn core_has_no_version(&self, name: &str) -> String {
        match self.lang {
            Lang::En => format!(
                "{name} has no detected version, so there are no switches to check against; give it a version before verifying"
            ),
            Lang::Zh => {
                format!("{name} 未检测到版本，因此没有可核对的开关；请先给它一个版本再校验")
            }
        }
    }

    /// A core the picker offers, whose version was never read.
    pub fn core_choice_unknown_version(&self, name: &str) -> String {
        match self.lang {
            Lang::En => format!("{name} (version unknown)"),
            Lang::Zh => format!("{name}（版本未知）"),
        }
    }

    /// A core whose name does not already say which major it is.
    pub fn core_choice_major(&self, name: &str, major: &str) -> String {
        match self.lang {
            Lang::En => format!("{name} (Chrome {major})"),
            Lang::Zh => format!("{name}（Chrome {major}）"),
        }
    }

    pub fn core_list_failed_frame(&self, error: &str) -> String {
        match self.lang {
            Lang::En => format!("could not read browser cores: {error}"),
            Lang::Zh => format!("无法读取浏览器内核：{error}"),
        }
    }

    pub fn core_save_failed_frame(&self, error: &str) -> String {
        match self.lang {
            Lang::En => format!("could not store browser core: {error}"),
            Lang::Zh => format!("无法保存浏览器内核：{error}"),
        }
    }

    pub fn core_update_failed_frame(&self, error: &str) -> String {
        match self.lang {
            Lang::En => format!("could not update browser core: {error}"),
            Lang::Zh => format!("无法更新浏览器内核：{error}"),
        }
    }

    /// A core whose binary answers nothing about its version.
    pub fn core_no_usable_version_notice(&self, executable: &str, env: &str) -> String {
        match self.lang {
            Lang::En => format!(
                "{executable} did not report a usable version; set {env} so fingerprint switches can be checked"
            ),
            Lang::Zh => {
                format!("{executable} 没有报告可用的版本；请设置 {env}，否则无法核对指纹开关")
            }
        }
    }

    /// Cores whose executable is no longer on disk, named in one line.
    pub fn core_executable_missing_notice(&self, cores: &str, env: &str) -> String {
        match self.lang {
            Lang::En => format!("browser core executable missing: {cores}; set {env} and restart"),
            Lang::Zh => format!("浏览器内核的可执行文件已缺失：{cores}；请设置 {env} 后重启"),
        }
    }

    pub fn core_updated_notice(&self, cores: &str) -> String {
        match self.lang {
            Lang::En => format!("browser core updated: {cores}"),
            Lang::Zh => format!("浏览器内核已更新：{cores}"),
        }
    }

    /// The startup notice when nothing on this machine can launch a browser.
    pub fn no_core_found_notice(&self, env: &str) -> String {
        match self.lang {
            Lang::En => {
                format!(
                    "no browser core found; set {env} to a fingerprint-chromium executable and restart"
                )
            }
            Lang::Zh => format!(
                "没有找到浏览器内核；请把 {env} 指向 fingerprint-chromium 可执行文件，然后重启"
            ),
        }
    }
}
