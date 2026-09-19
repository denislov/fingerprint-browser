# Implementation Plan v1

## Phase 0 — Workspace skeleton

目标：项目可编译，依赖方向固定。

交付：

```text
crates/domain
crates/storage
crates/runtime
crates/application
crates/app
```

完成条件：

```bash
cargo fmt --check
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

---

## Phase 1 — Domain + SQLite

实现：

- BrowserProfile
- FingerprintProfile
- BrowserCore
- ProxyProfile（先 Socks5）
- RuntimeState
- SQLite migrations
- repositories

UI 暂时只显示内存/数据库中的 Profile 列表。

---

## Phase 2 — Direct Chromium runtime

先不做 Xray。

实现：

```text
Profile -> LaunchPlan
spawn fingerprint-chromium
CDP readiness
stop
crash detection
effective args log
```

这是第一个真正可用里程碑。

验收：

1. 创建两个 Profile；
2. 两个独立 user-data-dir；
3. 不同 seed；
4. 可同时运行；
5. 分别停止；
6. 重启后 Cookie 状态保持；
7. CDP 仅 loopback。

---

## Phase 3 — Xray per-profile runtime

进度（2026-09-19）：已实现 SOCKS5/HTTP 配置、Xray 启动与本地 TCP readiness、启动回滚、停止清理、双进程崩溃监控和快照清理。第二批加入 Unix 独立进程组回收、CDP 请求超时和元数据校验、启动等待期间的已有会话监控。第三批完成启动期间 Stop/Shutdown 中断、Restart 延后重启、命令通道断开清理和排队命令顺序测试。第四批加入端口预留至 spawn 前、非阻塞事件通知和可恢复诊断快照，并覆盖事件满载/断开时的回收行为。真实 Xray 26.2.6 已通过本地带认证 SOCKS5/HTTP CONNECT 上游双向转发测试。真实 Chromium 验收及 UI 接入仍需推进，详见 README。

先支持：

```text
SOCKS5 upstream
HTTP upstream
```

流程：

```text
ProxyProfile
 -> XrayConfigBuilder
 -> local SOCKS
 -> Chromium --proxy-server
```

完成 Xray crash -> Chromium fail-closed。

---

## Phase 4 — Fingerprint capability layer

进度（2026-09-19）：第一批已落地。详情见 [fingerprint-matrix.md](fingerprint-matrix.md)。

- `runtime::version`：`--version` 超时探测 + major 解析（5 秒上限，避免错二进制卡启动）。
- `app::core_detect::maintain`：每次启动重探并刷新内核记录（二进制被换掉不会留下过期 major），
  自定义名字不覆盖，丢失可执行文件会上报。
- `CoreCapabilities::for_major` 真值表：以 `FingerprintGeneration::PIVOT_MAJOR = 144` 分代；
  稳定开关两代都发，canvas/client-rects 噪声开关只发给已认证代际。
- `runtime::compat::check`：把“内核不支持的开关”转成 Warning 事件，进入
  `RuntimeSnapshot::last_warning`，在行内与 Runtime Details 展示；major 0 直接拒绝启动。

前置验收（2026-09-19，仍有效）：真实 ungoogled-chromium 148 + Xray 26.2.6 已通过 Linux
无界面运行时验收，覆盖双 Profile、Cookie 隔离与持久化、本地认证代理链路、崩溃回收和
ShutdownAll；修复正常 Stop 强杀导致 Cookie 丢失的问题。详见 `chromium-acceptance.md`。
这不代表 fingerprint-chromium 的指纹能力已认证；v1 只正式认证一个 major。

仍需实现：

- browser version detection 在启动时复核（目前开工时复核）；
- 跨平台字体策略（目标平台不等于宿主平台时补 `font` 到 `--disable-spoofing`）；
- 音频/字体/WebRTC 泄漏/地理位置的运行时回读（当前探针看不到这些面）。

### 第二批：开关词汇的实测与回读验证

进度（2026-09-19）：已完成。方法与全部实测数据见 [fingerprint-matrix.md](fingerprint-matrix.md)。

- `runtime::cdp::CdpSession`：loopback 限制的 websocket 会话（有界超时、跳过事件、只认自己的 id），
  支持 `Page.navigate` 与 `Runtime.evaluate`。
- `runtime::verify`：在真实文档上执行探针（JSON 回读 canvas 哈希、measureText、client rects、
  `navigator.*`、`Intl`、UA-CH 高熵），把观测值与 profile 逐条对账；**读不到也算差异**。
- 实测发现并修复三处“引擎静默忽略”的本仓缺陷：`mac`→`macos`、`client-rects`→`clientrects`、
  去掉 Opera/Vivaldi 品牌声明（引擎只认 Chrome/Edge）。
- `crates/runtime/tests/fingerprint_real.rs`：5 个真实二进制验收，包括
  “同一 seed 可重现 / 不同 seed 不同 canvas / `--disable-spoofing=canvas` 使 canvas 与 seed 无关 /
  macOS 不被留在宿主平台 / 排除 client rects 只影响 rects”。

### 第三批：音频 / WebRTC / 字体的回读

进度（2026-09-19）：已完成。探针新增三类读数，判定规则按"单次读数能否定论"分开：

- 音频：`OfflineAudioContext` 指纹。**只能对比**——seed 必须改变它，`--disable-spoofing=audio`
  必须回到"另一个 seed 也排除音频"的同一读数；
- WebRTC：ICE 候选 + `iceGatheringState`。**单次读数可定论**——严格策略下必须"无 host/srflx
  候选且 gathering 完成"；验收里先用一个把策略改回 `default` 的 planner 证明探针**看得见泄漏**，
  否则"零候选"没有意义；
- 字体：枚举 + 布局度量（canvas measureText 会被 seed 噪声污染，读不到字体）+ CJK/emoji/tofu
  宽度对比。**单次读数可定论**——CJK 不能渲染成缺字框（这正是跨平台字体规则要防的 bug）。

实测发现：这构建默认 `--disable-non-proxied-udp` 生效（无开关也零候选）；音频噪声确实由 seed 驱动；
`--disable-spoofing=font` 在枚举与布局度量上测不出独立效果，但 CJK 字形在切换平台时始终正常。

仍未验证：地理位置（模型无字段，不发射开关；`--fingerprint-location` 在本环境产不出坐标，
无法与"无定位源"区分），以及 `--fingerprinting-client-rects-noise` 在已有 seed 时测不出额外效果。

### 第五批：Profile 编辑器（Phase 5 剩余页面的第一页）

进度（2026-09-20）：已完成。Proxies / Browser Cores / Settings 仍为 `(soon)`。

- `app::editor`：表单持有 profile 的可编辑副本（name / seed / brand (+version) / platform (+version) /
  language / accept language / timezone / CPU cores / 窗口尺寸 / WebRTC 策略 / 排除伪装清单），
  `build_profile()` 复用 domain 的 `validate_profile`+`validate_fingerprint`+`validate_window`，
  失败返回原因而不是写入；表单不编辑的字段（core / proxy / data dir / start target）原样带过去。
- `AppState`：`update_profile` / `duplicate_profile` / `delete_profile`（保留 user data）；
  UI 侧 Edit/Duplicate/Delete 按钮 + 删除二次确认（`AlertDialog`）。
- 对话框在后台线程之外同步执行，保存失败时**不关闭**并显示原因。

本轮最值得记的 bug：**gpui-component 的 overlay 层必须由应用视图自己渲染**
（`Root::render_dialog_layer` / `render_sheet_layer` / `render_notification_layer`）。
`Root` 只渲染 inner view，所以漏了这三行的症状是"对话框永远弹不出来"，
而且 headless 测试与真机都同样失败——这个 bug 是 headless UI 测试抓到的，
不是真机截图抓到的（真机只会看到点了没反应）。

### 第六批：Proxies 页面

进度（2026-09-20）：已完成。Browser Cores / Settings 仍为 `(soon)`。

- 侧边栏变成真实导航（`Page`），proxies 页与 profiles 页共用横幅与滚动容器。
- `domain::validate_proxy` 以前只查名字，现在把"启动时才会炸"的错误提前：
  主机为空/含空格或 scheme、端口为 0、用户名密码只给一半、各协议自己的必填字段
  （ss 的 password/method、vmess 的 uuid/security、vless 的 uuid/encryption、trojan 的 password）。
- `application::ProxyService`（新）：`create`/`update`/`delete`/`get`/`list`/`usage`。
  两层闸门：先过 domain 校验，再过 `runtime::outbound_is_supported`（配置构建器只支持
  SOCKS5 与 HTTP）。**存不下的代理不如早点拒绝**：四个构建不了的协议在表单里被列出来并说明原因，
  不允许选中，而不是存下来到启动时才失败。
- **在用代理不可删**：`profiles.proxy_id` 有 `ON DELETE SET NULL`，直接删会让那个 profile
  静默变成 direct（流量泄漏）。所以 `delete` 先查引用，拒绝并点名持有它的 profile，
  提示改派后再删。行内也实时显示 `used by …`。
- profile 编辑器补上 **Proxy 选择**（芯片：Direct + 每个代理；指向已消失代理时显示 missing 而
  不是当成 Direct）。这是上一批编辑器里唯一缺的字段。

真机验收链路（全部有证据）：界面建代理 → SQLite 落库 → 在 profile 编辑器里指派 → 启动后
xray 子进程的配置 outbound 正是表单里填的 `10.0.0.1:1080`，浏览器 argv 里出现
`--proxy-server=socks5://127.0.0.1:37815`（xray 的本地入站）→ 详情面板显示 Xray PID / SOCKS port，
行内显示 `proxy: Office`。删除在用代理被拒（红字点名 `Profile 1`），改派 Direct 后删除成功。

真机又抓出两个 headless 漏掉的缺陷（都在这一批修的）：
1. 代理芯片标签显示成 `0-Office`——我把"唯一 id"和"显示名"用同一个字符串拼了，
   而测试点的正是那个串，所以测试全绿。现在 id 与标签分开，映射由 `proxy_chip` 这个纯函数产出并有测试。
2. 协议说明文字在 640px 对话框里被裁切：gpui 的单行文本不会自动换行，长句要在源码里断行。

### 第七批：Browser Cores 页面

进度（2026-09-20）：已完成。Settings 仍为 `(soon)`。

- `application::CoreService`（新）：`add`/`update`/`redetect`/`delete`/`get`/`list`/`usage`。
  **版本一律靠探测**：`add` 对二进制跑 `--version`，存回它自己报的 banner 与 major，
  表单里没有版本输入框。读不出 major（或 major 为 0）就当场拒绝并给出出路
  （`FP_BROWSER_CHROMIUM_MAJOR`），而不是先存下来到启动时才失败。
  这与 bootstrap 的自动发现有意不同：开机时没有可改的东西，所以那里保留记录并只用警告说明。
- **改指向 = 重新探测**：把某个 core 指到另一个二进制时重新读版本；原地换了二进制用
  Re-detect 按钮重读。自动生成的 `<binary> <major>` 名字跟着新 major 走，
  用户起过的名字永不被改写（`renamed_for` 三条规则各有测试）。
- **在用 core 不可删**（`profiles.core_id` 没有级联），拒绝并点名持有它的 profile。
- 能力表在行上可见：`Chrome 144+ · noise switches verified`（绿）/
  `Chrome 143 and older · noise switches not offered`（琥珀）/ 无版本（红）。
  措辞收进 `CoreCapabilities::generation_label/noise_label`，行与对话框共用，不会各写一套。
- 顺手修掉一个**潜在 panic**：`AppState::verification_job` 直接调
  `CoreCapabilities::for_major(core.major)`，而该函数对 0 是 `debug_assert`。
  开机自动发现保留 major=0 的 core 时，debug 构建里点"Verify fingerprint"会 panic。
  现在改为 `BrowserCore::capabilities() -> Option<…>`（major 0 返回 None），
  UI 侧拒绝并说明"没有探测到版本，没有可对照的开关表"。

真机验收：页面显示两个内核——本机 148（绿）与自造的谎报 128 的包装脚本（琥珀，能力表差异可见）；
添加不报版本的脚本被拒且对话框不关（提示分两行完整显示）；建 profile 后该 148 内核行显示
`used by Profile 1`；删除在用内核被拒（红字点名）；把 128 包装脚本改成报 148 后点 Re-detect，
名字与版本与能力标签一起翻新（`chrome 128` → `chrome 148`，琥珀 → 绿）。

真机又抓到一个 headless 漏掉的缺陷：**Cores 页上同时渲染了 profiles 页的内容**
（空提示、列表、详情面板）。原因是页面分支写成了 `page != Page::Proxies`，
把 Cores 也放了进去。现在改成 `page == Page::Profiles`，并在测试里断言
"另一页的容器不存在"，而不是只断言"这一页存在"。

另外修掉两类**测试卫生**问题（与上一批同一性质）：新增的 `TempBinary` / `CoreBinary`
测试助手原先会留下临时目录（一轮 27 个），现在都用 RAII guard，一次全量测试后残留为 0。

同一轮还统一了**错误文案的渲染**：三个编辑器的错误框按 `; ` 分行，每行一句，
因为 gpui 不会自动换行，长句会被裁掉半句。

### 第八批：Settings 页面（Phase 5 最后一页）

进度（2026-09-20）：已完成。侧边栏四页全部不再是 `(soon)`。

- `app::settings`（新）：进程级配置 + **来源**。六行：数据目录、Xray 可执行文件、
  Chromium 二进制（env）、Chromium major 覆盖（env）、配置文件、运行时目录（派生）。
  每行给出「生效值 + 来源 + 生效时机」，来源是
  `environment / config file / default / derived from the data directory`。
- 优先级：**环境变量 > 配置文件 > 默认值**；被环境变量压住的那一行会把
  "the config file holds X, which this overrides" 直接写在行里——
  这正是"我存了设置却没生效"的现场答案。
- **可改的两项存在数据目录之外的配置文件里**（默认 `$XDG_CONFIG_HOME/fp-browser/config.json`，
  `FP_BROWSER_CONFIG` 可覆盖）。这是本批最关键的设计决定：数据目录自己不能存在数据目录的
  SQLite 里，否则改完目录重启会打开一个新库，设置当场"消失"。
- 配置文件读不出来（JSON 坏 / 无权限）时：**报告**、用默认值让程序照常启动，
  并**拒绝写入**直到修好——不覆盖用户文件里还留着的设置。
- 生效时机如实标注：两项都是 `next start`，对话框标题也写明
  "takes effect at the next start"，并说明当前进程仍用旧的（含运行中的 profile 保持启动时的 Xray）。

真机验收（有证据链）：把 Xray 指向一个会写日志再 `exec` 真 xray 的包装脚本 →
配置文件落盘 → 重启（不带 `FP_BROWSER_XRAY_BIN`）→ 行变成 `from the config file` 指向包装脚本 →
启动一个带代理的 profile → **包装脚本日志里出现 `run -config …/xray.json`**，详情面板 Xray PID 与
真 xray 进程 PID 一致（`exec` 替换），证明"存的设置就是真正被拉起的可执行文件"。
再带 `FP_BROWSER_XRAY_BIN` 重启 → 行回到琥珀色 `set by FP_BROWSER_XRAY_BIN` 并显示被覆盖的那条。

真机（其实这次是 headless）抓到的缺陷：设置对话框的输入框与页面行**共用了元素 id**
（`setting-data-dir`），测试报 "ambiguous ElementId"。同一棵树里 id 必须唯一，
输入框改为 `setting-field-<key>`。

另外把 `Runtime directory` 那行的来源从 "from the environment" 纠正为
`derived from the data directory`（新增 `Source::Derived`）——它并不是被谁选定的，而是算出来的。

### 第四批：窗口内的指纹验证动作

进度（2026-09-20）：已完成。

- `runtime::cdp`：新增 `create_page`/`close_page`（`PUT /json/new`、`GET /json/close/<id>`，
  target id 只接受十六进制形状）与 `wait_for_document`——只等 `readyState` 不够，
  新标签页会在导航提交前先报告一次 `complete`；同时探针不再依赖 `document.body` 存在
  （用 `documentElement` 兜底），这两处都是真机 GUI 验收抓出来的。
- `app::verifier`：`FingerprintVerifier` trait + `CdpFingerprintVerifier`；读不到一律算
  `Unreadable`，绝不当作"已验证"。
- `AppState`：验证状态机（Running/Confirmed/Disagreements/Unreadable）、`begin_verification`
  校验（必须 Running 且有 CDP 端口、同一 profile 不并发）、stop/restart 丢弃过期读数。
- `ui`：Runtime Details 内 "Verify fingerprint" 按钮（后台线程 + channel 回传，UI 不阻塞）、
  结论按声明逐条列出（琥珀）、行内徽章；结论列表在可滚动的面板里，不会被截断。

真机 GUI 验收同时验证了"假绿"风险：用一个剥掉所有指纹开关的包装脚本当内核，
验证器报出 7 条差异（platform/brand/user agent/hardware concurrency/language/timezone/...），
而不是显示 Confirmed。

既有实现：

```text
browser version detection  （已做，见上）
CoreCapabilities           （已做，见上）
fingerprint CLI serializer （已有，按能力表门控）
version compatibility warnings （已做，见上）
effective launch args 页面  （已有：Runtime Details + Copy args）
```

---

## Phase 5 — GPUI productization

进度（2026-09-19）：第一批 UI 接线已落地。`crates/app` 不再是静态 mockup：

- `AppState` 只持有面向视图的状态，运行时状态一律通过 `RuntimeService::snapshot` 读取；
  后台 200ms tick 排空 `RuntimeEvent` 并做快照对账（每 5 tick 全量对账），符合 facade 契约。
- Profiles 页支持新建、Start/Stop/Restart、状态徽章、Runtime Details（PID/端口/effective args/
  last error/warning/dropped events）与 Copy args。
- 首次启动用 `--version` 探测并注册一个 browser core；关闭窗口和 Quit 都会走
  `ShutdownAll` 回收子进程。

页面：

```text
Profiles            （已完成）
Profile Editor      （已完成，第五批）
Proxies             （已完成，第六批）
Browser Cores       （已完成，第七批）
Settings            （已完成，第八批）
Runtime Details     （已完成）
```

仍未实现（都是"锦上添花"，不是缺口）：

- 行内 toast（目前用横幅 + 对话框内错误行）；
- open user-data-dir（打开 profile 的数据目录）；
- recent error/log 面板（目前只有 Runtime Details 里的 last error/warning）。

已知限制：非 WM 关闭协议直接销毁窗口（如 `xdotool windowclose`）时，gpui 可能不感知
窗口已消失，进程会继续运行并持有浏览器；支持的退出方式是窗口管理器关闭按钮与窗口内
Quit 按钮。该问题在 gpui 的 X11 后端，不在本仓库接线代码。

---

## 推荐最小 Cargo 依赖

根 workspace：

```toml
[workspace]
resolver = "2"
members = [
  "crates/domain",
  "crates/storage",
  "crates/runtime",
  "crates/application",
  "crates/app",
]
```

建议依赖：

```text
serde
serde_json
uuid
thiserror
rusqlite
tracing
tracing-subscriber
crossbeam-channel
ureq
gpui-kit
```

不要在 Phase 0 就加入大量通用框架。

---

## 第一批测试

### Domain

```text
fingerprint seed serialization
accept-language default generation
window validation
profile duplication gets new seed/id/path
generation splits at the verified pivot (144)
stable switch set is carried by both generations
canvas noise is only claimed for the verified generation
```

### Compatibility

```text
verified generation reports nothing
legacy core reports the unverified switch group with the pivot major
unsupported brand is reported with what asked for it
unsupported switches are reported only when the profile uses them
findings keep a stable order
```

### Verification

```text
a faithful session reports nothing
every claim is checked independently
a missing reading is a discrepancy, not a pass
an unsupported brand is not asserted
noise is visible by comparison only
a leaking candidate is reported
candidates through a proxy are not a leak
gathering that never finished is not a pass
a relaxed policy makes no leak claim
missing cjk glyphs are reported
the font claim is settled by the widths not the enumeration
audio is read and compared between sessions
the probe expression never touches the network
```

### CDP session

```text
evaluation skips events and returns the reply value
a page exception is reported instead of a value
a silent peer cannot hold the evaluation past its deadline
a peer that never completes the handshake fails the dial
a debugger url off loopback is refused (and the probe port is accepted)
a browser without a page target reports no page target
an unresponsive http server cannot exceed the readiness deadline
empty json is not ready
```

### Verification state (app)

```text
a verification needs a running browser with a debug port
a verification job carries the profile and its capabilities
a second verification of the same profile is refused
an outcome records confirmation disagreement or failure
stopping or restarting a profile drops a stale reading
a confirmed fingerprint is reported in the window
disagreements are listed claim by claim
every disagreement is reachable from a short panel
```

### Profile editor

```text
the form rebuilds the profile it was opened on
every edited field reaches the rebuilt profile
a field the engine cannot read is refused with a reason
the domain rules still apply
a new seed is different and still valid
the engine specific defaults survive an edit
editing a profile writes it and keeps the row in step
a refused edit leaves the stored profile alone
a duplicate gets its own identity and is selected
deleting a profile removes it and forgets its reading
a profile can be edited from the window
a refused edit keeps the dialog open and shows why
duplicating adds a profile and deleting removes one
```

### Proxies

```text
a well formed proxy passes
a nameless proxy is refused
a proxy without a host or port is refused
a pasted url is refused as a host
half a credential is refused
the per protocol secrets are required
the endpoint names the protocol and the host
a created proxy can be read back
a broken proxy is refused before it is stored
a protocol the runtime cannot build is refused
updating a proxy that is gone is not found
an edit is stored and re-checked
a proxy still assigned to a profile cannot be deleted
an unassigned proxy is deleted
usage names the profiles per proxy
deleting a missing proxy is not found
a created proxy is listed and stored
a broken proxy is refused and reported
the picker offers every stored proxy
assigning a proxy reaches storage and the usage list
a proxy in use cannot be deleted
a proxy is deleted once nothing uses it
saving a proxy keeps a running profile on what it started with
the page can be switched
the sidebar switches to the proxies page
a proxy can be created from the window
a refused proxy form says why and stays open
assigning a proxy from the profile editor reaches storage
deleting a proxy in use is refused in the window
the form opens on a stored proxy
a new form starts on socks5 and needs a name and host
a port the user typed survives a protocol switch
an untouched port follows the protocol
half a credential is refused by the form
a port that is not a number is refused by the form
the offered protocols match what the runtime can build
a proxy chip is labelled with its name alone
the proxy the profile is on is what the form offers
choosing a proxy and choosing direct change the assignment
an assignment to a missing proxy is kept and shown
```

### Browser cores

```text
the generation is written the same way everywhere
adding a core stores what the binary reported
a chosen name is kept
a binary without a version is refused with the way out
major zero is not a version
a path that is not a file is refused before the probe
the same binary is not registered twice
renaming a core does not probe again
pointing a core at another binary re reads the version
pointing a core at a silent binary is refused and changes nothing
a re pointed core is renamed by its own rule
updating a core that is gone is not found
redetecting re reads a replaced binary
redetecting keeps a chosen name
redetecting a missing binary is refused and keeps the record
a core in use cannot be deleted
an unused core is deleted
usage names the profiles per core
an added core carries the version its binary reported
a legacy core says the noise switches are not offered
a binary without a usable version is refused and reported
verifying through a core without a version is refused not asserted
re detecting a replaced binary updates the major and the label
renaming a core keeps its version
a core a profile launches with cannot be deleted
an unused core is deleted
the sidebar switches to the cores page
a core can be added from the window
a refused core form says why and stays open
deleting a core in use is refused in the window
re detecting from the window reports what it found
the form opens on a stored core
a renamed core keeps its version
a blank name leaves the name alone
re pointing a core takes the new path
a new form has no core to build
an empty path is refused by the form
the edited path is what a new core would use
```

### Settings

```text
with nothing set every value is the default
a stored value is used and says where it came from
an environment override shows the value it shadows
a broken config file is reported and does not lose settings
saving keeps the other setting
a saved value is the pending one even while the process uses the old
an environment override has no pending value
an empty value is refused
the read only settings cannot be saved
the runtime directory follows the data directory
a relative data directory is shown against the working directory
the settings page names every source
the settings rows say where each value came from
saving a setting stores it and says when it applies
a refused setting is reported and changes nothing
the sidebar switches to the settings page
only the editable settings offer a button
a setting can be changed from the window
```

### Fingerprint acceptance (real binary, opt-in)

```text
a verified core honours every profile claim
a fresh reading repeats and leaves the session as it was
two profiles do not share a canvas surface
disabling canvas spoofing makes the canvas seed independent
a macos profile is not left on the host platform
excluding client rects removes only that noise
audio spoofing is seed driven and can be excluded
no ice candidate leaks an address on the product path
font surface is readable and cjk is not boxed
real chromium fingerprint verification through the verifier
```

### Version

```text
parses the major from a chromium banner
rejects banners without digits
missing executable reports nothing
hanging executable is killed at the deadline
```

### Core catalogue (app)

```text
empty catalogue registers the discovered core
major override covers a silent binary
unreadable version is reported without a major
no discovered core is an error, not an empty catalogue
replaced binary updates the stored major and auto-generated name
custom core name survives a version change
unreadable probe keeps the stored core
missing executable is an error naming the core
```

### LaunchPlanner

```text
same input -> same args
proxy none -> no --proxy-server
proxy enabled -> local SOCKS only
unsupported capability -> error/warning
argument ordering stable
```

### Runtime

```text
start nonexistent executable -> Failed
CDP timeout -> cleanup browser + Xray
stop is idempotent
browser unexpected exit -> Crashed -> Stopped
xray unexpected exit -> browser terminated
```

### Storage

```text
CRUD
migration from empty DB
round-trip all domain fields
```

### App (UI wiring)

```text
create profile -> row appears, selected, state Stopped
create without a core -> error notice, no row
start/stop -> row state follows the snapshot, commands reach the facade
unknown profile -> NotFound notice
refresh_runtime -> picks up external state changes
headless window: click New Profile x2 -> two rows; click Start -> badge Running,
  the other profile stays Stopped; click Stop -> Stopped
headless window: no core -> banner shows the discovery hint
real Chromium (opt-in): AppState start/stop/restart with two live sessions,
  distinct PIDs/CDP ports/data dirs, seed switches in effective args
```

---

## 建议的第一个编码 Batch

Batch 1 不碰 UI 复杂组件，只完成基础骨架：

1. 创建 workspace；
2. 定义 domain model；
3. 定义 error types；
4. 定义 repository traits；
5. 定义 runtime command/event；
6. 建立最小 GPUI Kit 窗口；
7. CI-style 本地检查全部通过。

Batch 2 再实现 SQLite。

Batch 3 再实现 Direct Chromium Runtime。

这样的推进顺序能保证每个阶段都可测试，而不是先做一个漂亮 UI 再补底层。
