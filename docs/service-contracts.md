# Service Contracts v1

## 1. Repository contracts

### ProfileRepository

```rust
pub trait ProfileRepository: Send + Sync {
    fn get(&self, id: ProfileId) -> Result<Option<BrowserProfile>, StorageError>;
    fn list(&self) -> Result<Vec<BrowserProfile>, StorageError>;
    fn insert(&self, profile: &BrowserProfile) -> Result<(), StorageError>;
    fn update(&self, profile: &BrowserProfile) -> Result<(), StorageError>;
    fn delete(&self, id: ProfileId) -> Result<(), StorageError>;
}
```

### ProxyRepository

```rust
pub trait ProxyRepository: Send + Sync {
    fn get(&self, id: ProxyId) -> Result<Option<ProxyProfile>, StorageError>;
    fn list(&self) -> Result<Vec<ProxyProfile>, StorageError>;
    fn save(&self, proxy: &ProxyProfile) -> Result<(), StorageError>;
    fn delete(&self, id: ProxyId) -> Result<(), StorageError>;
}
```

### CoreRepository

```rust
pub trait CoreRepository: Send + Sync {
    fn get(&self, id: CoreId) -> Result<Option<BrowserCore>, StorageError>;
    fn list(&self) -> Result<Vec<BrowserCore>, StorageError>;
    fn save(&self, core: &BrowserCore) -> Result<(), StorageError>;
    fn delete(&self, id: CoreId) -> Result<(), StorageError>;
}
```

---

## 2. Application services

### ProfileService

职责：Profile CRUD 与静态验证。

```rust
pub trait ProfileService {
    fn create(&self, draft: NewProfile) -> Result<BrowserProfile, AppError>;
    fn update(&self, profile: BrowserProfile) -> Result<(), AppError>;
    fn delete(&self, id: ProfileId, mode: DeleteMode) -> Result<(), AppError>;
    fn duplicate(&self, id: ProfileId, new_name: String) -> Result<BrowserProfile, AppError>;
}
```

复制 Profile 时默认：

- 新 UUID；
- 新 user-data-dir；
- 新 fingerprint seed；
- 继承 persona / proxy / core；
- 不复制运行态。

这比完整复制 seed 更符合“新实例”的语义。

---

## 3. CapabilityResolver

```rust
pub trait CapabilityResolver: Send + Sync {
    fn resolve(&self, core: &BrowserCore) -> Result<CoreCapabilities, CapabilityError>;
}
```

职责只有一个：

```text
Core version -> supported fingerprint capabilities
```

不要负责 CLI 拼装。

---

## 4. LaunchPlanner

```rust
pub struct LaunchContext<'a> {
    pub profile: &'a BrowserProfile,
    pub core: &'a BrowserCore,
    pub proxy: Option<&'a ProxyProfile>,
    pub capabilities: &'a CoreCapabilities,
    pub cdp_port: u16,
    pub socks_port: Option<u16>,
}

pub trait LaunchPlanner: Send + Sync {
    fn build(&self, ctx: LaunchContext<'_>) -> Result<LaunchPlan, LaunchPlanError>;
}
```

`LaunchPlanner` 不 spawn 进程。

输入相同、动态端口相同，则输出应稳定。

---

## 5. XrayConfigBuilder

```rust
pub trait XrayConfigBuilder: Send + Sync {
    fn build(
        &self,
        proxy: &ProxyProfile,
        local_socks_port: u16,
        output_path: &Path,
    ) -> Result<(), ProxyError>;
}
```

生成配置时必须：

```text
listen = 127.0.0.1
protocol = socks
```

作为 Chromium 的 local inbound。

---

## 6. RuntimeFacade

这是 Application/UI 唯一需要调用的运行时入口。

```rust
pub trait RuntimeFacade: Send + Sync {
    fn start(&self, profile_id: ProfileId) -> Result<(), RuntimeCommandError>;
    fn stop(&self, profile_id: ProfileId) -> Result<(), RuntimeCommandError>;
    fn restart(&self, profile_id: ProfileId) -> Result<(), RuntimeCommandError>;
    fn snapshot(&self, profile_id: ProfileId) -> Option<RuntimeSnapshot>;
}
```

调用 `start()` 的语义是“命令已接受”，不是“浏览器已经 ready”。

真正启动完成通过 RuntimeEvent 通知。

---

## 7. RuntimeCommand

```rust
pub enum RuntimeCommand {
    Start(ProfileId),
    Stop(ProfileId),
    Restart(ProfileId),
    ShutdownAll,
}
```

---

## 8. RuntimeEvent

```rust
pub enum RuntimeEvent {
    StateChanged {
        profile_id: ProfileId,
        state: RuntimeState,
    },
    EffectiveLaunchArgs {
        profile_id: ProfileId,
        args: Vec<String>,
    },
    Started {
        profile_id: ProfileId,
        browser_pid: u32,
        xray_pid: Option<u32>,
        cdp_port: u16,
        socks_port: Option<u16>,
    },
    Stopped {
        profile_id: ProfileId,
    },
    Crashed {
        profile_id: ProfileId,
        component: RuntimeComponent,
        message: String,
    },
    Warning {
        profile_id: ProfileId,
        message: String,
    },
}
```

UI 应以 event 驱动刷新状态，而不是自行扫描 PID。

---

## 9. RuntimeSupervisor internal ownership

runtime crate 内部：

```rust
struct ActiveSession {
    profile_id: ProfileId,
    browser: std::process::Child,
    xray: Option<std::process::Child>,
    cdp_port: u16,
    socks_port: Option<u16>,
    effective_args: Vec<String>,
}
```

这个类型不跨 crate 暴露。

原因：

- Child 句柄是所有权对象；
- UI / application 不应持有；
- 所有 kill/wait 必须由 Supervisor 串行化。

---

## 10. CdpProbe

```rust
pub trait CdpProbe: Send + Sync {
    fn wait_ready(
        &self,
        port: u16,
        timeout: Duration,
    ) -> Result<CdpInfo, CdpError>;
}
```

v1 只需要访问 loopback 的 CDP `/json/version` 或等价 readiness endpoint。

不需要构建完整 CDP client。

---

## 11. PortAllocator

```rust
pub trait PortAllocator: Send + Sync {
    fn allocate_loopback(&self) -> Result<u16, PortError>;
}
```

v1 允许“探测空闲端口 -> 立即 spawn -> 失败重试”的简单策略。

端口一定是 RuntimeSession 数据，不写回 Profile。

---

## 12. Process termination contract

停止顺序：

```text
1. state -> Stopping
2. graceful close Chromium (if available)
3. timeout
4. terminate browser process tree
5. stop Xray
6. clean temporary Xray config
7. release session
8. state -> Stopped
```

Windows 后期应使用 Job Object；v1 若先做主进程终止，也应把“进程树回收”作为独立接口保留。

```rust
pub trait ProcessTreeController: Send + Sync {
    fn terminate_tree(&self, pid: u32) -> Result<(), ProcessError>;

    /// 回收孤儿时先请求退出（Unix：信号进程组），默认实现不做任何事。
    fn request_tree_exit(&self, pid: u32) -> Result<(), ProcessError> {
        Ok(())
    }
}
```

---

## 12b. ProcessInspector / session journal

pid 不是证据，所以识别一个记录中的进程需要单独的读回接口，并且必须把
“平台问不到”与“进程不在了”分开（合并会让记录在不能读 `/proc` 的平台上全被判成死进程，
回收因此删掉唯一线索并留下孤儿进程）。

```rust
pub enum ProcessReading {
    Absent,                              // pid 未被占用或进程已退出
    Unknown,                             // 平台无法回答，或进程仍在 fork 与 execve 之间
    Live(ProcessIdentity),               // argv + start_time
}

pub trait ProcessInspector: Send + Sync {
    fn inspect(&self, pid: u32) -> ProcessReading;
}
```

会话记录（`runtime::journal`）与回收：

```text
SessionRecord { profile_id, cdp_port, socks_port, started_at,
                browser: ProcessRecord, xray: Option<ProcessRecord> }
ProcessRecord { pid, executable, args, start_time }

journal::write / read / remove / list
journal::reclaim(runtime_dir, inspector, tree, graceful) -> ReclaimReport
journal::confirm(record, inspector) -> Option<String>
```

`ReclaimReport`（reclaimed / unresolved / stale）是运行时返回的结构化结果，
面向用户的一行文案由 UI 层生成（它才知道 profile 名字）。

---

## 13. Error taxonomy

不要所有错误都变成 `String`。

建议：

```text
StorageError
ValidationError
CapabilityError
ProxyError
LaunchPlanError
ProcessError
JournalError
CdpError
RuntimeError
AppError
```

UI 边界再统一转换为用户可读文本。

---

## 14. GPUI integration contract

GPUI 层只做三件事：

```text
1. dispatch application command
2. subscribe runtime events
3. render application state
```

禁止：

```text
View -> rusqlite connection
View -> std::process::Command
View -> Xray JSON builder
View -> CDP HTTP request
```
