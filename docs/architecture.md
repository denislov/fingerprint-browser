# FpBrowser v1 Architecture

本页保留 v1 的设计定位与结构背景（架构图、依赖方向、架构原则）；各子系统的当前行为与契约
以 [子系统参考](subsystems/README.md) 及其所属文档为准，本页只保留指向它们的入口。

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
2. UI 回调不等待慢操作：worker 与 tick 模型见 [UI shell](subsystems/ui-shell.md)。
3. Storage 不认识 GPUI 类型。
4. Runtime 不直接操作 UI Entity / View。
5. Domain model 不依赖 SQLite、GPUI、Xray JSON 或 Chromium CLI。
6. 所有外部进程启动前，先生成不可变 `LaunchPlan`（见 [Domain model](domain-model.md) 第 11 节）。

---

## 4. Workspace

Rust workspace（成员以 `Cargo.toml` 的 `[workspace] members` 为准）：

```text
fp-browser/
├── Cargo.toml
├── crates/
│   ├── domain/
│   ├── storage/
│   ├── runtime/
│   ├── application/
│   └── app/
└── docs/
```

数据目录的内部布局（`app.db`、`profiles/`、`cores/`、`runtime/` 等）见 [Settings](subsystems/settings.md)。

### `domain`

纯领域对象与规则：类型、字段语义与校验见 [Domain model](domain-model.md)；源码在 `crates/domain/src/`。禁止依赖 GPUI、rusqlite、Xray JSON schema 与 `std::process::Child`。

### `storage`

本地持久化：SQLite schema / migrations 与 repository 实现，见 [Storage](subsystems/storage.md)；源码在 `crates/storage/src/`。

### `runtime`

所有外部运行时行为：启动计划、supervisor、就绪探测、端口分配、进程回收与崩溃处理（见 [Runtime lifecycle](lifecycle.md)），代理与 Xray 引擎（见 [Proxies and Xray](subsystems/proxies-xray.md)），代理测试与出口读回（见 [Traffic verification](subsystems/traffic-verification.md)）；源码在 `crates/runtime/src/`。

### `application`

用例编排：profile CRUD 与复制、start/stop/restart（见 [Service contracts](service-contracts.md) 第 2、6 节），proxy/core 校验（见 [Proxies and Xray](subsystems/proxies-xray.md)、[Browser cores](subsystems/browser-cores.md)），配置备份与导出/导入/还原/浏览器数据拷贝（见 [Backup and restore](backup-and-restore.md)）；源码在 `crates/application/src/`。

### `app`

唯一依赖 GPUI Kit 的 crate：窗口、对话框与视图状态（见 [UI shell](subsystems/ui-shell.md)），设置（见 [Settings](subsystems/settings.md)），界面文字（见 [Interface text](i18n.md)），单实例、退出模式与托盘（见 [Exit modes](exit-modes.md)），外观（见 10d）；源码在 `crates/app/src/`。

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

`BrowserProfile` 与 `RuntimeSession` 的字段定义见 [domain-model.md](domain-model.md) 第 1、2、10 节（标识符、`BrowserProfile`、`RuntimeSession`；源码 `crates/domain/src/profile.rs`、`crates/domain/src/runtime.rs`）；运行态的生命周期（状态机、启动、停止、崩溃与回收）见 [lifecycle.md](lifecycle.md)。

---

## 7. Profile 的逻辑身份

一个实例的长期身份由 user-data-dir、fingerprint seed、browser persona、proxy binding 与 Chromium core compatibility 共同决定，各成分的规则归各自的参考页：

- user-data-dir 的绝对路径规则见 [backup-and-restore.md](backup-and-restore.md)；
- seed、persona 与内核能力集见 [subsystems/fingerprints.md](subsystems/fingerprints.md)，字段定义见 [domain-model.md](domain-model.md)；
- 代理绑定见 [domain-model.md](domain-model.md) 第 8 节，网络出口见 [subsystems/proxies-xray.md](subsystems/proxies-xray.md) 与 [subsystems/traffic-verification.md](subsystems/traffic-verification.md)。

---

## 8. Fingerprint v1 策略

v1 采用“稳定 Seed + 少量显式画像”的模式。可编辑字段与每个字段最终变成的开关见
[subsystems/fingerprints.md](subsystems/fingerprints.md)；字段定义见 `FingerprintProfile`
（`crates/domain/src/fingerprint.rs`）。

模型也没有为 `device_memory`、`touch_points`、`audio_noise` 建独立字段：这些能力不散落到
UI 与数据库模型，而由 seed 和 capability adapter 统一处理（见
[subsystems/fingerprints.md](subsystems/fingerprints.md) 与 [fingerprint-matrix.md](fingerprint-matrix.md)）。未建模的指纹面见
[fingerprint-matrix.md](fingerprint-matrix.md) 的“Not implemented yet”。

---

## 9. Capability Adapter

不要让 Chromium 版本判断散落在启动代码中：core 的 major 由 `DefaultCapabilityResolver`
（`crates/runtime/src/capability.rs`）解析成 `CoreCapabilities`；分代、开关门控与 major 0
的处置见 [subsystems/fingerprints.md](subsystems/fingerprints.md)，类型见
[domain-model.md](domain-model.md) 第 7 节（源码 `crates/domain/src/core.rs`）。

v1 可以只重点支持一个经过验证的 Chromium major；capability 层仍保留，以免未来升级时
重写 runtime。

---

## 10. Xray 模型

个人版采用“一 Profile 一 Xray 进程”：引擎、loopback 入站、浏览器使用的本地 SOCKS 参数与生命周期归属见 [Proxies and Xray](subsystems/proxies-xray.md)；启动计划中的引擎类型见 `crates/domain/src/plan.rs` 的 `XrayLaunchPlan`。

优点：生命周期简单；Profile 间故障隔离；不需要动态修改一个共享的 Xray 配置；停止实例时可以直接回收对应 Xray。代价：进程数更多，个人使用可接受。

---

## 10b. 代理诊断

“本地端口在监听”与“流量真的出去了”是两件事：`wait_ready` 只证明前者（其边界见
[Proxies and Xray](subsystems/proxies-xray.md)），预检发一条真实请求来回答后者。故障分类
（`FaultClass`）、临时与在跑引擎的起停（`Engine`）、地址端点规则、代理把关的启动，以及
预检证明得了什么、证明不了什么，全部归
[Traffic verification](subsystems/traffic-verification.md)；实现起点见
`crates/runtime/src/diagnostic.rs`。

---

## 10c. 运行时出口读回

预检证明的是**代理**，读回证明的是**浏览器**。结局（`EgressOutcome`）、判定规则与提问
范围归 [Traffic verification](subsystems/traffic-verification.md)；实现起点见
`crates/runtime/src/egress.rs`（`verify_egress`）。

---

## 10d. 外观（明暗主题）

外观（`ThemeChoice`、`Palette`、`theme::apply` 与 `palette(cx)` 的机制）以及调色板的
设计理由与验证归 [UI shell](subsystems/ui-shell.md)；主题键 `theme` 与“写文件在前”
的存储语义归 [Settings](subsystems/settings.md)。两套取值的现行定义见
`crates/app/src/theme.rs` 的 `Palette`（`DARK` / `LIGHT`）。

本节只保留两条别处没有的约定：

- 颜色是 `0xRRGGBB` 的 `u32`，经 `rgb(...)` 进入元素树；
- 配色从签名可辨：无上下文的辅助函数按值接收 `p: Palette`（`Palette` 是 `Copy`），
  而不是去够一个全局；有上下文的则 `let p = palette(cx)`。

---

## 10e. 语言（中英双语）

窗口的中英双语界面文字出自 `crates/app/src/text.rs`（`Lang`、`catalog!`、`Text`）。

表的规则与防漂移测试、句子为什么是方法、传表的方式、语言为什么不是全局、元素 id 为什么冻结、回退与持久化、语言 chip，以及“框架翻译、证据保留”的边界，都在 [Interface text](i18n.md)。

---

## 10f. 一个目录装下整个安装

配置文件住在数据目录里，一个目录就是一次安装的全部。布局规则（包括配置文件为什么不能
再决定数据目录在哪里）、数据目录与 `FP_BROWSER_CONFIG` 的解析，以及旧位置配置文件在
首次启动时被搬进数据目录的迁移（三种结局与各自的横幅），全部由
[Settings](subsystems/settings.md) 拥有；源码见 `crates/app/src/settings.rs`、
`crates/app/src/settings/resolution.rs`、`crates/app/src/paths.rs`。

---

## 11. RuntimeSupervisor

监督器的模型、状态机与并发行为以 [lifecycle.md](lifecycle.md) 为准；`RuntimeFacade`、`RuntimeCommand`、`RuntimeEvent` 的当前形状以 [service-contracts.md](service-contracts.md) 第 6–8 节为准，子进程句柄的所有权规则见同页第 9 节。

---

## 12. LaunchPlan

只有在 `LaunchPlan` 验证通过之后才会 spawn——规划失败（`LaunchPlanError`）即本次启动失败，不产生任何进程。计划的结构与不可变规则见 [domain-model.md](domain-model.md) 第 11 节（源码 `crates/domain/src/plan.rs`）；`LaunchPlanner` 契约（不 spawn 进程、输入相同且动态端口相同则输出稳定）见 [service-contracts.md](service-contracts.md) 第 4 节。

---

## 13. Chromium 参数生成顺序

参数按固定顺序生成，顺序以代码为准：外层各段见 `DefaultLaunchPlanner::build`（`crates/runtime/src/planner.rs`）的编号注释，指纹块内部顺序见 `serialize_fingerprint_args`（`crates/runtime/src/fingerprint_args.rs`；各开关的发射条件见 [Fingerprints](subsystems/fingerprints.md)）。核对一次启动实际传入的参数时，看 `RuntimeEvent::EffectiveLaunchArgs`（[service-contracts.md](service-contracts.md) 第 8 节）与 `RuntimeSnapshot::effective_args`（[Fingerprints](subsystems/fingerprints.md) 的 Launch 一节；类型见 `crates/runtime/src/facade.rs`）。

---

## 14. 安全边界

### CDP

- 只监听 loopback、不暴露到 LAN：监听地址由启动参数固定（`--remote-debugging-address=127.0.0.1`，`DefaultLaunchPlanner::build`，`crates/runtime/src/planner.rs`），不是设置项，UI 无法把它配置成 `0.0.0.0` 或其它地址；
- v1 中 CDP 的用法（readiness、优雅关闭与读回）见 [lifecycle.md](lifecycle.md) 第 4、11 节、[Fingerprints](subsystems/fingerprints.md) 与 [Traffic verification](subsystems/traffic-verification.md)。

### Xray inbound

inbound 的监听规则（显式 `listen: 127.0.0.1`）见 [Proxies and Xray](subsystems/proxies-xray.md) 与 [service-contracts.md](service-contracts.md) 第 5 节。

### 数据目录

删除 Profile 时记录与磁盘数据的处理规则，以 [service-contracts.md](service-contracts.md) 第 2 节与 [backup-and-restore.md](backup-and-restore.md) 为准。

---

## 15. v1 UI

v1 只做五个主区域——Profiles、Proxies、Cores、Log、Settings；页面、对话框、编辑器与视图状态以 [UI shell](subsystems/ui-shell.md) 为准。

Profile 表单（新建/编辑）的字段与分组见 `ProfileEditor`（`crates/app/src/editor.rs`）及其 `Render`（`crates/app/src/editor/render.rs`）；Profiles 页的列、行与各项操作以 `crates/app/src/ui/profiles.rs` 为准（`profile_list`、`list_header`、`row_actions`、`row_menu`）。

生效启动参数与运行日志是 Runtime Details 面板中的两个视图，见 `crates/app/src/ui/details.rs` 的 `args_body`、`panel_log`。

---

## 16. 推荐 v1 依赖

依赖清单及其用途、取舍以根 `Cargo.toml` 的 `[workspace.dependencies]` 与各 crate 的 `Cargo.toml`（`crates/*/Cargo.toml`）为准。保持精简：依赖只在有真实用途时引入。

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
