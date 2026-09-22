//! settings messages.
use super::*;

impl Text {
    /// The title of the dialog that edits one setting.
    pub fn setting_dialog_title(&self, label: &str, effect: Effect) -> String {
        match (self.lang, effect) {
            (Lang::En, Effect::Now) => format!("{label} - takes effect now"),
            (Lang::En, Effect::NextStart) => format!("{label} - takes effect at the next start"),
            (Lang::En, Effect::Derived) => format!("{label} - follows the data directory"),
            (Lang::Zh, Effect::Now) => format!("{label}——立即生效"),
            (Lang::Zh, Effect::NextStart) => format!("{label}——下次启动生效"),
            (Lang::Zh, Effect::Derived) => format!("{label}——随数据目录变化"),
        }
    }

    /// What a saved setting reports, in the same three cases.
    pub fn setting_saved(&self, label: &str, effect: Effect) -> String {
        match (self.lang, effect) {
            (Lang::En, Effect::Now) => format!("Saved {label}. It takes effect now."),
            (Lang::En, Effect::NextStart) => {
                format!("Saved {label}. It takes effect at the next start.")
            }
            (Lang::En, Effect::Derived) => format!("Saved {label}. It follows the data directory."),
            (Lang::Zh, Effect::Now) => format!("已保存{label}，立即生效。"),
            (Lang::Zh, Effect::NextStart) => format!("已保存{label}，下次启动生效。"),
            (Lang::Zh, Effect::Derived) => format!("已保存{label}，随数据目录变化。"),
        }
    }

    pub fn setting_not_a_setting(&self, label: &str) -> String {
        match self.lang {
            Lang::En => format!("{label} is not a setting"),
            Lang::Zh => format!("{label} 不是可设置的项"),
        }
    }

    pub fn setting_not_editable(&self, label: &str) -> String {
        match self.lang {
            Lang::En => format!("{label} is not editable"),
            Lang::Zh => format!("{label} 不可修改"),
        }
    }

    pub fn setting_cannot_be_empty(&self, label: &str) -> String {
        match self.lang {
            Lang::En => format!("{label} cannot be empty"),
            Lang::Zh => format!("{label} 不能为空"),
        }
    }

    /// The value the field opens with, so a "change" does not look like a blank
    /// field that is about to erase what is there.
    pub fn setting_now(&self, now: &str) -> String {
        match self.lang {
            Lang::En => format!("Now: {now}"),
            Lang::Zh => format!("当前：{now}"),
        }
    }

    /// The accessible name of one theme chip, which needs the mode's name.
    pub fn theme_aria(&self, label: &str) -> String {
        match self.lang {
            Lang::En => format!("{label} theme"),
            Lang::Zh => format!("{label}主题"),
        }
    }

    pub fn theme_chosen(&self, label: &str) -> String {
        match self.lang {
            Lang::En => format!("Now showing the {label} theme."),
            Lang::Zh => format!("已切换到{label}主题。"),
        }
    }

    pub fn language_chosen(&self, label: &str) -> String {
        match self.lang {
            Lang::En => format!("Now showing the window in {label}."),
            Lang::Zh => format!("界面语言已切换为{label}。"),
        }
    }

    /// The sentence a change to the exit mode reports. It is not "now showing",
    /// because nothing about this choice is visible until a window is closed.
    pub fn exit_mode_chosen(&self, label: &str) -> String {
        match self.lang {
            Lang::En => format!("Closing the window will: {label}."),
            Lang::Zh => format!("关闭窗口时：{label}。"),
        }
    }

    /// Where a row's value came from, in the three shapes it has.
    pub fn source_set_by_env(&self, env: &str) -> String {
        match self.lang {
            Lang::En => format!("set by {env}"),
            Lang::Zh => format!("由 {env} 设置"),
        }
    }

    pub fn source_from(&self, source: &str) -> String {
        match self.lang {
            Lang::En => format!("from the {source}"),
            Lang::Zh => format!("来自{source}"),
        }
    }

    /// A stored value the environment is winning: shown, not hidden, because a
    /// stored setting that silently does nothing is a trap.
    pub fn source_shadowed(&self, value: &str) -> String {
        match self.lang {
            Lang::En => format!("the config file holds {value}, which this overrides"),
            Lang::Zh => format!("配置文件里是 {value}，被环境变量覆盖"),
        }
    }

    pub fn settings_unreadable(&self, error: &str) -> String {
        match self.lang {
            Lang::En => {
                format!("{error}; using defaults, and settings cannot be saved until it is fixed")
            }
            Lang::Zh => format!("{error}；正在使用默认值，修复之前无法保存设置"),
        }
    }

    /// A refusal to write the config file. `what` is the path or the setting, so
    /// one sentence covers "this file cannot be written" and "this setting
    /// cannot be stored".
    pub fn settings_cannot_write(&self, what: &str, error: &str) -> String {
        match self.lang {
            Lang::En => {
                format!("{what} cannot be written while the config file is unreadable: {error}")
            }
            Lang::Zh => format!("配置文件不可读时无法写入{what}：{error}"),
        }
    }

    /// A relative path is shown with the directory it resolves against, so the
    /// window never shows a path the user cannot find.
    pub fn setting_relative_to(&self, path: &str, base: &str) -> String {
        match self.lang {
            Lang::En => format!("{path} (relative to {base})"),
            Lang::Zh => format!("{path}（相对于 {base}）"),
        }
    }

    pub fn config_write_failed(&self, error: &str) -> String {
        match self.lang {
            Lang::En => format!("The configuration could not be written. {error}"),
            Lang::Zh => format!("无法写入配置。{error}"),
        }
    }

    /// Once, when the database is still in the directory this build no longer
    /// uses.
    pub fn data_dir_moved_notice(&self, data_dir: &str, legacy: &str) -> String {
        match self.lang {
            Lang::En => format!(
                "the data directory moved to {data_dir}; the database in {legacy} is still there, and the Settings page points at it in one field"
            ),
            Lang::Zh => format!(
                "数据目录已移到 {data_dir}；{legacy} 里的数据库仍然存在，设置页会用一项指向它"
            ),
        }
    }

    pub fn config_read_failed(&self, path: &str, error: &str) -> String {
        match self.lang {
            Lang::En => format!("could not read {path}: {error}"),
            Lang::Zh => format!("无法读取 {path}：{error}"),
        }
    }

    pub fn config_create_failed(&self, path: &str, error: &str) -> String {
        match self.lang {
            Lang::En => format!("could not create {path}: {error}"),
            Lang::Zh => format!("无法创建 {path}：{error}"),
        }
    }

    pub fn config_encode_failed(&self, error: &str) -> String {
        match self.lang {
            Lang::En => format!("could not encode the settings: {error}"),
            Lang::Zh => format!("无法序列化设置：{error}"),
        }
    }

    pub fn config_file_write_failed(&self, path: &str, error: &str) -> String {
        match self.lang {
            Lang::En => format!("could not write {path}: {error}"),
            Lang::Zh => format!("无法写入 {path}：{error}"),
        }
    }

    /// An old config file that named a data directory this build no longer
    /// reads, and the one way to keep using it.
    pub fn data_dir_no_longer_in_config(&self, old: &str, env: &str, resolved: &str) -> String {
        match self.lang {
            Lang::En => format!(
                "the config file used to say the data directory is {old}; this build keeps its config inside the data directory, so it uses {resolved} - set {env}={old} to go back"
            ),
            Lang::Zh => format!(
                "配置文件里记录的数据目录是 {old}；本版本把配置文件放在数据目录内，因此使用 {resolved} —— 若要回到原目录，请设置 {env}={old}"
            ),
        }
    }

    /// Settings were read from the old location but could not be written to the
    /// new one. Not "unreadable": the values in force are the ones that were
    /// read, and nothing is lost while the old file stays where it is.
    pub fn settings_not_moved(&self, from: &str, to: &str, error: &str) -> String {
        match self.lang {
            Lang::En => format!(
                "the settings in {from} could not be moved to {to}, so they are still being read from the old file: {error}"
            ),
            Lang::Zh => format!("{from} 里的设置无法搬到 {to}，目前仍从旧文件读取：{error}"),
        }
    }
}
