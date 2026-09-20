//! The window's words, in each language it speaks.
//!
//! A fixed label lives in exactly one place: a line in [`catalog!`], English
//! beside Chinese. That is what makes the two languages impossible to drift
//! apart in *shape* - an added field is a compile error in both tables at once,
//! because the macro generates both from the same list - and a test
//! ([`tests::every_message_is_translated`]) catches the other failure, a field
//! that was copied across and never translated.
//!
//! A sentence that a number or a name has to be slotted into is not a field but
//! a method on [`Text`], because word order is part of the translation and the
//! two languages do not agree on where the number goes. English also needs
//! plurals and Chinese does not; the method is where that stops being a caller's
//! problem, and where a phrase like "takes effect now" can be written out per
//! language instead of being assembled from translated fragments - which is how
//! a sentence ends up grammatical in neither language.
//!
//! What is deliberately **not** here, and stays inline at its call site:
//!
//! - names rather than sentences: `SOCKS5`, `Shadowsocks`, `Chrome`, `Windows`,
//!   `Fingerprint Browser`, the `FP_BROWSER_*` variables, the JavaScript property
//!   names shown on the Runtime Details panel (`Brand`, `Platform`, `Language`,
//!   `Timezone`), and the `ss://, vmess://, vless:// or trojan://` example. A
//!   translated protocol name is a worse bug than an untranslated one.
//! - the classification text of errors raised by `runtime`, `domain` and
//!   `storage`. Those layers describe what the operating system or the network
//!   said, in their own terms; the app frames them - and the frame *is*
//!   translated - but it does not re-write their account of the fault. See
//!   `docs/i18n.md`.

use crate::settings::Effect;
use runtime::FaultClass;

/// Which language the window speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Lang {
    /// The language the program was written in, and the default.
    #[default]
    En,
    Zh,
}

impl Lang {
    /// Every language, in the order the switch shows them.
    pub const ALL: [Lang; 2] = [Self::En, Self::Zh];

    /// The name it is stored under, and parsed from.
    pub fn code(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Zh => "zh",
        }
    }

    /// The name the switch shows, **in that language**. Someone who cannot read
    /// the language they are in has to be able to find their way out of it, and
    /// an endonym is the only label that works for that. These are therefore not
    /// translated, which is why they are not in the catalog.
    pub fn label(self) -> &'static str {
        match self {
            Self::En => "English",
            Self::Zh => "简体中文",
        }
    }

    /// Reads a stored name. A region is not a different language as far as this
    /// build is concerned (`zh-Hans`, `zh-CN`), and anything it does not know
    /// falls back to English rather than refusing to start.
    pub fn from_code(code: &str) -> Self {
        let code = code.trim().to_ascii_lowercase();
        if code == "zh" || code.starts_with("zh-") || code.starts_with("zh_") {
            Self::Zh
        } else {
            Self::En
        }
    }
}

/// Declares the catalog once and generates both tables from it.
///
/// One line per message, English beside Chinese, so a reviewer sees the pair they
/// are checking and a new message cannot be added to one language only. The
/// generated `PAIRS` is what the translation test walks, which keeps that test
/// honest as the catalog grows: it never needs editing per message.
macro_rules! catalog {
    ($( $field:ident => $en:expr => $zh:expr; )*) => {
        /// The window's text for one language.
        pub struct Text {
            lang: Lang,
            $( pub $field: &'static str, )*
        }

        impl Text {
            const EN: Self = Self {
                lang: Lang::En,
                $( $field: $en, )*
            };

            const ZH: Self = Self {
                lang: Lang::Zh,
                $( $field: $zh, )*
            };

            /// Every message as (name, English, Chinese), for the tests.
            #[cfg(test)]
            const PAIRS: &'static [(&'static str, &'static str, &'static str)] = &[
                $( (stringify!($field), $en, $zh), )*
            ];
        }
    };
}

catalog! {
    // ---- the shell ----
    quit => "Quit" => "退出";

    nav_profiles => "Profiles" => "档案";
    nav_proxies => "Proxies" => "代理";
    nav_cores => "Browser Cores" => "浏览器内核";
    nav_log => "Log" => "日志";
    nav_settings => "Settings" => "设置";

    // ---- the profiles page ----
    profiles_intro => "Each profile owns its seed, data directory and browser process."
        => "每个档案拥有自己的指纹种子、数据目录和浏览器进程。";
    profiles_filter_placeholder => "Filter by name, seed, core or proxy"
        => "按名称、种子、内核或代理筛选";
    profiles_filter_aria => "Filter profiles" => "筛选档案";
    new_profile => "New Profile" => "新建档案";
    empty_no_profiles => "No profiles yet. Create one to start a browser."
        => "还没有档案。新建一个即可启动浏览器。";
    clear_filter => "Clear filter" => "清除筛选";

    log_level_info => "info" => "信息";
    log_level_warning => "warning" => "警告";
    log_level_error => "error" => "错误";

    // ---- the Settings page ----
    settings_intro => "The value in force and where it came from. An environment variable wins over the config file, and the row says so."
        => "当前生效的值及其来源。环境变量优先于配置文件，行上会注明。";
    change => "Change" => "修改";
    save => "Save" => "保存";
    cancel => "Cancel" => "取消";
    dismiss => "Dismiss" => "知道了";

    // ---- the interface card ----
    interface_title => "Interface" => "界面";
    interface_body => "Both take effect at once, and both are kept for the next start."
        => "两项都立即生效，并保留到下次启动。";
    appearance_title => "Appearance" => "外观";
    theme_dark => "Dark" => "深色";
    theme_light => "Light" => "浅色";
    language_title => "Language" => "语言";

    // ---- the Settings rows ----
    setting_data_dir => "Data directory" => "数据目录";
    setting_xray_executable => "Xray executable" => "Xray 可执行文件";
    setting_echo_url => "Proxy test endpoint" => "代理测试端点";
    setting_chromium_bin => "Chromium binary" => "Chromium 可执行文件";
    setting_chromium_major => "Chromium major override" => "Chromium 主版本号覆盖";
    setting_config_file => "Config file" => "配置文件";
    setting_runtime_dir => "Runtime directory" => "运行时目录";

    // The short form beside a row, and inside the sentence a change reports.
    effect_now => "now" => "立即";
    just_now => "now" => "刚刚";
    effect_next_start => "next start" => "下次启动";
    effect_derived => "derived from the data directory" => "由数据目录推导";

    source_environment => "environment" => "环境变量";
    source_config_file => "config file" => "配置文件";
    source_default => "default" => "默认值";
    source_derived => "derived from the data directory" => "由数据目录推导";

    log_filter_all => "All" => "全部";
    log_filter_warnings => "Warnings" => "警告";
    log_filter_errors => "Errors" => "错误";
    log_filter_noun_all => "anything" => "任何内容";
    log_filter_noun_warnings => "warning or error" => "警告或错误";
    log_filter_noun_errors => "error" => "错误";

    details_tab_details => "Details" => "详情";
    details_tab_args => "Args" => "参数";
    details_tab_log => "Log" => "日志";

    state_stopped => "Stopped" => "已停止";
    state_starting => "Starting" => "启动中";
    state_running => "Running" => "运行中";
    state_stopping => "Stopping" => "停止中";
    state_failed => "Failed" => "失败";
    state_crashed => "Crashed" => "已崩溃";

    value_not_set_path => "not set; discovered on PATH" => "未设置；在 PATH 中查找";
    value_not_set_versions => "not set; read from each binary" => "未设置；从各可执行文件读取";

    help_echo_url => "asked to report the address a connection left from, so it sees that address; asked only when you run a proxy test"
        => "被要求报告连接从哪个地址出去，因此它会看到该地址；只有运行代理测试时才会被问到";
    help_chromium_bin => "cores discovered from it are listed on the Browser Cores page"
        => "由它发现的内核会列在「浏览器内核」页";
    help_chromium_major => "used only for binaries that answer nothing to --version"
        => "仅用于对 --version 无输出的可执行文件";
    help_config_file => "the data directory and Xray executable are stored here, outside the data directory, so changing the data directory cannot lose them"
        => "数据目录与 Xray 可执行文件记录在这里，位于数据目录之外，因此更改数据目录不会丢失它们";
    help_runtime_dir => "temporary launch files; Xray configs are removed on shutdown"
        => "临时启动文件；Xray 配置在退出时删除";

    // ---- the profiles page ----
    edit_profile => "Edit profile" => "编辑档案";
    profile_saved => "Saved." => "已保存。";
    profile_duplicated => "Copied the profile." => "已复制档案。";
    delete_profile_title => "Delete profile" => "删除档案";
    delete => "Delete" => "删除";
    keep => "Keep" => "保留";
    profile_deleted => "Deleted the profile." => "已删除档案。";
    window_gone => "the window is gone" => "窗口已关闭";
    core_gone => "that browser core is no longer there" => "该浏览器内核已不存在";
    proxy_gone => "that proxy is no longer there" => "该代理已不存在";
    executable_cannot_be_empty => "the executable path cannot be empty" => "可执行文件路径不能为空";

    // ---- the proxies page ----
    proxies_intro => "Assign a proxy to a profile to route its traffic through it."
        => "把代理分配给档案，即可让它的流量走这条代理。";
    import_from_link => "Import from link" => "从链接导入";
    new_proxy => "New Proxy" => "新建代理";
    proxies_empty => "No proxies yet. A profile with no proxy goes direct from this machine."
        => "还没有代理。没有代理的档案将从本机直连。";
    test => "Test" => "测试";
    edit => "Edit" => "编辑";
    engine_running_profile => "through the engine a running profile is using"
        => "经由运行中档案正在使用的引擎";
    engine_temporary => "through a temporary engine" => "经由临时引擎";
    delete_proxy_title => "Delete proxy" => "删除代理";

    // ---- the cores page ----
    cores_intro => "Each core is a fingerprint-chromium binary; its detected version decides which switches a profile may claim."
        => "每个内核都是一个 fingerprint-chromium 可执行文件；检测到的版本决定档案可以声明哪些开关。";
    add_core => "Add Core" => "添加内核";
    cores_empty => "No browser core yet. Add a fingerprint-chromium binary to launch profiles with it."
        => "还没有浏览器内核。添加一个 fingerprint-chromium 可执行文件，即可用它启动档案。";
    core_executable_missing => "executable missing" => "可执行文件缺失";
    core_no_version => "no detected version: no switches can be claimed"
        => "未检测到版本：无法声明任何开关";
    redetect => "Re-detect" => "重新检测";
    delete_core_title => "Delete browser core" => "删除浏览器内核";

    // ---- the backup cards on the Settings page ----
    export_title => "Export configuration" => "导出配置";
    export_body => "Writes cores, proxies and profiles to a JSON file, for another machine or for keeping. Browser data is not included: it is far larger, and a copy belongs beside the profile it came from."
        => "把内核、代理和档案写入一个 JSON 文件，用于迁移到另一台机器或留档。不含浏览器数据：它大得多，而且副本应当留在它所属的档案旁边。";
    export_path_label => "Export file path" => "导出文件路径";
    export => "Export" => "导出";
    export_credentials => "Include proxy credentials" => "包含代理凭据";
    export_credentials_note => "Left off, the passwords are dropped and the rest of each proxy is kept."
        => "不勾选时，密码会被丢弃，代理的其余部分保留。";
    export_credentials_warning => "A file that includes credentials holds them in plain text. Do not send it to anyone casually."
        => "包含凭据的文件以明文保存它们，请不要随便发给别人。";
    leave_empty_new_file => "Leave empty for a new file under the data directory"
        => "留空则在数据目录下新建文件";

    import_title => "Import configuration" => "导入配置";
    import_body => "Reads a backup written by the export above. Nothing already here is overwritten: identifiers that exist are kept as they are, a profile whose core is not here is skipped, and the report says which was which."
        => "读取上面导出的备份。已存在的内容不会被覆盖：标识符已占用的记录原样保留，内核不在本机的档案会被跳过，报告会说明各自是哪种。";
    import_path_label => "Import file path" => "导入文件路径";
    import => "Import" => "导入";
    import_note => "Nothing is confirmed first: import only adds, so undoing one is deleting the rows it named."
        => "导入不会先确认：它只做新增，因此撤销就是删掉它列出的那些行。";

    restore_title => "Restore configuration" => "还原配置";
    restore_body => "Makes this installation be the file's configuration, replacing what is here. An empty installation is restored at once; a populated one asks before replacing anything."
        => "让本机配置变成该文件的内容，替换现有配置。空安装直接还原；已有配置时会先询问再替换。";
    restore_path_label => "Restore file path" => "还原文件路径";
    restore_needs_path => "Type the path of a configuration backup to restore."
        => "请输入要还原的配置文件路径。";
    restore => "Restore" => "还原";
    replace => "Replace" => "替换";
    restore_note => "This replaces the configuration, not the sessions: a profile keeps its browser data on disk."
        => "替换的是配置，不是会话：档案的浏览器数据仍保留在磁盘上。";

    browser_data_title => "Browser data" => "浏览器数据";
    browser_data_body => "Copies each profile's cookies, storage and sessions to a directory of your own, or back from one. Only stopped profiles are copied: a running browser is still writing."
        => "把每个档案的 Cookie、存储和会话拷到你自己的目录，或从该目录拷回。只拷贝已停止的档案：运行中的浏览器仍在写入。";
    browser_data_path_label => "Browser data directory" => "浏览器数据目录";
    copy_out => "Copy out" => "拷出";
    copy_in => "Copy in" => "拷入";
    browser_data_note => "The directory holds one subdirectory per profile, named after its identifier, so it lines up with the configuration."
        => "该目录下每个档案一个子目录，以标识符命名，因此能与配置一一对应。";

    // ---- the profile editor ----
    name_field => "Name" => "名称";
    profile_name => "Profile name" => "档案名称";
    browser_core => "Browser core" => "浏览器内核";
    fingerprint_seed => "Fingerprint seed" => "指纹种子";
    new_seed => "New seed" => "新的种子";
    seed_help => "Drives every noisy surface the profile claims."
        => "驱动该档案声明的所有易变指纹项。";
    brand_field => "Brand" => "品牌";
    brand_version => "Brand version" => "品牌版本";
    brand_version_help => "Reported to pages only when a brand is set. Blank means the engine's own."
        => "仅在设置了品牌时报告给页面。留空表示沿用引擎自身的值。";
    accept_language => "Accept language" => "接受语言";
    accept_language_help => "What navigator.language reports; the first entry wins."
        => "即 navigator.language 报告的值；以第一项为准。";
    timezone_field => "Timezone" => "时区";
    cpu_cores => "CPU cores" => "CPU 核心数";
    cpu_cores_help => "Blank leaves the engine's own value." => "留空表示沿用引擎自身的值。";
    hardware_concurrency => "Hardware concurrency" => "硬件并发数";
    window_field => "Window" => "窗口";
    window_width => "Window width" => "窗口宽度";
    window_height => "Window height" => "窗口高度";
    proxy_field_help => "Chosen when the profile starts. A change applies to the next start."
        => "在档案启动时选定。修改从下次启动起生效。";
    webrtc_field => "WebRTC" => "WebRTC 策略";
    webrtc_none => "No non-proxied UDP" => "禁用非代理 UDP";
    webrtc_public => "Public interface only" => "仅公开接口";
    webrtc_public_private => "Public and private" => "公开与私有";
    excluded_spoofing => "Excluded spoofing" => "排除的伪装项";
    excluded_spoofing_help => "Features the engine must leave at the host's real value."
        => "引擎必须保持主机真实值的特性。";
    direct => "Direct" => "直连";
    seed_whole_number => "seed must be a whole number" => "种子必须是整数";
    concurrency_whole_number => "hardware concurrency must be a whole number"
        => "硬件并发数必须是整数";
    width_whole_number => "window width must be a whole number" => "窗口宽度必须是整数";
    height_whole_number => "window height must be a whole number" => "窗口高度必须是整数";

    // ---- the proxy editor ----
    new_proxy_title => "New proxy" => "新建代理";
    edit_proxy_title => "Edit proxy" => "编辑代理";
    proxy_name => "Proxy name" => "代理名称";
    protocol_field => "Protocol" => "协议";
    host_field => "Host" => "主机";
    host_help => "A host name or address, without a scheme."
        => "主机名或地址，不要带协议前缀。";
    proxy_host => "Proxy host" => "代理主机";
    port_field => "Port" => "端口";
    proxy_port => "Proxy port" => "代理端口";
    username_field => "Username" => "用户名";
    username_help => "Both fields, or neither." => "两个字段要么都填，要么都不填。";
    proxy_username => "Proxy username" => "代理用户名";
    password_field => "Password" => "密码";
    proxy_password => "Proxy password" => "代理密码";
    port_whole_number => "port must be a whole number between 1 and 65535"
        => "端口必须是 1 到 65535 之间的整数";
    link_only_note => "a link carries the fields they need." => "链接里已经带有它们需要的字段。";
    link_only_hint => "Use \"Import from link\" on the Proxies page."
        => "请使用「代理」页上的「从链接导入」。";

    // ---- the core editor ----
    edit_core_title => "Edit browser core" => "编辑浏览器内核";
    add_core_title => "Add browser core" => "添加浏览器内核";
    add_core_use_add => "a new core is added by picking a binary; use `add` instead"
        => "新增内核通过选择可执行文件完成；请改用 `add`。";
    executable_field => "Executable" => "可执行文件";
    executable_help => "The fingerprint-chromium binary to launch."
        => "要启动的 fingerprint-chromium 可执行文件。";
    core_executable => "Core executable" => "内核可执行文件";
    core_name => "Core name" => "内核名称";
    core_name_help => "Blank uses the binary's name and detected major."
        => "留空则使用可执行文件名和检测到的主版本号。";
    version_field => "Version" => "版本";
    version_help => "Read from the binary. Saving a different executable re-reads it."
        => "从可执行文件读取。换成别的可执行文件保存时会重新读取。";

    // ---- the paste dialog ----
    import_link_title => "Import from a link" => "从链接导入";
    share_link => "Share link" => "分享链接";

    // ---- the activity log and the notices ----
    log_browser_stopped => "browser stopped" => "浏览器已停止";
    no_activity_log => "no activity log is being kept" => "没有保存活动日志";
    no_core_registered => "no browser core is registered yet; add one on the Browser Cores page before creating a profile"
        => "还没有注册浏览器内核；请先到「浏览器内核」页添加一个，然后再新建档案";
    a_removed_profile => "a removed profile" => "已删除的档案";
    proxy_not_assigned => "not assigned" => "未分配";
    proxy_not_used => "not used" => "未使用";
    core_missing_marker => "(missing core)" => "（内核缺失）";
    fingerprint_confirmed_short => "fingerprint confirmed" => "指纹已确认";
    fingerprint_confirmed_toast => "Fingerprint confirmed." => "指纹已确认。";
    verification_running => "verifying..." => "校验中……";
    fingerprint_unreadable_short => "fingerprint unreadable" => "指纹无法读取";

    // ---- the log page ----
    log_intro => "What this window has done and seen: starts, stops, warnings, errors and reads, newest first."
        => "这个窗口做过和看到的事：启动、停止、警告、错误与读数，最新的在最前。";
    copy => "Copy" => "复制";
    clear => "Clear" => "清空";
    log_empty => "Nothing has happened yet in this window." => "这个窗口里还没有发生任何事。";

    // ---- the Runtime Details panel ----
    details_empty => "Select a profile to inspect its runtime." => "选择一个档案以查看它的运行状态。";
    runtime_details => "Runtime Details" => "运行详情";
    copy_args => "Copy args" => "复制参数";
    open_data_dir => "Open data dir" => "打开数据目录";
    verify_fingerprint => "Verify fingerprint" => "校验指纹";
    duplicate => "Duplicate" => "复制档案";
    start => "Start" => "启动";
    stop => "Stop" => "停止";
    restart => "Restart" => "重启";
    field_profile_id => "Profile ID" => "档案 ID";
    field_state => "State" => "状态";
    field_seed => "Seed" => "种子";
    field_core => "Core" => "内核";
    field_proxy => "Proxy" => "代理";
    field_data_dir => "Data dir" => "数据目录";
    field_browser_pid => "Browser PID" => "浏览器 PID";
    field_xray_pid => "Xray PID" => "Xray PID";
    field_cdp_port => "CDP port" => "CDP 端口";
    field_socks_port => "SOCKS port" => "SOCKS 端口";
    field_started => "Started" => "启动时间";
    field_dropped_events => "Dropped events" => "丢弃的事件";
    no_launch_recorded => "No launch recorded yet." => "还没有启动记录。";
    reading_fingerprint => "Reading the fingerprint out of the running browser..."
        => "正在从运行中的浏览器读取指纹……";
    fingerprint_confirmed => "Confirmed: every claim this profile makes was read back from the browser."
        => "已确认：该档案声明的每一项都已从浏览器读回。";
}

/// The English table, for tests.
///
/// A test asserts words a user reads, so it has to name the language it is
/// reading them in. Naming one keeps the assertion a compile error when a field
/// is renamed, where a lookup by string would silently assert nothing.
#[cfg(test)]
pub fn en() -> &'static Text {
    &Text::EN
}

/// The catalog for a language.
///
/// A `&'static Text` rather than a value: the two tables are constants, and the
/// window reads through them on every render.
pub fn text(lang: Lang) -> &'static Text {
    match lang {
        Lang::En => &Text::EN,
        Lang::Zh => &Text::ZH,
    }
}

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

    /// A core as the list and the picker name it: the binary's name and the
    /// major a profile's switches are chosen against.
    pub fn core_label(&self, name: &str, major: u32) -> String {
        match self.lang {
            Lang::En => format!("{name} · major {major}"),
            Lang::Zh => format!("{name} · 主版本 {major}"),
        }
    }

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

    pub fn delete_proxy_confirm(&self, name: &str, endpoint: &str) -> String {
        match self.lang {
            Lang::En => format!("\"{name}\" ({endpoint}) will be removed."),
            Lang::Zh => format!("将删除「{name}」（{endpoint}）。"),
        }
    }

    pub fn proxy_in_use(&self, name: &str, used_by: &str) -> String {
        match self.lang {
            Lang::En => format!(
                "\"{name}\" is assigned to {used_by}. Assign those profiles to another proxy or to Direct first."
            ),
            Lang::Zh => {
                format!("「{name}」已分配给 {used_by}。请先把这些档案改派给别的代理，或改为直连。")
            }
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

    /// A profile's row: its state, and the path its traffic takes.
    pub fn profile_meta_proxy(&self, state: &str, proxy: &str) -> String {
        match self.lang {
            Lang::En => format!("{state} · proxy: {proxy}"),
            Lang::Zh => format!("{state} · 代理：{proxy}"),
        }
    }

    pub fn profile_meta_direct(&self, state: &str) -> String {
        match self.lang {
            Lang::En => format!("{state} · direct"),
            Lang::Zh => format!("{state} · 直连"),
        }
    }

    /// The seed and the surfaces derived from it, on one line.
    pub fn profile_seed_line<B: std::fmt::Display, P: std::fmt::Display>(
        &self,
        seed: u32,
        brand: B,
        platform: P,
    ) -> String {
        match self.lang {
            Lang::En => format!("seed {seed} · {brand} · {platform}"),
            Lang::Zh => format!("种子 {seed} · {brand} · {platform}"),
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

    /// A proxy reading: what was measured, and through which engine.
    pub fn proxy_test_reading(&self, reading: &str, scope: &str) -> String {
        match self.lang {
            Lang::En => format!("{reading} {scope}"),
            Lang::Zh => format!("{reading}，{scope}"),
        }
    }

    /// A failed test: the class is named, and the engine's own words are in the
    /// log rather than in a row that has one line.
    pub fn proxy_test_failed(&self, reading: &str) -> String {
        match self.lang {
            Lang::En => format!("{reading}; see the log"),
            Lang::Zh => format!("{reading}；详见日志"),
        }
    }

    /// The export path an empty field would write to.
    pub fn export_empty_writes(&self, path: &str) -> String {
        match self.lang {
            Lang::En => format!("Empty writes {path}"),
            Lang::Zh => format!("留空则写入 {path}"),
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

    /// A proxy test that is still running.
    pub fn proxy_testing(&self) -> String {
        match self.lang {
            Lang::En => "testing...".to_string(),
            Lang::Zh => "测试中……".to_string(),
        }
    }

    /// The address a passing test was measured at.
    pub fn proxy_exit(&self, exit_ip: &str) -> String {
        match self.lang {
            Lang::En => format!("exit {exit_ip}"),
            Lang::Zh => format!("出口 {exit_ip}"),
        }
    }

    /// A failing test, named by class. The engine's own words are the log's.
    pub fn proxy_no_traffic(&self, class: &str) -> String {
        match self.lang {
            Lang::En => format!("no traffic ({class})"),
            Lang::Zh => format!("无流量（{class}）"),
        }
    }

    /// How a passing test was taken, for the row's tooltip.
    pub fn proxy_left_from(&self, exit_ip: &str, millis: u128, scope: &str) -> String {
        match self.lang {
            Lang::En => format!("left from {exit_ip} in {millis} ms, {scope}"),
            Lang::Zh => format!("{millis} 毫秒内从 {exit_ip} 出去，{scope}"),
        }
    }

    /// A failing test's full sentence. `fault` is the runtime's own account.
    pub fn proxy_no_traffic_detail(&self, fault: &str) -> String {
        match self.lang {
            Lang::En => format!("no traffic reached the endpoint: {fault}"),
            Lang::Zh => format!("没有流量到达端点：{fault}"),
        }
    }

    /// The chip for a proxy that is not there any more. It is shown rather than
    /// dropped, so a profile does not silently read as having none.
    pub fn missing_proxy(&self, id: &str) -> String {
        match self.lang {
            Lang::En => format!("(missing proxy {id})"),
            Lang::Zh => format!("（代理 {id} 已缺失）"),
        }
    }

    pub fn missing_core(&self, id: &str) -> String {
        match self.lang {
            Lang::En => format!("(missing core {id})"),
            Lang::Zh => format!("（内核 {id} 已缺失）"),
        }
    }

    /// The engine generation, and the fact that it ignores the exclusions below.
    pub fn generation_ignores_exclusions(&self, generation: &str) -> String {
        match self.lang {
            Lang::En => format!("{generation}; the exclusions below are ignored by this engine"),
            Lang::Zh => format!("{generation}；下面的排除项会被该引擎忽略"),
        }
    }

    /// The protocols that have no form, only a link.
    pub fn link_only_protocols(&self, kinds: &str) -> String {
        match self.lang {
            Lang::En => format!("{kinds} are not filled in here:"),
            Lang::Zh => format!("{kinds} 不在这里填写："),
        }
    }

    /// A core as the picker names it, where the major may be unknown.
    pub fn core_picker_label(&self, name: &str, detail: &str) -> String {
        match self.lang {
            Lang::En => format!("{name} · {detail}"),
            Lang::Zh => format!("{name} · {detail}"),
        }
    }

    pub fn proxy_created(&self, name: &str) -> String {
        match self.lang {
            Lang::En => format!("Created proxy {name}"),
            Lang::Zh => format!("已创建代理 {name}"),
        }
    }

    pub fn proxy_saved(&self, name: &str) -> String {
        match self.lang {
            Lang::En => {
                format!("Saved {name}. Running profiles keep the proxy they started with.")
            }
            Lang::Zh => format!("已保存{name}。运行中的档案仍使用启动时的代理。"),
        }
    }

    pub fn proxy_deleted(&self, name: &str) -> String {
        match self.lang {
            Lang::En => format!("Deleted proxy {name}"),
            Lang::Zh => format!("已删除代理 {name}"),
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

    pub fn config_write_failed(&self, error: &str) -> String {
        match self.lang {
            Lang::En => format!("The configuration could not be written. {error}"),
            Lang::Zh => format!("无法写入配置。{error}"),
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

    pub fn verification_busy(&self, id: &str) -> String {
        match self.lang {
            Lang::En => format!("profile {id} is already being verified"),
            Lang::Zh => format!("档案 {id} 正在校验中"),
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

    pub fn proxy_test_busy(&self, id: &str) -> String {
        match self.lang {
            Lang::En => format!("proxy {id} is already being tested"),
            Lang::Zh => format!("代理 {id} 正在测试中"),
        }
    }

    /// The identifier of a proxy the caller asked for and did not get.
    pub fn proxy_not_found(&self, id: &str) -> String {
        match self.lang {
            Lang::En => format!("proxy {id}"),
            Lang::Zh => format!("代理 {id}"),
        }
    }

    pub fn proxy_traffic_from(&self, name: &str, exit_ip: &str) -> String {
        match self.lang {
            Lang::En => format!("{name}: traffic leaves from {exit_ip}"),
            Lang::Zh => format!("{name}：流量从 {exit_ip} 出去"),
        }
    }

    pub fn proxy_no_traffic_reached(&self, name: &str, fault: &str) -> String {
        match self.lang {
            Lang::En => format!("{name}: no traffic reached the endpoint. {fault}"),
            Lang::Zh => format!("{name}：没有流量到达端点。{fault}"),
        }
    }

    pub fn proxy_test_log(&self, name: &str, detail: &str) -> String {
        match self.lang {
            Lang::En => format!("proxy test: {name} - {detail}"),
            Lang::Zh => format!("代理测试：{name} — {detail}"),
        }
    }

    pub fn egress_confirmed(&self) -> String {
        match self.lang {
            Lang::En => "fingerprint confirmed by reading the running browser".to_string(),
            Lang::Zh => "通过读取运行中的浏览器确认了指纹".to_string(),
        }
    }

    pub fn egress_left_from(&self, exit_ip: &str) -> String {
        match self.lang {
            Lang::En => format!("; traffic left from {exit_ip}"),
            Lang::Zh => format!("；流量从 {exit_ip} 出去"),
        }
    }

    pub fn egress_unreadable(&self, reason: &str) -> String {
        match self.lang {
            Lang::En => format!("; the exit address was not read: {reason}"),
            Lang::Zh => format!("；未能读回出口地址：{reason}"),
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

    /// Joins clauses into one sentence. Chinese does not put a space after its
    /// full stop, and the clauses already carry their own punctuation.
    pub fn join_sentences(&self, parts: &[String]) -> String {
        match self.lang {
            Lang::En => parts.join(" "),
            Lang::Zh => parts.concat(),
        }
    }

    /// A list, capped: a toast is not the place for twenty names.
    pub fn listed_more(&self, shown: &str, more: usize) -> String {
        match self.lang {
            Lang::En => format!("{shown}, and {more} more"),
            Lang::Zh => format!("{shown}，另有 {more} 项"),
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

    /// A proxy test's fault class, in the window's language. The class is a
    /// closed set, which is why it can be translated where the engine's own
    /// words cannot.
    pub fn fault_class(&self, class: FaultClass) -> String {
        match (self.lang, class) {
            (Lang::En, FaultClass::Engine) => "engine".to_string(),
            (Lang::En, FaultClass::Config) => "configuration".to_string(),
            (Lang::En, FaultClass::Auth) => "authentication".to_string(),
            (Lang::En, FaultClass::Dns) => "name resolution".to_string(),
            (Lang::En, FaultClass::Unreachable) => "unreachable".to_string(),
            (Lang::En, FaultClass::Timeout) => "timeout".to_string(),
            (Lang::En, FaultClass::Tls) => "tls".to_string(),
            (Lang::En, FaultClass::Http(code)) => format!("http {code}"),
            (Lang::En, FaultClass::Reading) => "unreadable answer".to_string(),
            (Lang::En, FaultClass::Other) => "unclassified".to_string(),
            (Lang::Zh, FaultClass::Engine) => "引擎".to_string(),
            (Lang::Zh, FaultClass::Config) => "配置".to_string(),
            (Lang::Zh, FaultClass::Auth) => "认证失败".to_string(),
            (Lang::Zh, FaultClass::Dns) => "域名解析".to_string(),
            (Lang::Zh, FaultClass::Unreachable) => "不可达".to_string(),
            (Lang::Zh, FaultClass::Timeout) => "超时".to_string(),
            (Lang::Zh, FaultClass::Tls) => "TLS".to_string(),
            (Lang::Zh, FaultClass::Http(code)) => format!("HTTP {code}"),
            (Lang::Zh, FaultClass::Reading) => "应答无法解析".to_string(),
            (Lang::Zh, FaultClass::Other) => "未分类".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// A message that reads the same in both languages, and why.
    ///
    /// Only abbreviations and names belong here. Every entry is a decision that
    /// the string is not prose: if a sentence ends up on this list because
    /// nobody wanted to translate it, the list has stopped doing its job.
    /// Fields that are the same in both languages on purpose. Protocol names,
    /// header names and product names are spelled the same wherever they are
    /// read; translating one would be inventing a name the browser does not use.
    const SAME_IN_BOTH: &[&str] = &[
        "field_xray_pid",
        "field_cdp_port",
        "field_socks_port",
        "brand_field",
        "share_link",
    ];

    #[test]
    fn every_message_is_translated() {
        let allow: HashSet<&str> = SAME_IN_BOTH.iter().copied().collect();
        for (name, en, zh) in Text::PAIRS {
            if allow.contains(name) {
                continue;
            }
            assert!(!en.trim().is_empty(), "{name} has no English text");
            assert!(!zh.trim().is_empty(), "{name} has no Chinese text");
            assert_ne!(
                en, zh,
                "{name} is identical in both languages, which is what an \
                 untranslated message looks like"
            );
        }
    }

    /// A field that is whitespace is not a message, and a catalog that shrank to
    /// nothing would pass the test above vacuously.
    #[test]
    fn the_catalog_is_substantial() {
        assert!(Text::PAIRS.len() > 50, "{}", Text::PAIRS.len());
        for (name, en, _) in Text::PAIRS {
            assert_eq!(en.trim(), *en, "{name} has stray whitespace");
        }
    }

    #[test]
    fn every_language_round_trips_through_its_code() {
        for lang in Lang::ALL {
            assert_eq!(Lang::from_code(lang.code()), lang);
        }
        assert_eq!(Lang::from_code("  ZH  "), Lang::Zh);
        assert_eq!(Lang::from_code("zh-Hans"), Lang::Zh);
        assert_eq!(Lang::from_code("zh_CN"), Lang::Zh);
    }

    #[test]
    fn an_unknown_language_falls_back_to_english() {
        assert_eq!(Lang::from_code(""), Lang::En);
        assert_eq!(Lang::from_code("de"), Lang::En);
        assert_eq!(Lang::from_code("solarized"), Lang::En);
    }

    /// The two languages must actually read differently, or the switch would do
    /// nothing - the same property the two palettes are held to.
    #[test]
    fn the_two_tables_differ() {
        let en = text(Lang::En);
        let zh = text(Lang::Zh);
        assert_eq!(en.lang, Lang::En);
        assert_eq!(zh.lang, Lang::Zh);
        assert_ne!(en.nav_settings, zh.nav_settings);
        assert_ne!(en.save, zh.save);
        assert_ne!(en.language_title, zh.language_title);
    }

    /// The sentences are methods rather than fields, so they need their own
    /// check that each language is a real rendering of the same information.
    #[test]
    fn the_sentences_read_in_both_languages() {
        for lang in Lang::ALL {
            let t = text(lang);
            assert!(t.profiles_showing(2, 5).contains('2'));
            assert!(t.profiles_showing(2, 5).contains('5'));
            assert!(
                t.setting_saved("Data directory", Effect::NextStart)
                    .contains("Data directory")
            );
            assert!(
                t.setting_dialog_title("Data directory", Effect::Now)
                    .contains("Data directory")
            );
            assert!(
                t.source_set_by_env("FP_BROWSER_DATA_DIR")
                    .contains("FP_BROWSER_DATA_DIR")
            );
            assert!(t.empty_no_match("work", 3).contains("work"));
        }
        // And the three effects are three different sentences, in both.
        for lang in Lang::ALL {
            let t = text(lang);
            let now = t.setting_saved("X", Effect::Now);
            let next = t.setting_saved("X", Effect::NextStart);
            let derived = t.setting_saved("X", Effect::Derived);
            assert_ne!(now, next);
            assert_ne!(next, derived);
            assert_ne!(now, derived);
        }
    }
}
