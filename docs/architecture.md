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
2. Storage 不认识 GPUI 类型。
3. Runtime 不直接操作 UI Entity / View。
4. Domain model 不依赖 SQLite、GPUI、Xray JSON 或 Chromium CLI。
5. 所有外部进程启动前，先生成不可变 `LaunchPlan`。

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
- 将 runtime event 转为 UI 可消费状态。

### `app`

唯一依赖 GPUI Kit 的 crate：

- application bootstrap；
- window / routing；
- views；
- dialogs；
- state binding。

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
