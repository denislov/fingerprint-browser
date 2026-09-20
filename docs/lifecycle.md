# Runtime Lifecycle v1

## 1. 状态机

```text
                       ┌────────────┐
                       │   Failed   │
                       └─────▲──────┘
                             │
Stopped ──start──> Starting ─┼────> Running
   ▲              │          │        │
   │              └──────────┘        │ stop
   │                                  v
   └────────────── Stopping <─────────┘

Running ──unexpected exit──> Crashed
Crashed ──cleanup──────────> Stopped
```

状态含义：

- `Stopped`：没有该 Profile 的受管运行进程；
- `Starting`：正在准备/启动 Xray 或 Chromium，CDP 尚未 ready；
- `Running`：Chromium 已通过 CDP readiness；
- `Stopping`：正在回收 Chromium / Xray；
- `Failed`：本次启动在进入 Running 前失败；
- `Crashed`：已经 Running 后出现意外进程退出。

---

## 2. Start sequence

```text
UI
 │ start(profile)
 v
Application
 │ load profile/core/proxy
 │ static validation
 v
RuntimeSupervisor
 │ state = Starting
 │
 ├─ 1. runtime validation
 │     browser executable
 │     xray executable
 │     user-data-dir
 │
 ├─ 2. allocate ports
 │     socks_port (if proxy)
 │     cdp_port
 │
 ├─ 3. resolve core capabilities
 │
 ├─ 4. build Xray config
 │
 ├─ 5. build immutable LaunchPlan
 │
 ├─ 6. emit effective args
 │
 ├─ 7. start Xray
 │
 ├─ 8. wait local SOCKS ready
 │
 ├─ 9. start Chromium
 │
 ├─10. wait CDP ready
 │
 └─11. state = Running
```

如果任一步失败，进入 rollback。

---

## 3. Start rollback

### Xray 启动失败

```text
Starting
  -> kill Xray if partially started
  -> delete temp config
  -> Failed
```

### Chromium spawn 失败

```text
Starting
  -> stop Xray
  -> clean temp config
  -> Failed
```

### Chromium spawn 成功但 CDP 未 ready

```text
Starting
  -> terminate Chromium tree
  -> stop Xray
  -> clean temp config
  -> Failed
```

v1 不建议 Ant Browser 那种“窗口已起来但 CDP 未 ready 时继续后台附着”的复杂兼容行为。

个人版应优先确定性：

> 在超时窗口内未 ready，则本次启动失败并清理。

---

## 4. Stop sequence

```text
Running
  -> Stopping
  -> request Browser.close over loopback CDP
  -> wait up to 2 seconds for exit (if request succeeds)
  -> terminate browser tree if needed
  -> stop Xray
  -> delete temp Xray config
  -> delete session record
  -> clear RuntimeSession
  -> Stopped
```

停止是幂等操作：

```text
stop(Stopped) -> no-op success
stop(Stopping) -> no-op / already stopping
```

真实 Chromium 验收发现直接强杀会丢失尚未落盘的 Cookie，因此正常 Stop、Restart、Shutdown 优先请求优雅关闭。
CDP 不可用时直接强制回收；崩溃和启动回滚仍立即强制回收，尤其 Xray 崩溃不能等待浏览器落盘。
异常退出或强制关闭时不保证 Cookie 持久化。Linux 实测结果见 `chromium-acceptance.md`。

---

## 5. Crash handling
### Browser crash

如果 Chromium 在 `Running` 状态意外退出：

```text
Running
  -> Crashed(browser)
  -> stop Xray
  -> cleanup
  -> Stopped
```

v1 不自动重启。

### Xray crash

如果绑定代理的实例中 Xray 意外退出：

```text
Running
  -> Crashed(xray)
  -> terminate Chromium
  -> cleanup
  -> Stopped
```

原因：浏览器继续运行可能造成网络行为与预期代理出口不一致。

这是一条重要的 fail-closed 原则。

---

## 5b. Startup reclaim（上一次运行留下的进程）

被直接杀死（`SIGKILL`、崩溃、非 WM 协议销毁窗口）的进程无法回收自己的子进程，
所以每个 `Running` 会话都会把自己的子进程写进 `data/runtime/<profile-id>/session.json`：

```text
profile_id / cdp_port / socks_port / started_at
browser: pid, executable, args, start_time
xray:    pid, executable, args, start_time   (optional)
```

下一次启动在窗口打开之前、在接受任何命令之前读回这些记录：

```text
reclaim_orphans()
  -> 读取 session.json
  -> 判定每个子进程：Gone / Ours / 不可识别
  -> Ours: 先 SIGTERM 进程组，宽限期内未退出再 SIGKILL
  -> Gone: 清理残留的 xray.json（里面是上游凭据）
  -> 不可识别: 不动它，记录保留，由 UI 上报
  -> 停止后删除记录与 xray.json
```

**pid 不是证据**：pid 会在两次运行之间被回收给无关进程。只有满足以下条件才发信号：

- 记录中的 `start_time` 与存活进程的 `/proc/<pid>/stat` 一致（同一进程实例）；或
- 无法读取 `start_time` 时，存活进程的命令行仍以记录中的参数结尾——包括
  被进程重写成单个字符串的 `/proc/self/cmdline`（Chromium 就是如此）。

无法识别的记录一律上报而不是猜测：杀错别人的进程比留下我们自己的更糟。

---

## 6. Application shutdown

桌面应用退出：

```text
ShutdownAll
  -> reject new Start commands
  -> stop all active Chromium sessions
  -> stop all Xray processes
  -> remove session records and temporary Xray configs
  -> flush storage/log
  -> exit GPUI application
```

不得直接依赖父进程退出后由 OS“顺便”回收全部子进程。

`SIGINT` / `SIGTERM` / `SIGHUP` 与 Quit 动作走同一条路径：信号处理函数只向 self-pipe
写一个字节（异步信号安全），由专用线程请求 `ShutdownAll`、等待宽限期并退出 0。
`SIGKILL` 无法被处理，它留下的进程由下一次启动的 reclaim 兜底（见 5b）。

---

## 7. Duplicate Profile lifecycle

复制实例：

```text
source profile
  -> copy static persona/core/proxy/window
  -> generate new UUID
  -> generate new fingerprint seed
  -> allocate new user-data-dir
  -> persist new profile
```

默认不复制原 Profile 的 user-data-dir 内容。

后期如果要做“克隆已登录状态”，应作为独立动作：

```text
CloneProfileData
```

并要求源实例停止。

---

## 8. Effective configuration

Profile 编辑页展示的是 declared config：

```text
seed = 123
platform = windows
...
```

Runtime 详情展示的是 effective config：

```text
resolved core = 144.x
socks port = 53122
cdp port = 53123
launch args = [...]
xray config path = runtime/sessions/<profile>/xray.json
```

两者必须分开。

---

## 9. Runtime session directory

建议：

```text
data/runtime/<profile-id>/
├── session.json   # 当前会话的子进程记录，停止/回收后删除
├── xray.json      # 临时凭据，停止/回收后删除
├── xray.log
└── browser.log   (optional)
```

它是临时运行态，不属于 user-data-dir。
停止后删除敏感临时凭据与 session record；下一次启动先按 session record 回收残留进程（见 5b）。

---

## 10. Xray readiness

启动 Xray 后不要立刻启动 Chromium。

最小检查：

```text
TCP connect 127.0.0.1:<socks_port>
```

成功后才进入 Chromium spawn。

---

## 11. CDP readiness

Chromium spawn 后：

```text
poll http://127.0.0.1:<cdp_port>/json/version
```

满足：

```text
HTTP success
+ valid JSON
+ Browser/WebSocket metadata available
```

才进入 `Running`。

建议：

```text
poll interval: 100-250 ms
timeout: 10-15 s
```

这些值应进入 Settings，而不是 Profile。

---

## 12. Runtime concurrency

Supervisor 单线程串行处理 command，但每个 active child 的退出监控可以通过辅助线程/非阻塞轮询实现。

第一版最简单的实现：

```text
Supervisor loop
  recv_timeout(100ms)
  process commands
  poll try_wait() for active children
  emit state changes
```

当前实现会在 Xray readiness 循环和每段至多 100 ms 的 CDP 探测之间读取命令。
Stop 当前启动项会先回收临时进程和配置，再发布 Stopped；不会发布 Started 或 Failed。
ShutdownAll 和命令通道断开会取消启动、清理已有会话并退出主循环。
Restart 当前启动项先取消，再通过待处理队列重新启动，避免递归启动。
其他 Start/Restart 延后执行；Stop 可立即处理其他运行会话，并移除该 Profile 之前排队的启动请求。
每次最多处理 64 条命令，避免持续命令流饿死 readiness 和崩溃轮询。
上述响应时间不涵盖同步文件操作和进程创建。事件发送使用 try_send，不因 UI 消费暂停而阻塞监督器。

CDP 和 SOCKS TCP 端口在规划阶段由监听句柄预留，直到对应子进程 spawn 前释放；错误返回会自动释放句柄。
由于 Chromium/Xray 自行绑定端口，释放到绑定仍存在竞争窗口，并非原子交接；需结合 readiness 和进程退出检查识别启动失败。

个人使用实例数通常有限，这比引入复杂 async runtime 更容易验证。

---

## 13. Windows process tree

Chromium 为多进程结构。

Windows 10+ 已实现独立 Job Object。Chromium 和 Xray 各自一个 Job，句柄由
Supervisor 所有，不被子进程继承：

```text
create Job
set JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
CreateProcessW + PROC_THREAD_ATTRIBUTE_JOB_LIST (atomic assignment)
normal stop -> CDP Browser.close -> bounded wait -> terminate Job
manager killed -> OS closes Job handles -> children terminated
```

创建与绑定是同一步，避免 spawn 后尚未 assign 时管理器死亡留下孤儿。
主进程已退出时仍会终止 Job，回收 renderer / GPU / utility 等后代。
不支持 Job 绑定的环境会启动失败，不退回无保护的启动方式。

Windows 进程身份使用 PID + GetProcessTimes 创建时间（FILETIME）；退出态通过
进程句柄检查。权限不足返回 Unknown，不当成已退出。恢复旧会话时只处理身份匹配者，
终止前再次核对创建时间，并持有进程句柄直到 taskkill /T 完成，防止 PID 被复用。
新会话通常已经由 Job 清理，下一次启动只需删除 stale journal 和临时代理配置。
旧 Windows journal 没有创建时间时保留并报告；不会只凭 PID 杀进程。
恢复失败不会删除 journal/config，也不会报成回收成功。

测试与边界见 [windows-acceptance.md](windows-acceptance.md)。强制清理不保证 Cookie
落盘，正常停止仍优先 CDP 关闭。

---

## 14. Runtime invariants

必须长期保持：

1. 一个 Profile 同时最多一个 ActiveSession。
2. `Running` 必须意味着 CDP 已 ready。
3. 有代理的 `Running` 必须意味着对应 Xray 仍存活。
4. Xray crash 时不得让 Chromium 静默转为非预期网络路径。
5. RuntimeSession 删除前必须完成子进程回收。
6. 动态端口不持久化到 Profile。
7. UI 状态以 Supervisor snapshot 为准；event 仅作非阻塞更新通知，需定期和重连时读取快照。
8. 事件队列满或断开时允许丢弃通知，快照保留最后错误、警告、启动参数和累计 dropped_events；停止不清空诊断，下次启动清空诊断。
9. session record 中的 pid 只有在进程身份（同一 `start_time`，或无该信息时命令行尾部匹配）成立时才可被发信号；无法识别时保留记录并上报。
