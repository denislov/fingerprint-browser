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
    // ---- the command line ----
    //
    // Not the window's words, and still this table's: the person reading the
    // help is the same person who chose the language. `--help` reads that choice
    // the cheap way (`settings::language_hint`), so answering it cannot move a
    // file, and falls back to English when there is nothing to read.
    cli_usage => concat!(
        "Fingerprint Browser - browser profiles for fingerprint-chromium\n",
        "\n",
        "Usage:\n",
        "  fingerprint-browser [option]\n",
        "\n",
        "Options:\n",
        "  -h, --help         Print this help and exit\n",
        "  -V, --version      Print the version, the commit and the platform, and exit\n",
        "      --diagnostics  Write a report about this installation and exit\n",
        "      --out <path>   Where --diagnostics writes; the data directory by default\n",
        "\n",
        "Environment:\n",
        "  FP_BROWSER_DATA_DIR, FP_BROWSER_XRAY_BIN, FP_BROWSER_CHROMIUM_BIN,\n",
        "  FP_BROWSER_CHROMIUM_MAJOR, FP_BROWSER_CONFIG, FP_BROWSER_ECHO_URL\n",
        "\n",
        "With no option the window opens. The report holds versions, paths, file modes\n",
        "and the end of the activity log; it holds no proxy credentials and no browser\n",
        "data. See the README for what each environment variable does.",
    ) => concat!(
        "Fingerprint Browser —— fingerprint-chromium 的浏览器档案管理器\n",
        "\n",
        "用法：\n",
        "  fingerprint-browser [选项]\n",
        "\n",
        "选项：\n",
        "  -h, --help         显示本帮助并退出\n",
        "  -V, --version      显示版本、提交与平台并退出\n",
        "      --diagnostics  写出本安装的诊断报告并退出\n",
        "      --out <路径>   --diagnostics 的写入位置；默认为数据目录\n",
        "\n",
        "环境变量：\n",
        "  FP_BROWSER_DATA_DIR、FP_BROWSER_XRAY_BIN、FP_BROWSER_CHROMIUM_BIN、\n",
        "  FP_BROWSER_CHROMIUM_MAJOR、FP_BROWSER_CONFIG、FP_BROWSER_ECHO_URL\n",
        "\n",
        "不带选项时打开窗口。报告包含版本、路径、文件权限与日志的末尾若干行，\n",
        "不包含任何代理凭据与浏览器数据。各环境变量的作用见 README。",
    );
    cli_usage_hint => "Run fingerprint-browser --help for the options."
        => "运行 fingerprint-browser --help 查看选项。";

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

    // ---- closing the window ----
    //
    // Three answers, asked once and remembered, and the question itself. The
    // short labels are the chips on the Settings card and the headings of the
    // dialog's rows, so they have to read as actions there and as names here.
    tray_show => "Open window" => "打开窗口";
    tray_quit => "Quit completely" => "完全退出";
    exit_card_title => "On closing the window" => "关闭窗口时";
    exit_card_body => "What the window's close button does. \"Ask\" shows the question every time; the other three answer it once and are kept for the next start."
        => "窗口关闭按钮的行为。选“每次询问”则每次都问；其余三项等于预先作答，并保留到下次启动。";
    exit_ask => "Ask every time" => "每次询问";
    exit_ask_note => "Show the three choices whenever the window is closed."
        => "每次关闭窗口时都显示这三个选项。";
    exit_background => "Keep running" => "后台继续运行";
    exit_background_note => "The window is closed and the program keeps managing the profiles. A tray icon brings the window back or exits completely."
        => "窗口关闭，程序继续管理各档案；托盘图标可恢复窗口或完全退出。";
    exit_keep_running => "Leave browsers running" => "仅退出程序";
    exit_keep_running_note => "The program ends; the browsers and proxy tunnels it started keep running, and the next start takes them over."
        => "程序结束；它启动的浏览器与代理隧道继续运行，下次启动会接管它们。";
    exit_exit_all => "Stop everything" => "退出全部";
    exit_exit_all_note => "The program ends and stops every browser and proxy tunnel it started."
        => "程序结束，并停止它启动的所有浏览器与代理隧道。";

    exit_in_background => "Still running in the background. The tray icon brings the window back, or leaves for good."
        => "仍在后台运行。托盘图标可以恢复窗口，或彻底退出。";
    exit_dialog_title => "Close Fingerprint Browser?" => "关闭 Fingerprint Browser？";
    exit_dialog_body => "Browsers and proxy tunnels are running. Choose what happens to them."
        => "当前有浏览器或代理隧道在运行。请选择它们的去向。";
    exit_dialog_body_idle => "Choose what closing the window does."
        => "请选择关闭窗口的行为。";
    exit_remember => "Remember this choice" => "记住这个选择";
    exit_remember_note => "The Settings page can change it again."
        => "之后可在设置页更改。";

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
    help_config_file => "lives in the data directory, beside the database, the logs and the profiles, so one directory holds the whole installation"
        => "位于数据目录内，与数据库、日志和档案放在一起，因此整个安装只在一个目录里";
    help_data_dir => "where the database, logs, profiles and this config file live; set FP_BROWSER_DATA_DIR before starting to use another directory"
        => "数据库、日志、档案与本配置文件所在的位置；想换目录请在启动前设置 FP_BROWSER_DATA_DIR";
    help_xray_executable_field => "Used when a profile has a proxy. The running process keeps the executable it started with."
        => "档案使用代理时会用到它。正在运行的进程仍使用它启动时的那一个。";
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

    // ---- the diagnostics report ----
    diag_title => "Fingerprint Browser diagnostics" => "Fingerprint Browser 诊断报告";
    diag_intro => "Versions, paths, file modes and the end of the activity log. No proxy credentials and no browser data: this report never opens the database."
        => "版本、路径、文件权限与日志的末尾。不含代理凭据与浏览器数据：本报告不会打开数据库。";
    diag_build => "Build" => "构建";
    diag_version => "Version" => "版本";
    diag_commit => "Commit" => "提交";
    diag_platform => "Platform" => "平台";
    diag_report_time => "Report time" => "生成时间";
    diag_settings => "Settings in force" => "当前设置";
    diag_files => "Files" => "文件";
    diag_database => "Database" => "数据库";
    diag_activity_log => "Activity log" => "日志文件";
    diag_activity_log_rotated => "Rotated activity log" => "轮转的日志文件";
    diag_instance_lock => "Instance lock" => "实例锁";
    diag_log => "The end of the activity log" => "日志的末尾";
    diag_log_empty => "nothing has been written to this log yet" => "这个日志里还没有任何内容";
    diag_missing => "missing" => "缺失";

    // The card on the Settings page that writes the same report.
    diag_card_title => "Diagnostics" => "诊断";
    diag_card_body => "Writes one file about this installation - the build, the settings in force and where each came from, the state of every file it keeps, and the end of the activity log - for a problem report. It never opens the database and never reads browser data."
        => "把本安装的情况写成一个文件——构建信息、当前设置及其来源、各个文件的状态、以及日志的末尾——用于问题反馈。它不会打开数据库，也不会读取浏览器数据。";
    diag_write => "Write report" => "写出报告";

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
    no_core_found => "No browser core yet. Add a fingerprint-chromium binary on the Browser Cores page, or point FP_BROWSER_CHROMIUM_BIN at one and restart."
        => "还没有浏览器内核。请在「浏览器内核」页添加一个 fingerprint-chromium 可执行文件，或把 FP_BROWSER_CHROMIUM_BIN 指向它并重启。";
    add_browser_core => "Add a browser core" => "添加浏览器内核";

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

mod cores;
mod maintenance;
mod profiles;
mod proxies;
mod settings;
mod system;

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

    /// The refusal a second copy of the program reads. It is built from a pid and
    /// a build string rather than being one fixed line, so it is not in the
    /// catalog and is pinned here instead: both halves have to be present, and
    /// both languages have to say them.
    #[test]
    fn the_second_instance_refusal_names_the_holder_where_it_can() {
        for lang in Lang::ALL {
            let named = text(lang).instance_busy(Some((4242, "Fingerprint Browser 0.1.0 (abc)")));
            assert!(named.contains("4242"), "{named}");
            assert!(named.contains("0.1.0"), "{named}");

            // An unreadable line still refuses; it just cannot say who.
            let anonymous = text(lang).instance_busy(None);
            assert!(!anonymous.contains("4242"), "{anonymous}");
            assert!(!anonymous.trim().is_empty(), "{anonymous}");

            assert_ne!(
                named, anonymous,
                "a refusal that names the holder is not the same sentence as one that cannot"
            );
        }
        assert_ne!(
            text(Lang::En).instance_busy(None),
            text(Lang::Zh).instance_busy(None)
        );
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
