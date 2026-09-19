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

实现：

- browser version detection；
- CoreCapabilities；
- fingerprint CLI serializer；
- version compatibility warnings；
- effective launch args 页面。

v1 只正式认证一个 fingerprint-chromium major。

---

## Phase 5 — GPUI productization

页面：

```text
Profiles
Profile Editor
Proxies
Browser Cores
Settings
Runtime Details
```

实现：

- toast / dialog；
- runtime status badge；
- start / stop / restart；
- open user-data-dir；
- copy effective args；
- recent error/log。

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
