# Fingerprint Browser v1 Architecture

## 1. 目标

本项目是一个面向个人使用的轻量指纹浏览器管理器。它不修改 Chromium 源码，而是围绕现成的 `fingerprint-chromium` 内核提供：

- 浏览器实例（Profile）管理；
- 独立 `user-data-dir`；
- 稳定的 fingerprint seed 与少量显式画像参数；
- 每实例独立 Xray 代理桥；
- Chromium / Xray 进程生命周期管理；
- 本地 SQLite 持久化；
- 基于 GPUI Kit 的桌面 UI。

v1 的核心定位不是“自动化平台”，而是：

> 将 Profile 配置编译为稳定、可复现的浏览器运行环境，并可靠管理其生命周期。

---

## 2. 非目标

v1 明确不做：

- Playwright / Puppeteer 自动化脚本平台；
- 插件市场与扩展分发；
- 云同步、团队协作、账号权限；
- OpenList / S3 备份；
- Mihomo / sing-box 多代理内核；
- 自研 Chromium patch；
- 大量逐字段指纹伪装 UI；
- 远程暴露 CDP；
- 多用户服务端。

这些功能不进入核心抽象，避免未来为“可能会做”而提前复杂化。

---

## 3. 总体架构

```text
┌─────────────────────────────────────────────┐
│                 GPUI Kit UI                 │
│ Profile List / Editor / Proxy / Core / Log │
└──────────────────────┬──────────────────────┘
                       │ Application Commands
                       v
┌─────────────────────────────────────────────┐
│               Application Layer             │
│ ProfileService / ProxyService / CoreService │
│ RuntimeFacade                               │
└───────────────┬─────────────────┬───────────┘
                │                 │
                v                 v
┌───────────────────────┐   ┌───────────────────────┐
│       Storage         │   │   RuntimeSupervisor   │
│ SQLite repositories   │   │ dedicated worker     │
└───────────────────────┘   └──────────┬────────────┘
                                      │
                           ┌──────────┴───────────┐
                           v                      v
                ┌──────────────────┐    ┌──────────────────┐
                │   XrayRuntime    │    │ BrowserRuntime   │
                │ per-profile proc │    │ Chromium proc    │
                └─────────┬────────┘    └─────────┬────────┘
                          │                       │
                          v                       v
                127.0.0.1 SOCKS          fingerprint-chromium
```

关键约束：

1. UI 不直接 `Command::spawn()`。
2. UI 回调不等待慢操作：代理测试、指纹校验、打开目录、浏览器数据复制，以及
   导出/导入/还原配置与重新探测 core 版本，都把工作交给 worker，答案经各自的通道
   回到 tick 循环。四个配置类任务共用一个结果类型与一条通道（`maintenance`），
   同时只允许一个在跑，第二个点击被拒绝。
3. Storage 不认识 GPUI 类型。
4. Runtime 不直接操作 UI Entity / View。
5. Domain model 不依赖 SQLite、GPUI、Xray JSON 或 Chromium CLI。
6. 所有外部进程启动前，先生成不可变 `LaunchPlan`。

---

## 4. Workspace

建议采用 Rust workspace：

```text
fingerprint-browser/
├── Cargo.toml
├── crates/
│   ├── domain/
│   ├── storage/
│   ├── runtime/
│   ├── application/
│   └── app/
├── runtime/
│   ├── chromium/
│   └── xray/
├── data/
│   ├── app.db
│   └── profiles/
└── docs/
```

### `domain`

纯领域对象与规则：

- `BrowserProfile`
- `FingerprintProfile`
- `ProxyProfile`
- `BrowserCore`
- `RuntimeState`
- `LaunchPlan`
- validation
- 凭据剥离规则（哪些字段算凭据）：`ProxyOutbound::without_credentials` / `carries_credentials`，穷尽 `match`，新增协议无法回避该决定

禁止依赖：

- GPUI
- rusqlite
- Xray JSON schema
- `std::process::Child`

### `storage`

本地持久化：

- SQLite schema / migrations；
- repository 实现；
- settings；
- runtime history（可选）。

### `runtime`

所有外部运行时行为：

- `LaunchPlanner`
- `RuntimeSupervisor`
- `BrowserProcess`
- `XrayProcess`
- CDP readiness probe
- port allocation
- process tree cleanup
- crash detection

### `application`

用例编排：

- create/update/delete profile；
- start/stop/restart profile；
- duplicate profile；
- proxy/core validation；
- 配置备份交换文档的构建与读取（`config_backup`）：只负责文档本身；
- 导出（`export`）：读三个列表、按凭据选择构建文档并写盘，产出逐项报告；
- 导入（`import`）：纯规划器（`plan_import`，决定新增/保留/跳过/路径改写）+ 按依赖顺序落库的执行器（`apply_import`，阶段间读回实际集合）；不覆盖任何已存在记录；
- 还原（`restore`）：同一文档的另一种读法，但走**严格**路径而不是复用 `plan_import`——文件里的标识符总是胜过已存记录；非空安装的前置条件（`OnlyWhenEmpty`）在规划函数里而不在调用点，误操作到不了写入器；规划阶段先做全量领域校验，再查重复 ID 与悬空引用（profile 指向文件里没有的 core/proxy），凭据被省略的备份在这一步就被明确拒绝并说明原因；写入是 storage 的**配置级事务**（`ConfigurationRepository::replace`），按 profile → proxy → core 逆序删除、再按相反顺序写入，任一步失败整体回滚，旧配置分毫不动；
- 浏览器数据拷贝（`browser_data`）：把 `profiles/<id>` 目录整体拷出到用户指定目录、或从该目录拷回；运行中的档案按名拒绝且发生在写第一个字节之前，从未启动过的档案记为跳过而非失败，目标目录即源目录时按名拒绝（先清空目标会删掉源）；
- 将 runtime event 转为 UI 可消费状态。

### `app`

唯一依赖 GPUI Kit 的 crate：

- application bootstrap；
- window / routing；
- views；
- dialogs；
- state binding；
- 单实例（`instance`）：数据目录上的内核级互斥锁——Unix 用 `flock`、Windows 用
  `LockFileEx`。第二个副本拿不到锁就报出持有者并以退出码 3 结束，因此不会打开同一个数据
  库，也不会把第一个副本正在运行的浏览器当成遗留进程回收。锁由内核在进程结束时释放，所以
  没有需要判定陈旧的锁文件（见 implementation-plan 的 One instance）；
- 外观（`theme`）：两套语义调色板 + `ThemeChoice`，以及从组件主题读回当前配色的
  `palette(cx)`。设置持久化在 `settings`，启动时在画第一帧之前应用（见 10d）；
- 退出模式（`exit`）：关窗的三种答法与"每次都问"，以及关闭请求的拦截点。模式一的窗口
  最小化与托盘图标在 `tray`：Linux 走 `ksni`（D-Bus 上的 StatusNotifierItem，纯 Rust，
  不引入 GTK/libdbus），Windows 走 `tray-icon`（复用窗口已有的消息泵）；两者都只把点击
  投进队列，由窗口的 tick 处理（见 exit-modes）。

---

## 5. 推荐依赖方向

```text
app
 ↓
application
 ↓        ↘
domain    runtime
  ↑       ↓
  └──── storage
```

实际规则应更严格：

```text
domain        -> no internal dependency
storage       -> domain
runtime       -> domain
application   -> domain + storage + runtime
app           -> application + domain + gpui-kit
```

禁止反向依赖。

---

## 6. Profile 是配置，Runtime Session 是运行态

不要把数据库里的 Profile 和正在运行的进程混为一体。

```text
BrowserProfile (persistent)
├── id
├── name
├── core_id
├── user_data_dir
├── fingerprint
├── proxy_id
└── window

RuntimeSession (ephemeral)
├── profile_id
├── browser_pid
├── xray_pid
├── cdp_port
├── socks_port
├── started_at
├── state
└── last_error
```

这样可避免把 PID、动态端口写进长期 Profile 配置。

---

## 7. Profile 的逻辑身份

一个实例的长期身份由以下组合决定：

```text
Profile Identity
  = user-data-dir
  + fingerprint seed
  + browser persona
  + proxy binding
  + Chromium core compatibility
```

其中：

- `user-data-dir` 保存 Cookie / LocalStorage / IndexedDB 等浏览器状态；
- `seed` 驱动多数随机化指纹；
- persona 只保留真正需要显式控制的字段；
- proxy 决定网络出口；
- core version 决定指纹参数能力集。

---

## 8. Fingerprint v1 策略

v1 采用“稳定 Seed + 少量显式画像”的模式。

推荐显式字段：

```text
seed
brand
brand_version?        (optional)
platform
platform_version?     (optional)
language
accept_language
timezone
hardware_concurrency?
webrtc_policy
window_size
```

不在 v1 中作为可编辑字段：

```text
device_memory
color_depth
touch_points
audio_noise
font_list
webgl_vendor
webgl_renderer
```

理由：这些能力随 fingerprint-chromium / Chromium major 变化较大，应由 seed 和 capability adapter 统一处理，而不是散落到 UI 与数据库模型中。

---

## 9. Capability Adapter

不要让 Chromium 版本判断散落在启动代码中。

定义：

```rust
pub struct CoreCapabilities {
    pub major: u32,
    pub supports_disable_spoofing: bool,
    pub supports_explicit_gpu: bool,
    pub supports_canvas_noise_flag: bool,
    pub supported_brands: Vec<BrowserBrand>,
}
```

由：

```text
BrowserCore.version
        ↓
CapabilityResolver
        ↓
CoreCapabilities
        ↓
LaunchPlanner
```

决定最终参数。

v1 可以只重点支持一个经过验证的 Chromium major；capability 层仍保留，以免未来升级时重写 runtime。

---

## 10. Xray 模型

个人版采用“一 Profile 一 Xray 进程”。

```text
Profile A
├── Chromium A
└── Xray A -> 127.0.0.1:<socks_port>

Profile B
├── Chromium B
└── Xray B -> 127.0.0.1:<socks_port>
```

浏览器只认识本地 SOCKS：

```text
--proxy-server=socks5://127.0.0.1:<port>
```

Xray 负责将其转发到真实 outbound。

优点：

- 生命周期简单；
- Profile 间故障隔离；
- 不需要动态修改一个共享 Xray 配置；
- 停止实例时可以直接回收对应 Xray。

代价：进程数更多。个人使用可接受。

---

## 10b. 代理诊断

“本地端口在监听”与“流量真的出去了”是两件事。启动路径只验证前者
（`wait_ready` 连一次 `127.0.0.1:<port>`），因此一个凭据被上游拒绝、域名解析
失败、或者干脆什么都不转发的代理可以通过启动检查——那个 Profile 之后就会
泄漏：它宣称的指纹与它被看到的地址互相矛盾。

诊断因此发一条真实请求，并报告对端看到的地址，或者失败停在哪一类：

```text
Engine / Config / Auth / Dns / Unreachable / Timeout / Tls / Http(code) / Reading / Other
```

分类不合并，因为它们在不同地方修：凭据被拒不是网络不通。

```text
Engine::Temporary { executable, directory, ready_timeout }
    → 诊断自己起一个引擎、等就绪、发一条请求、然后停掉它
      （代理必须在没有任何 Profile 使用它之前就可测）

Engine::Running { socks_port }
    → 探测一个已经在跑的引擎，除探测外不碰它
      （谁启动的谁停止；诊断杀掉运行中 Profile 的引擎，会是比它要找的 bug
        更糟的 bug）
```

SOCKS5 握手与 HTTP 请求由本模块自己写，不借用 HTTP 客户端库：`ureq` 的
SOCKS connector 在工作线程里握手并 join，于是“接受连接但什么都不说”的对端
会把调用挂住 60 秒，无论超时怎么配（实测）。协议自己的 reply code 也更精确，
匹配库的错误字符串只是猜别人的措辞。每一次读写都带自己的 deadline。

性质：

- 地址端点默认明文 HTTP（TLS 会把证书问题报成代理问题），且可配置；
  `https://` 端点被拒绝而不是降级。
- 临时引擎的配置（含上游凭据）写在 runtime 目录下，并在**所有**路径上删除，
  包括失败。
- 预检与启动是同一条门：带代理的 Profile 点 Start/Restart 时，`AppState::begin_opening`
  先取租约并把 ProxyTestJob 交回给窗口，命令要等这一次请求出得去才入队；不通就
  拒绝并点名 Profile、代理与失败环节，租约归还以便重试。已经开始的手动测试会被
  复用（等待它的结果，而不是再问一次），检查期间按 Stop 会取消这次启动。没有代理
  的 Profile 不做任何检查，直接入队。
- 诊断证明的是“这条请求从该引擎出去、并从那个地址抵达了对端”。它**不**证明
  浏览器被指向了该引擎——那是指纹读回的邻接问题，两者不能互相替代。

---

## 10c. 运行时出口读回

10b 的预检证明的是**这个代理**能转发一条请求；它证明不了**这个浏览器**被指向
了这个引擎。两者会在真正要紧的地方分开：某个启动开关被忽略、Profile 根本没有
代理、浏览器与引擎之间另有一层代理。症状正是本项目要防的那一个——指纹声称一
台机器，流量却从另一台出去。

所以第二层直接问浏览器自己：`runtime::egress` 在用户看不到的标签页里打开地址
端点，让浏览器用它被启动时的网络路径去取，再把对端报出的地址读回来。不去翻命
令行，也不比对开关列表：答案来自对端。

结局分开，因为修法不同：

```text
Read { exit_ip, url }          到达了，对端报出了这个地址
NotReached { url, state }      页面从未提交到端点——这条路上什么都没出去
Timeout { url, state }         页面已到端点、但没加载完——路慢，不是路不通
Unreadable { url, excerpt }    到了，答案里没有地址
NotAsked(reason)               浏览器根本问不到（没有 CDP 端点，或被拒）
Unusable(reason)               端点本身不可问（不是明文 HTTP）
```

“没到达”与“太慢”的区别**从页面读出来**，而不是靠等超时猜：`DocumentState`
读的是 `readyState|protocol|document?`，所以一个提交到了别处并已加载完成的页面
（例如浏览器的错误页）会被立即判为未到达——再等下去不可能改变它。

`verify_egress(expected, outcome)` 只在**预检成功过**时才下判断：`expected` 是
那个代理被测出的出口地址。没有预检就没有任何可被推翻的断言，此时读数是信息而
不是发现——把读数当发现，等于断言 Profile 从未做过的承诺。

性质：

- 只对**有代理**的 Profile 提问。没有代理的 Profile 不对地址做任何声明，而端点
  会看到它被提问的源地址；没有东西要核对，就不该付出这个代价。
- 读回排在指纹读回**之后**：浏览器完全读不到时，报告的是“读不到”，而不是
  “路不通”。两者的修法在不同地方，而用户按下的是前者。
- 出口地址进活动日志。行上没有它的位置，而这正是“流量走的是不是我想的那条路”
  的那一半答案。
- `Read` 只说明**一个**文档、**一个**标签页、**一次**的读数。它不证明用户自己
  打开的页面走同一条路。

读回本身的编排不依赖浏览器就能测：一个 CDP 桩在**同一个端口**上说两种协议
（元数据走 HTTP、会话走 WebSocket，就和浏览器一样），于是"等就绪 → 自己开一个
标签页 → 轮询文档状态 → 读答案 → 关页"全程进门禁。桩回答不了的只有一件事——
**真**浏览器是否如假设那样表现，所以真机验收仍是可选运行。

---

## 10d. 外观（明暗主题）

窗口自己画皮肤，所以它需要自己的一套颜色；组件库又有自己的主题。两者必须一起变，
否则按钮的描边会消失在它背后的窗口里。于是**组件主题是"当前是哪种模式"的唯一真源**
（`Theme::change`），窗口通过 `theme::palette(cx)` 把它读回来选自己的颜色。没有第二
个开关需要保持同步，两层也不可能互相矛盾。

颜色是 `0xRRGGBB` 的 `u32`，经 `rgb(...)` 进入元素树——和只有一套配色时完全一样，改动
只在取值处：以前是 6 个模块级常量加散落的字面量，现在是 `Palette` 的语义字段
（`bg` / `panel` / `border` / `text` / `text_soft` / `muted` / `dim` / `secondary` /
`success` / `warning` / `danger` …）。`Palette` 是 `Copy`，所以**没有上下文**的辅助
函数按值接收它（`p: Palette`），而不是去够一个全局；**有上下文**的则
`let p = palette(cx);`。这条规则让"这次渲染用的是哪种配色"始终能从签名看出来。

两套值分开写而不是派生，因为对比度是相对背景的：在白底上读得清的绿，在近黑底上几乎
看不见，所以浅色一套的状态色整体压深，而不是复用同一个色相。

`_strong` 变体在浅色一套里**更深**而不是更亮——名字说的是角色（在有色底上读的状态、
或作为实心标记），不是明度；在白底上可读的那一端是深的那一端。这条不靠眼睛定：
`both_palettes_have_readable_contrast` 按 WCAG 2.1 逐对计算对比度（正文 4.5:1、
辅助文字 3:1、`dim` 2:1），并且每个状态色都在**两处**检查——窗口底上、以及它自己
配对的有色底上。只在一处可读，正是徽章会消失的原因；浅色一套最初就是这样被这个
测试挡下来的（有三个状态色按文本用只有 3:1 上下）。

性质：

- **选择立刻生效**，是设置页里唯一不等下次启动的一项；其余每一项描述的都是未来的启动。
- **先写文件、再切主题**：配置文件写不进去就拒绝这次切换。否则窗口会显示成一种下次
  启动不会保留的样子——"看起来生效"而其实没有，正是设置页要避免的事。
- 控件用**并列两个 chip**，而不是一个翻转开关：翻转开关把另一个选项藏在"你不是当前
  这个"的标签背后，而读者唯一能拿来对照的，恰好就是眼前这个窗口。
- 组件主题是**进程级**状态，所以断言它的测试要串行（`theme::testing::exclusive`）；
  否则两个测试会互相看到对方的配色，失败在竞态上而不是错误上。

---

## 10e. 语言（中英双语）

窗口说的话集中在一张表里：`crates/app/src/text.rs` 的 `catalog!` 宏，一行英文、一行中文，
由同一个列表生成 `Text::EN` 和 `Text::ZH`。固定标签是字段，要嵌入数字或名字的句子是
`Text` 上的方法——因为**语序属于译文**，两种语言不共用"数字放哪"这件事；英文还需要
单复数而中文不需要，方法把这件事从调用方手里拿走，也避免把译文片段拼成一句在两种语言
里都不通顺的话。句子的连接符号也不共用（`join_sentences`）：中文句末已有句号，不再加空格。

**语言只从状态里读，没有全局**。这和主题相反，是刻意的：主题是进程级的事实（组件库只有
一个），语言是这一份 `AppState` 的事实。测试并行跑，各自持有一个语言；若做成全局，一个
测试切到中文就会把另一个测试的英文断言弄失败，只能靠串行回避，而这里不需要付这个代价。

传表的方式沿用配色那条规则：**有上下文**的函数 `let t = text(cx)` 或 `self.state.text()`；
**没有上下文**的按参数接收 `t: &Text`。对话框（`editor` / `proxy_editor` / `core_editor` /
`proxy_import`）是独立实体，够不到 `AppState`，所以它们在打开时把表**带在身上**
（`text: &'static Text`）——代价是一次切换不会重画已经打开的对话框，收益是不引入全局。

**元素 id 不随语言变**。`Page::id()` 返回冻结的英文 slug（`Profiles` / `Proxies` /
`Cores` / `Log` / `Settings`），`nav-{id}` 由此拼出；侧栏显示的名字仍来自表。用显示文本
当 id，是本地化最容易踩的坑：切换语言会让每个测试、每段自动化脚本的点击目标消失。
`SettingKey::effect()` 同理返回 `Effect` 枚举而不是那句话——程序行为不能依赖当前语言。

边界：`runtime` / `domain` / `storage` 的**故障描述**保持英文原样，窗口只翻译自己加的
框架（"无法读取指纹：…"里的前半句是自己的，后半句是底层的）。例外是 `FaultClass`——
它是封闭枚举，所以由窗口翻译。详见 `docs/i18n.md`。

性质：

- **默认英文**：配置文件里没有 `lang`，或者写了这个版本不认识的取值（`kl`、空串），都
  以英文启动，而不是拒绝启动；`zh-Hans` / `zh-CN` 视为同一种语言。
- **先写文件、再切语言**，与主题同理：写不进去就拒绝，窗口不会显示成下次启动不保留的样子。
- 语言 chip 用**各自的语言**标注（`English` / `简体中文`）：读不懂当前语言的人，只能靠
  母语名字找到出口，所以这两个词不进表。

---

## 10f. 一个目录装下整个安装

配置文件住在数据目录里，与数据库、日志、runtime 文件、档案数据和浏览器数据放在一起。
一个目录就是一次安装的全部：出问题时看它，换机器时拷它，想重来时删它。此前它在平台
配置目录（Linux 上 `~/.config/fp-browser/config.json`），于是有两处需要记住，也有两处
可能留下过期副本。

代价必须说清楚，因为它是真的：**配置文件不能再决定数据目录在哪里**。住在某个目录里的
文件无法决定去哪里找它自己，所以数据目录由 `FP_BROWSER_DATA_DIR` 或平台默认值决定，
设置页那一行变成只读——它的说明文字改为告诉用户怎么搬，而不是给一个注定不生效的输入框。

- 旧位置的配置文件在**首次启动时被搬进来**：设置写入 `<data dir>/config.json`，写出的
  内容不再包含 `data_dir`（下次启动会从环境或平台解析出同一个目录，再存一份这个答案就是
  两者开始不一致的方式），旧文件随后删除。此后不再往旧位置写任何东西。
- 唯一搬不动的情况是旧文件指定了**另一个**数据目录。迁就它会把"指出它在哪"的唯一记录
  留在下次启动不会看的地方，所以这个文件原地不动，本次运行使用自己解析出的目录，横幅
  告诉用户该设置哪个环境变量才能回去；设置页那一行也会把被忽略的值按"环境覆盖了配置"
  的同一句话显示出来。
- `FP_BROWSER_CONFIG` 仍然可以指定一个明确的文件路径。它是覆盖而不是位置：测试和脚本
  需要指向某个文件而不必为它造一个数据目录，而明确的路径不该被迁移。

---

## 11. RuntimeSupervisor

v1 不引入全局 Tokio runtime。

```text
GPUI main thread
      │
      │ RuntimeCommand
      v
crossbeam/std channel
      │
      v
RuntimeSupervisor thread
      │
      ├── spawn Xray
      ├── spawn Chromium
      ├── probe CDP
      ├── watch child exit
      └── emit RuntimeEvent
              │
              v
        Application/UI
```

命令：

```text
Start(profile_id)
Stop(profile_id)
Restart(profile_id)
ShutdownAll
```

事件：

```text
StateChanged
Started
Stopped
Crashed
Failed
Warning
```

Supervisor 是唯一拥有运行进程句柄的组件。

---

## 12. LaunchPlan

所有启动参数先编译成不可变计划：

```rust
pub struct LaunchPlan {
    pub profile_id: ProfileId,
    pub browser_executable: PathBuf,
    pub browser_args: Vec<OsString>,
    pub xray: Option<XrayLaunchPlan>,
    pub user_data_dir: PathBuf,
    pub cdp_port: u16,
    pub socks_port: Option<u16>,
}
```

`LaunchPlanner` 必须是尽量纯的：

```text
Profile + Core + Proxy + DynamicPorts
                ↓
            LaunchPlan
```

只有在 `LaunchPlan` 验证通过后才能 spawn。

---

## 13. Chromium 参数生成顺序

建议固定顺序，方便日志与调试：

```text
1. user-data-dir
2. remote-debugging
3. common launch flags
4. proxy
5. fingerprint seed
6. identity
7. locale/timezone
8. hardware
9. WebRTC / spoofing switches
10. window
11. start target
```

示例：

```text
--user-data-dir=.../profiles/<uuid>
--remote-debugging-port=<port>
--disable-session-crashed-bubble
--proxy-server=socks5://127.0.0.1:<port>
--fingerprint=<seed>
--fingerprint-brand=Chrome
--fingerprint-platform=windows
--lang=en-US
--accept-lang=en-US,en
--timezone=America/Los_Angeles
--fingerprint-hardware-concurrency=8
--disable-non-proxied-udp
--window-size=1920,1080
--disable-sync
--no-first-run
about:blank
```

---

## 14. 安全边界

### CDP

- 仅监听 loopback；
- 不允许 UI 配置 `0.0.0.0`；
- 不暴露到 LAN；
- v1 只用于 readiness / basic control。

### Xray inbound

必须显式：

```json
"listen": "127.0.0.1"
```

不要依赖 Xray 默认监听行为。

### 数据目录

删除 Profile 时：

1. 先确认不在运行；
2. 数据库删除与目录删除分离；
3. v1 建议默认“软删除配置，保留 user-data-dir”；
4. 用户显式选择后再删除磁盘数据。

---

## 15. v1 UI

只做 4 个主区域：

```text
Profiles
Proxies
Browser Cores
Settings
```

Profile 页面：

```text
General
  Name
  Browser Core

Fingerprint
  Seed
  Brand
  Platform
  Language
  Timezone
  CPU
  WebRTC

Proxy
  Proxy Profile

Window
  Width / Height
```

运行时操作：

```text
Start
Stop
Restart
Open data directory
View effective launch args
View runtime log
```

“View effective launch args”应从 v1 就保留，它对排查版本兼容问题价值很高。

---

## 16. 推荐 v1 依赖

保持精简：

```text
gpui-kit          UI facade
serde             domain/config serialization
serde_json        Xray config / auxiliary JSON
uuid              profile IDs
rusqlite          local SQLite
thiserror         typed errors
tracing           structured logging
tracing-subscriber
crossbeam-channel runtime supervisor channel
ureq              simple blocking CDP readiness probe
```

是否启用 `rusqlite/bundled` 取决于打包策略。

---

## 17. 架构原则

1. Profile 是声明式配置，不保存进程状态。
2. LaunchPlan 是配置到运行时的唯一编译结果。
3. RuntimeSupervisor 是唯一进程所有者。
4. UI 只发送意图，不管理 Child handle。
5. Xray 与 Chromium 生命周期绑定到 RuntimeSession。
6. Fingerprint 参数必须经过 CapabilityResolver。
7. 动态端口属于 session，不属于 profile。
8. 所有对外监听默认 loopback。
9. 优先支持一个已验证 Chromium major，再做兼容矩阵。
10. 不为未来功能提前引入插件化框架。
