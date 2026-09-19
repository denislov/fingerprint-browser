# Domain Model v1

## 1. Identifier

```rust
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProfileId(pub Uuid);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProxyId(pub Uuid);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CoreId(pub Uuid);
```

可以在持久化边界统一转换为字符串。

---

## 2. BrowserProfile

```rust
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct BrowserProfile {
    pub id: ProfileId,
    pub name: String,
    pub core_id: CoreId,
    pub user_data_dir: PathBuf,
    pub fingerprint: FingerprintProfile,
    pub proxy_id: Option<ProxyId>,
    pub window: WindowProfile,
    pub start_target: StartTarget,
}
```

Profile 不包含：

```text
pid
cdp_port
socks_port
running
last_runtime_error
```

这些属于 RuntimeSession。

---

## 3. FingerprintProfile

```rust
#[derive(Debug, Clone)]
pub struct FingerprintProfile {
    pub seed: u32,
    pub brand: BrowserBrand,
    pub brand_version: Option<String>,
    pub platform: Platform,
    pub platform_version: Option<String>,
    pub language: String,
    pub accept_language: String,
    pub timezone: String,
    pub hardware_concurrency: Option<u8>,
    pub webrtc_policy: WebRtcPolicy,
    pub disabled_spoofing: Vec<SpoofingFeature>,
}
```

### BrowserBrand

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserBrand {
    Chrome,
    Edge,
    Opera,
    Vivaldi,
}
```

### Platform

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Windows,
    MacOs,
    Linux,
}
```

### WebRtcPolicy

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebRtcPolicy {
    DisableNonProxiedUdp,
    DefaultPublicInterfaceOnly,
    DefaultPublicAndPrivateInterfaces,
}
```

### SpoofingFeature

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpoofingFeature {
    Font,
    Audio,
    Canvas,
    ClientRects,
    Gpu,
}
```

语义注意：`disabled_spoofing` 表示传给 `--disable-spoofing=` 的集合，而不是“启用这些伪装”。

---

## 4. WindowProfile

```rust
#[derive(Debug, Clone, Copy)]
pub struct WindowProfile {
    pub width: u32,
    pub height: u32,
}
```

v1 不把 `screen.width`、DPR、color depth 等独立建模。

---

## 5. StartTarget

```rust
#[derive(Debug, Clone)]
pub enum StartTarget {
    Blank,
    Url(String),
}
```

v1 只支持单个启动目标。

---

## 6. BrowserCore

```rust
#[derive(Debug, Clone)]
pub struct BrowserCore {
    pub id: CoreId,
    pub name: String,
    pub executable: PathBuf,
    pub version: String,
    pub major: u32,
}
```

不要只保存路径；major version 是 capability resolution 的关键输入。

---

## 7. CoreCapabilities

```rust
#[derive(Debug, Clone)]
pub struct CoreCapabilities {
    pub major: u32,
    pub supports_disable_spoofing: bool,
    pub supports_explicit_gpu: bool,
    pub supports_canvas_noise_flag: bool,
}
```

v1 可以先支持：

```text
KnownSupported
Unsupported
Unknown
```

未知 major 不应静默假定兼容。

---

## 8. ProxyProfile

建议区分“浏览器看到的 local SOCKS”和“用户配置的 upstream”。

```rust
#[derive(Debug, Clone)]
pub struct ProxyProfile {
    pub id: ProxyId,
    pub name: String,
    pub outbound: ProxyOutbound,
}
```

```rust
#[derive(Debug, Clone)]
pub enum ProxyOutbound {
    Socks5(Socks5Outbound),
    Http(HttpOutbound),
    Shadowsocks(ShadowsocksOutbound),
    Vmess(VmessOutbound),
    Vless(VlessOutbound),
    Trojan(TrojanOutbound),
}
```

v1 实现顺序建议：

```text
1. Socks5
2. Http
3. Shadowsocks
4. Vless
5. Vmess / Trojan
```

不是一次把所有 Xray 协议 UI 做完。

---

## 9. RuntimeState

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeState {
    Stopped,
    Starting,
    Running,
    Stopping,
    Failed { message: String },
    Crashed { message: String },
}
```

不要用 `bool running`。

---

## 10. RuntimeSession

```rust
use std::time::SystemTime;

#[derive(Debug)]
pub struct RuntimeSession {
    pub profile_id: ProfileId,
    pub state: RuntimeState,
    pub browser_pid: Option<u32>,
    pub xray_pid: Option<u32>,
    pub cdp_port: Option<u16>,
    pub socks_port: Option<u16>,
    pub started_at: Option<SystemTime>,
    pub effective_args: Vec<String>,
}
```

`Child` handle 不放在 domain；由 runtime crate 内部私有结构持有。

---

## 11. LaunchPlan

```rust
use std::ffi::OsString;

#[derive(Debug, Clone)]
pub struct LaunchPlan {
    pub profile_id: ProfileId,
    pub browser_executable: PathBuf,
    pub browser_args: Vec<OsString>,
    pub user_data_dir: PathBuf,
    pub cdp_port: u16,
    pub xray: Option<XrayLaunchPlan>,
}
```

```rust
#[derive(Debug, Clone)]
pub struct XrayLaunchPlan {
    pub executable: PathBuf,
    pub config_path: PathBuf,
    pub socks_port: u16,
}
```

`LaunchPlan` 一旦构造完成，在本次启动中不可变。

---

## 12. Validation

建议将验证分为三层：

### Domain validation

不依赖环境：

```text
profile name 非空
window > 0
hardware_concurrency in 1..=128
language 非空
timezone 非空
```

### Configuration validation

依赖关联数据：

```text
core_id 存在
proxy_id 存在
core capability 支持当前 persona
```

### Runtime validation

依赖机器环境：

```text
browser executable exists
xray executable exists
user-data-dir 可创建/可写
动态端口可用
Xray config 可生成
```
