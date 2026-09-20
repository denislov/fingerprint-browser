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
指纹能力随后由回读认证：148（已验证代）与 **142（低于 pivot 的最后一版）**都在本机实测过。

Phase 4 的剩余项在第十二批结清，结论与证据见 [fingerprint-matrix.md](fingerprint-matrix.md)：

- **跨平台字体策略：实测推翻了继承来的规则。** `Ant-Browser` 的“目标平台≠宿主平台就补
  `font` 到 `--disable-spoofing`”在本机实测下会把一个伪造但对齐的字体清单换成宿主的真清单，
  反而向页面暴露宿主平台；而它原本要修的缺字问题，它根本修不了（字形永远是宿主的）。
  因此产品**不自动加** `font`，每个 profile 仍可在编辑器里自己勾选。
- 音频 / WebRTC / 字体的运行时回读在第三、四批完成；第十二批把字体面补齐为三条可单次定论的读数
  （可读性 / CJK 字形 / emoji 字形），emoji 是新增的那条。
- browser version detection 每次开工都会复核（`app::core_detect::maintain`）。
- 地理位置仍无模型字段，`--fingerprint-location` 在本环境产不出坐标，见
  [fingerprint-matrix.md](fingerprint-matrix.md) 的“尚未实现”。

### 第十二批：跨平台字体的实测与结算（结清 Phase 4 的最后一条）

进度（2026-09-20）：已完成。**这一批的产出是一个被推翻的假设，而不是一个新开关。**

- 新增可重复的实测：`the_font_exclusion_changes_what_a_spoofed_platform_enumerates`。
  同一 seed、同一 Linux 宿主，三种声明平台 × 有/无 `--disable-spoofing=font` 共六次回读，
  开关作为**原始参数**注入（用 `RawArgsPlanner`），所以测的是引擎而不是能力表。
  结果：声明 windows/macos 时，不加开关会枚举到 Tahoma/Cambria/Comic Sans MS 这类**宿主没有**
  的字体；加开关后回落到宿主真实清单；声明 linux（即宿主）时开关是 no-op。
  六次读数的宽度集合完全一致，CJK 与 emoji 在六次里都有字形。
  也就是说：**开关换的是“声称”，不是“渲染”**。
- 结论：字形永远是宿主的——所以“排除 font 以免缺字”在机制上不可能成立；而排除之后清单会变成
  宿主清单，等于向任何枚举字体的页面宣布“我不是 Windows”。产品因此不自动加 `font`，
  把选择留给 profile 编辑器（`SpoofingFeature::Font` 本来就在那里）。
- 同一实测顺手认证了能力表在这个特性上的取值：148 尊重该开关、142 忽略（两两清单相同），
  与 canvas/clientrects/audio 同一分界，所以 `supports_disable_spoofing` 一个标志覆盖它们是对的。
- 回读面按实测补齐：`verify_fingerprint` 原来只判 CJK，现在判三条单次读数可定论的问题——
  **字体面是否读得到**（枚举与宽度缺一不可，且会说缺的是哪一半）、**CJK 是否有字形**、
  **emoji 是否有字形**。emoji 那条抓的是“宿主没装 emoji 字体”，与声明的平台无关，
  这正是跨平台字体里真正可验证的部分。`ObservedFingerprint::has_boxed_emoji()` 与
  `has_missing_cjk()` 共用同一个 `boxed()` 判定。
- 实测还抓出一个与本批无关但必须修的旧 bug（已单独提交）：会话记录写得太早（readiness 之前，
  这是上一批有意改的），而 `ProcessRecord::captured` 拿 start time 走的是 `inspect`，
  需要 `/proc/<pid>/cmdline`——没 exec 完的进程没有 cmdline，于是 start time 记成了 `None`。
  等到回读时，Chromium 已经重写并追加过自己的参数，记录与进程对不上，启动时报
  “the next run will not reclaim them”。修法是 `ProcessInspector::start_time`：
  从 `/proc/<pid>/stat` 单独读第 22 个字段，进程一创建就有。

### 第十三批：日志落盘与过滤，以及 opener 的真实结果

进度（2026-09-20）：已完成。**这一批把上一批的可观测性从“窗口活着时”扩到“窗口关闭后”，
并消掉一个“spawn 成功就算打开成功”的自欺。**

- `app::log_file`（新）：`LogFile` 把页面上的每一行同时写到数据目录下的
  `logs/activity.log`。行格式是绝对 UTC 时间 + 级别 + profile 名 + 消息；时间戳由本仓
  的 `civil_from_days` 算（不引日期库，含闰日测试）。超过 512 KiB 轮转为 `activity.log.1`，
  下次轮转覆盖它——**最多两个文件，不无界增长，也不按运行堆碎文件**。
- 失败不静默也不重试刷屏：目录建不出来/不可写时启动就记下错误（页面头部直接显示
  “Not written to a file: …”），写入失败**只报一次**（一个 toast；toast 不走日志，不会递归）。
- Log 页新增 **All / Warnings / Errors 过滤**：`LogFilter` 在 `log_rows()` 里生效，
  `log_len()` 保留总数，所以空页会说清是哪种空（“No error lines; 3 were recorded …”）。
  按 profile 过滤没做：每行已经标明归属，而 Log 页再放一个 profile 选择器只增长 UI 债务。
- **opener 的结果能到窗口了**。`DirectoryOpener::open` 的契约从“spawn 完就返回”改成
  “等到桌面接手为止（有界）”：exit 0 = 打开了；退出码非 0 = 报出程序与退出码（
  `xdg-open` 找不到 handler 就是这条路）；超过 5 秒仍在跑 = 前台文件管理器，算打开。
  它在 worker 线程上跑，结果经 channel 由 tick 取回——与指纹回读同一套模式，
  `wait_for_state` 测试助手同时驱动两条队列。真机验收：把 `xdg-open` 换成一个
  `exit 3` 的包装脚本，界面红横幅 + error toast + 文件日志都出现了
  “xdg-open exited with exit status: 3; nothing may have opened …”；目录不存在那条拒绝路径
  也走同一条异步通道。
- 测试卫生：`AppState::new` 会写真实数据目录，所以测试改用 `AppState::for_test` /
  `with_log`（显式注入 sink），测试不会往 Cargo.toml 旁边的 `data/` 写东西。

### 第十四批：Runtime Details 分页，以及 X11 窗口被销毁这条限制

进度（2026-09-20）：已完成（面板部分）/ 已定调（窗口部分）。

- **Runtime Details 面板改成三页**：`DetailsTab::{Details, Args, Log}`。面板原来是一条长滚动：
  身份、诊断、对账结论、整条启动命令行挤在一起，而现在这三个视图对应三个不同的问题
  （它是什么 / 它是拿什么启的 / 它做了什么），通常只问其中一个。
  Args 页会滚动（`max_h` 没变），Log 页只列**这个 profile 自己的**行（最新 20 条）——
  全量在 Log 页，面板只服务“正在看的这个 profile”。标题行保留原来的六个动作。
  实现上：三个视图一旦 `.id()` 就变成不同类型，所以面板先 `into_any_element()` 再选。
- **X11 下“窗口被别的客户端销毁”的结论：不做 watchdog，靠 journal 兑现可靠性。**
  实测（`xdotool windowclose`）：进程不退出，且 gpui 的 X11 后端会不断打
  `X11 QueryPointer failed ... bad_value: <window>` —— 它**知道**窗口不在了（那些错误），
  只是当成错误日志丢弃。上游没有对主窗口处理 `DestroyNotify`（grep 确认：只有 clipboard
  那条路径处理）。自己加 watchdog 需要：新增 `x11rb` 直接依赖 + 用第二个连接按窗口名轮询根
  窗口子树 + 处理“刚启动还没窗口”的误判，代价不小。而它要保护的东西（子进程与状态）已经由
  `session.json` + 下一次启动的 reclaim 兑现——**而这个 reclaim 在此之前是坏的**（见第十二批
  修掉的 start time bug），修完后 `chromium_real` 的“被杀死的一轮留下的浏览器被回收”在真机上
  通过。所以这一项的结论是：限制留在 README，不上 X11 依赖；要真要 watchdog，那是另一个有界任务。

### 第十五批：把 pivot 自己那一版（144）测了，并结算内核的名字

进度（2026-09-20）：已完成。**这一批消掉的是“整张表里唯一一行靠继承的证据”。**

第九批用 142 推翻了继承来的结论，但 pivot 本身（144）在本仓库从没测过：`for_major` 在 144 这
一格上写的是兄弟项目 `Ant-Browser` 的读数。pivot 是整张表的分界，不该是唯一借来的那一行。

- 上游 `adryfish/fingerprint-chromium` 的 Linux 资产里，144 就是 pivot 本身（第九批已注明没有
  143 的 release）。这个包用户早就下载了，一直没解；本批解出来实测（`--version` →
  `Chromium 144.0.7559.132`），命令与 142/148 完全相同：
  `CHROMIUM_BIN=<144 的 chrome> cargo test -p runtime --test fingerprint_real -- --ignored`。
- 结果：**继承来的结论这次是对的，但它现在是实测。**
  `--disable-spoofing=canvas|clientrects|audio` 在 144 上**生效**（两个 seed 的读数完全相同，
  说明 seed 到不了那个面）；`--fingerprinting-canvas-image-data-noise` 改 `toDataURL` 而不改
  `getImageData`——与 148 一致。同 seed 的 canvas 读数与 142/148 **逐字节相同**（无 noise
  `1251849731`，有 noise `368676017`，`getImageData` 均 `4160716610`），与第九批在 142 上记录的
  数字一致，说明三台确实走在同一条路径上（`the_capability_table_matches_this_build` 打印的
  就是这几个数）。
- 三台上都是 11/11：142、144、148。测试按二进制自报的 major 断言能力表，所以覆盖的是
  “表对 144 说了什么”而不是一个写死的数字。于是 pivot 两侧都由本仓库的实测支撑，只剩
  145–147 仍按兄弟项目的 144 读数继承。
- 顺手结算内核的**名字**：同一个包，`chromium-acceptance.md` 叫它 ungoogled-chromium，
  `fingerprint-matrix.md` 叫它 fingerprint-chromium。实测把这个矛盾解决了——这些
  `ungoogled-chromium-<version>-1-x86_64_linux.tar.xz` 就是 `adryfish/fingerprint-chromium`
  的 Linux 资产名，而它们确实实现了指纹开关（本批实测的三台都是 11/11）。所以：
  `fingerprint-matrix.md` 开头写清名字的来源；`chromium-acceptance.md` 的 Limits 从
  “这是 ungoogled，不是指纹”改成“运行路径与指纹回读跑在同一批二进制上，两半各自证明什么”；
  README 里“Phase 3 要再用 fingerprint-chromium 重做一次验收”这条**已经成立**，不再是待办。
- 二进制仍然解在仓库之外、不入库（沿用既有约定）。

### 第十六批：其余四种 outbound 的配置（以及一个被引擎删掉的开关）

进度（2026-09-20）：已完成。**把“模型能存六种、配置只能建两种”补成六种，并且用引擎自己当裁判。**

- domain 新增 `StreamSettings`（`network`: tcp/ws/grpc，`security`: none/tls/reality，外加
  `tls`/`reality`/`ws`/`grpc` 四个可选块），挂在 Shadowsocks/VMess/VLESS/Trojan 四种 outbound
  上；SOCKS5/HTTP 不带，所以“配了一个永远不会被读的 stream”在**类型层面**就写不出来。
  `#[serde(default)]` 让已存的代理（旧 JSON 没有这个字段）照常读出。
- `validate_proxy` 新增 stream 规则，分两类：引擎自己也拒绝的（REALITY 只能走 tcp），
  以及**引擎完全不吽声的**——`security: none` 旁边的 `tls` 块、`network: tcp` 旁边的 `ws` 块。
  实测 `xray run -test` 对后者是 exit 0：没人读，也没人说。这正是本批加校验的理由——
  “写了但会被静默丢掉”必须被点名拒绝。
- `DefaultXrayConfigBuilder` 覆盖六种协议并写出 `streamSettings`；生成前先过
  `domain::validate_proxy`，于是“能存的”和“能生成的”永远同一套规则，不再有两份校验。
- **判据不是我们写的字段名，是引擎。** 新增 opt-in 测试
  `every_shape_the_builder_makes_is_accepted_by_the_engine`：把每种形状用真构建器写成文件，
  交给 `xray run -test`（只解析、不开 socket，所以不需要服务器与网络）。十个正例全部被接受；
  反例必须被拒绝——未知 cipher（cipher 名单由引擎定义，不由我们枚举），以及把 `allowInsecure`
  手工注回构建器写出的配置（证明这个探针不是空转）。
- **两个由实测改变的方案**：
  1. `allowInsecure` 在 26.2.6 已被**移除**（exit 23，提示改用 `pinnedPeerCertSha256`）。
     兄弟项目 Go 版靠“内部标记 + 运行时抓对端证书算 pin”绕开它。本仓库因此不建模这个开关：
     模型里没有能变成它的字段。
  2. SS（无 FS）/ Trojan / WebSocket / gRPC 在 26.2.6 都带 deprecation warning（仍可用），
     xhttp 是新的推荐传输；`x25519` 现在把公钥打印为 `Password:`，而 REALITY 的 `publicKey`
     与新的 `password` 两种写法都被接受（本仓库发 `publicKey`，与分享链接里的 `pbk` 一致）。
- 测试自己踩的两个坑（都修了，值得记）：注入 `allowInsecure` 那一步一开始替换的是
  `"serverName"`，而该用例根本没生成 `tlsSettings`，于是“引擎接受了它”其实是什么都没注入
  —— 现在注入前后断言文本必须变化；另外 `-test` 的拒绝信息走 **stdout**，只收 stderr 会拿到
  一条空理由。
- 表单仍是两页：四种新协议的编辑器，以及“粘贴 `vless://` `vmess://` `trojan://` `ss://`
  导入”，是下一批。`runtime::is_supported` 现在的含义是“表单能不能填”，不是“配置能不能建”。
- 真实远端出口的验收仍需真服务器：本批证明的是“生成的配置引擎接受”，不是“这个服务器能通”。

### 第十七批：分享链接的解析

进度（2026-09-20）：已完成（解析层；界面下一批）。

- `domain::uri`（新）：`ss://` / `vmess://` / `vless://` / `trojan://` 解析成现有模型。新增三个
  依赖（`base64`、`url`、`percent-encoding`），三个都早已在依赖图里（由 gpui 那条链引入），
  不是新的构建成本。
- **规则写在模块文档里**：认识的字段但模型装不下 → 点名拒绝（`plugin`、`type=kcp|xhttp|raw`、
  `headerType=http`、`mode=multi`）；完全不认识的 query 参数 → 放过（客户端会加自己的参数，
  为没见过的名字拒绝可用链接更糟）。
- **三个由实测决定的取值**：
  1. vmess 链接的 `aid` **保留**而不是丢掉：`xray run -test` 接受非零 alterId，而按 0 发出去
     只会得到一个连不上且没有解释的配置。模型因此新增 `VmessOutbound::alter_id`，非零才写出
     （0 就是引擎默认值，用“不写”表达）。
  2. trojan 链接默认 TLS（显式写 `security=none` 才关），因为这个协议在现实里没有明文部署；
     引擎接受明文，所以显式写 none 的链接按它说的来。
  3. `type=raw` 与 `type=xhttp` 引擎都接受，但模型没有它们 → 拒绝并点名，而不是当成 tcp 混过去。
- 解析出的东西**必然也能过 `validate_proxy`**：`Transport::stream` 结尾直接调同一份
  `validate_stream`，所以“reality 走 ws”这类矛盾在粘贴那一刻就被拒绝，错误信息就是 domain 那句。
- 验收闭环（opt-in）：`links_from_the_wild_become_configs_the_engine_accepts` 让 8 个真实形状的
  链接走完 解析 → 模型 → 配置 → `xray run -test`，全部被引擎接受；第 9 个（带 `plugin` 的 ss）
  必须被本程序拒绝。
- 界面还没接：下一批做粘贴对话框，并顺手拆掉 `ProxyService` 那道协议闸门——构建器现在六种都能建，
  真正的限制是“表单能不能填”。

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
  两层闸门：先过 domain 校验，再过 `runtime::is_supported`（当时配置构建器只支持
  SOCKS5 与 HTTP；第十六批后构建器六种都支持，这道闸门变成“表单能不能填”，见第十六批）。**存不下的代理不如早点拒绝**：四个构建不了的协议在表单里被列出来并说明原因，
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

### 第九批：认证第二个 major（142），并据此修正能力表

进度（2026-09-20）：已完成。**这一批是行为修正，不是补文档。**

上游 `adryfish/fingerprint-chromium` 的 Linux 资产里，142 是**低于 pivot 的最后一个版本**
（没有 143 的 release，144 是 pivot 本身）。用户下载了 142，本机实测（`--version` → Chromium 142.0.7444.175）。

**实测结果推翻了继承来的结论**（`CHROMIUM_BIN=<142 的 chrome> cargo test -p runtime --test fingerprint_real`）：

| 开关 | 142 | 148 | 原能力表 |
| --- | --- | --- | --- |
| `--fingerprinting-canvas-image-data-noise` 改 `toDataURL` | **是** | 是 | 仅 ≥144 → **错**：142 被无故禁用了引擎支持的开关 |
| 同上，是否改 `getImageData` | 否 | 否 | （不变式，一致） |
| `--disable-spoofing=canvas|clientrects|audio` 是否生效 | **否** | 是 | 两代通用 → **错**：142 接受并忽略它 |

也就是说：**两个"不稳定开关"分处 pivot 两侧，而原来的表把它们写反了**。

改动：

- `CoreCapabilities::for_major`：`supports_disable_spoofing = 生成代（≥144）`，
  `supports_canvas_noise = true`（142 与 148 都实测生效）。文档写清每个数字测于哪个 major。
- `compat::check`：`--disable-spoofing` 的省略原来**没有任何上报**（因为它被当作两代通用），
  现在会报告，并按"低于已验证代"给出理由；noise 分支改为**由能力而非代际**驱动
  （否则会给一个其实拿到了开关的 142 核心报"缺失"）。
- Cores 页的能力行改为描述真正有差异的那一项：`spoofing exclusions honoured / not honoured`
  （`noise switches verified` 已不成立）。
- 集成测试**按二进制自报的 major 判定契约**（`Harness::detected()` 探测 `--version`），
  所以同一套测试对任何内核都成立；新增
  `the_capability_table_matches_this_build`：把开关当**原始参数**加上去直接测引擎
  （用 `FixedCapabilities` 固定能力表，避免"用被测对象去测它自己"），再与 `for_major` 对比。
- 两台上都跑：142 与 148 各 10/10 通过；142 上失败过的三个旧测试改为按 major 分支断言。

同一 seed 在两个 major 上给出**逐字节相同**的 canvas 读数（无 noise：`1251849731`，
有 noise：`368676017`，`getImageData` 均为 `4160716610`），说明这套回读确实在同一条路径上对比。

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
Log                 （已完成，第十一批）
Runtime Details     （已完成）
```

仍未实现：无。三个「锦上添花」（toast / open user-data-dir / 日志面板）
已在第十一批完成，见下。

已知限制：非 WM 关闭协议直接销毁窗口（如 `xdotool windowclose`）时，gpui 可能不感知
窗口已消失，进程会继续运行并持有浏览器；支持的退出方式是窗口管理器关闭按钮、窗口内
Quit 按钮，以及 `SIGINT`/`SIGTERM`/`SIGHUP`（第十批）。该问题在 gpui 的 X11 后端，
不在本仓库接线代码；它留下的子进程由下一次启动的 reclaim 兜底。

---

### 第十一批：三个锦上添花（toast / open user-data-dir / 日志面板）

进度（2026-09-20）：已完成。**这一批不新增页面能力，而是把“发生了什么”变得看得见、
留得住。**

- **toast**：`AppState` 新增 `Toast` 队列（`ToastKind::{Success, Warning, Error}`），
  由 `drain_toasts()` 交给视图。真正推送在**后台 tick 里**、而不是点击处理函数里
  （`poll_runtime` → `cx.update_window` → `window.push_notification`），
  所以“内核忽略某个开关”“浏览器崩了”这类自己到来的事件和按钮一样能弹出来。
  这里有个 gpui 的实现细节：push 会去改通知层所在的 root view，所以必须在
  `this.update(...)` 返回之后再 `update_window`，不能在实体更新闭包里做。
- **横幅语义变了**，这是一次行为修正而不是换皮：横幅只留给**问题**——它留到用户点
  Dismiss；成功只发 toast（且会清掉当前横幅，因为横幅是“当前的问题”而不是“所有历史问题”）。
  被判为错误的操作是两处都出：横幅留证，toast 当场可见。
- **日志面板**（新的 Log 页，侧边栏第五页）：`AppState::record_event` 把 `RuntimeEvent`
  翻成人话——starting/stopping、failed、`browser started (pid, cdp port, socks port, xray pid)`、
  browser stopped、`xray crashed: …`、warning、`launching with N arguments`。
  Running/Stopped/Crashed 的 `StateChanged` 有意不记，因为 `Started`/`Stopped`/`Crashed`
  已经各有一行——同一件事不写两遍。每条 notice（成功或失败）、每次指纹回读结论
  （confirmed / N claims not confirmed / unreadable）也进日志。行按最新在前，带
  相对时间、级别、profile 名（或 `app`），容量 500 行，有 Copy / Clear。
  `RuntimeEvent` 以前只是“该重读快照了”的信号，事件载荷**当场丢弃**；现在它至少有了
  一份会话内的成文记录（仍不是持久化审计，契约不变）。
- **open user-data-dir**：`app::open_dir` 新增 `DirectoryOpener` trait + `SystemDirectoryOpener`
  （`xdg-open` / `open` / `explorer`，由纯函数 `opener(path, os)` 给出，三个平台都可测）。
  子进程 spawn 后在独立线程 wait 回收，避免僵尸进程、也不阻塞 UI；
  opener 退出码非 0（桌面没注册 handler）只写 `tracing::warn`，因为从 spawn 看不出来，
  也不好意思把它当成“已打开”。
  目录不存在时**拒绝**并报出路径（提示先启动一次让浏览器创建），而不是顺手建个空目录——
  空目录看起来像状态。相对路径先按工作目录解析（与 Settings 页的展示一致），因为 opener
  是外部进程，不保证共享本进程的 cwd。视图注入 opener（同 verifier 的做法），headless 测试
  点真实按钮、由 fake 收下路径，真机不会弹文件管理器。
- 测试：state 侧覆盖 toast 入队/出队、notice→日志、事件→日志（逐条断言文案）、
  容量裁剪、`log_rows` 命名与倒序；UI 侧覆盖“点一下就能在窗口的通知列表里看到一个 toast”、
  “问题同时在横幅和 toast 里”、“Log 页只渲染自己”、“日志行内容与清空”、“open dir 收到
  正确路径”、“open 失败给出原因”。UI 测试直接调 `show_toasts`（即 tick 用的那段逻辑），
  不等 200ms 定时器。

---

### 第十批：进程生命周期与退出路径的硬化

进度（2026-09-20）：已完成。**这一批消掉的是“会真丢状态”的缺口，不是补页面。**

- `runtime::journal`（新）：每个 `Running` 会话把子进程写进
  `data/runtime/<profile-id>/session.json`（临时文件 + rename 原子落盘，0600），
  停止与回收后删除。记录带 pid、可执行文件、启动参数，以及**从活进程读回**的
  `/proc/<pid>/stat` start time。
- `RuntimeSupervisor::reclaim_orphans()`：窗口打开前、接受任何命令前调用，读回记录并
  回收上一次运行留下的进程：先 `SIGTERM` 进程组（让 Chromium 落盘），宽限期内没退出
  再 `SIGKILL`；顺手删掉残留的 `xray.json`（里面是上游凭据）。进程已不在的记录也会被清掉。
- `process::ProcessInspector` / `ProcessReading`：**pid 不是证据**。判定分三态
  （Absent / Unknown / Live），绝不把“平台问不到”与“进程不在了”合并——在不能读
  `/proc` 的平台上，合并会让每条记录都像是死进程的记录，回收就会删掉唯一线索并把
  浏览器留在那里。身份成立的条件是同一 pid 的**同一 start time**；读不到 start time 时
  退回命令行匹配。
- `app::signal`（新）：`SIGINT`/`SIGTERM`/`SIGHUP` 与 Quit 走同一条退出路径。
  处理函数只做异步信号安全的事（向 self-pipe 写一个字节），由专用线程请求 `ShutdownAll`、
  等宽限期、`exit(0)`；pipe 两端都设 `FD_CLOEXEC`，子进程不会继承。安装失败只告警不致命。
- `app::reclaim`（新）：把回收报告翻成一行横幅文案（纯函数 + 测试），并同时写日志。
  横幅只有一行，所以回收提示**最后**入栈：设置/内核问题在自己的页面上还在，而被停掉的
  浏览器只有这一处会说。
- 测试卫生：`journal` 的测试助手对 `ETXTBSY` 做有界重试（“刚写完的可执行文件立刻 exec”
  是内核/glibc 已知竞态，glibc 的 `execvp` 也这么做；生产从不写自己要启动的二进制）。

**这一批又被真机推翻了两个假设**（都是“测试绿但断言在空转”那一类）：

1. **Chromium 会重写自己的 `/proc/self/cmdline`**，整条命令行变成一个字段（没有 NUL 分隔）。
   按“参数向量尾部匹配”识别记录，会把明明是自己的浏览器判成陌生人而拒杀。现在身份 = 同一
   pid 的同一 start time（内核不会把回收的 pid 配上死者的 start time），命令行只在读不到
   start time 时兜底，并且同时接受两种形状（分离字段的尾部 / 单字符串的结尾）。
2. **验收探针自己写错了**：它把 profile 目录当裸词找，而浏览器持有的是
   `--user-data-dir=<dir>`，于是“没有浏览器残留”在什么都没看的情况下永远是绿的。改成按 flag
   匹配后，孤儿回收测试才真的在断言。

真机验收（有证据）：真实 148 上跑 `chromium_real` 3/3（含新增的“启动中途取消”与
“被杀死的一轮留下的浏览器被回收”）、`xray_real` 1/1、`fingerprint_real` 两者各 10/10
（148 与 142）。窗口里又走了一遍完整链路：新建 profile → Start（真实 Chromium + 落盘记录）
→ `kill -9` app（浏览器存活、记录还在）→ 重新启动 app，日志与横幅均出现
`a previous run left 1 browser session running; Profile 1 (browser pid 496461)`，浏览器与记录都被清掉；
另一次在浏览器运行时 `kill -TERM` app，日志出现
`received signal 15; reclaiming child processes before exiting`，退出后无残留浏览器、无残留记录。

顺手清掉的技术债：删除了 `.cargo/config.toml`（`RUSTC_BOOTSTRAP` + 向所有 crate 注入
`feature(cold_path, atomic_try_update)`）；两个 feature 自 Rust 1.95 已稳定，本仓库无人使用，
依赖图里也没有 nightly 需求（已在 rustc 1.96 上验证）。因此 `clippy --workspace --all-targets -- -D warnings`
成为真正的门禁，不再需要 `-A stable-features`。

仍留在这一批之外：Windows 的进程树回收与验收（现在的 reclaim 依赖 `/proc`，在 Windows 上
只会如实报告“无法识别”），以及真实远端代理出口的验收。

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
an emoji that collapses into the tofu box is reported
the font surface needs an enumeration and the widths
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
a protocol the editor cannot fill in is refused
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

### Capability table vs a real engine

```text
the two unstable switches sit on opposite sides of the pivot
the stable switch set is carried by both generations
the generation is written the same way everywhere
a legacy core reports the exclusions it ignores
a legacy core is not told the noise switch is missing
the legacy generation gets the noise switches but not the exclusions
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
the font exclusion changes what a spoofed platform enumerates, and agrees with
the capability table about whether this build honours it
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
headless window: creating a profile shows one toast in the notification list,
  and showing the queue again does not duplicate it
headless window: a refused setting keeps the banner and is also toasted
headless window: the details panel switches between details, args and log, one
  view at a time, and the log view is this profile's own lines
headless window: the sidebar switches to the Log page and only that page renders
headless window: the newest log line says what happened and at what level, and
  Clear empties the history
headless window: the filter narrows the page and the line is hidden not dropped
headless window: the page names the file it writes to, and the file holds the line
headless window: clicking open-dir hands the profile's data directory to the
  opener, and a failed open is refused with the reason
```

### Activity log and toasts

```text
a success is a toast and leaves the banner clear
a problem owns the banner until it is dismissed
draining toasts leaves nothing to show twice
every notice is written to the log
runtime events are written to the log once each
  (running/stopped/crashed state changes are the events' own lines)
a crash and a refused start are logged as errors
a failed reading is logged as an error
the log is capped and the newest line survives
log rows name the profile and put the newest line first
the log page filter hides lines without losing them
the activity log is written to the file as well
a log file that cannot be written is reported once
the panel log tail is one profile and newest first
the details panel starts on details
```

### Activity log file

```text
a line is appended and readable back
an existing file is appended to rather than replaced
a full file is rotated and the new line survives
a full file is rotated at startup too
a directory that cannot be created is an error
a timestamp is utc and reads the same everywhere
```

### Open data directory

```text
the opener matches the platform
the directory is passed as one argument
a directory that does not exist is refused with its path
a relative directory is named against the working directory
an opener that exits successfully is an open
an opener that fails is reported with what it ran
an opener that keeps running is an open
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
