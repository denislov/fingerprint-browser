# Crate 代码质量审阅与优化方案

审阅日期：2026-09-22。范围：workspace 中的 domain、storage、runtime、application、app；重点沿配置输入、持久化、启动/停止、备份/恢复及 UI 后台任务调用链检查。未修改业务实现。

总体判断：已有清晰分层、较丰富的行为测试和认真设计的进程生命周期机制。主要短板不是格式或语法，而是破坏性操作的原子性、异步任务的互斥，以及跨层不变量没有统一执行。建议先修数据安全和生命周期正确性，再拆大模块；不建议立即增加 crate 或全面更换运行时。

## 验证范围与边界

- Windows 本机 `cargo test --workspace --locked` 通过。
- `cargo fmt --all --check` 通过。
- `cargo clippy --workspace --all-targets --locked -- -D warnings` 通过。
- 未运行被 ignored 的真实 Chromium/Xray 验收，也未执行 Linux 专属测试；本次通过不代表跨平台和真实引擎全部通过。
- 在 target 下使用临时验证程序调用实际库实现，使用独立测试目录和内存 SQLite。复现结果：`nested_restore_error=true source_deleted=true`；`switch_as_url_passes_validation=true`；`restore_failures=1 original_cores_remaining=0 original_profiles_remaining=0`。
- 第三个实验直接调用公开的 `apply_restore`，传入包含空 core 名称的计划；源代码核对表明 `plan_restore` 也不会提前完成此类领域校验。因此破坏性写入前的校验不足，不仅是实验绕过入口造成的。
- 临时验证未启动真实浏览器，未对用户数据执行操作。启动参数问题验证到领域校验和参数构造层，未实测 Chromium 的最终行为。
- 以下行数包含注释和测试，不等同于生产逻辑复杂度，也不作为测试覆盖率。

## 每个 crate 的评价

| crate | Rust 文件 / 总行数 | 评价 | 优点 | 主要优化点 |
|---|---:|---|---|---|
| domain | 13 / 3,346 | 结构较好，输入约束不足 | 类型化 ID、协议枚举、集中校验、凭据移除逻辑；URI 解析有多种拒绝分支 | StartTarget 使用任意 String；profile 校验遗漏启动目标和数据目录约束；协议 DTO 与可启动对象尚未明确区分 |
| storage | 6 / 1,130 | 简单可理解，契约需加强 | 参数化 SQL、外键开启、迁移使用事务、连接集中管理 | 缺跨仓库事务接口；约束错误误分类；get/list 解码重复；内存仓库不模拟外键；迁移读取错误被吞掉 |
| runtime | 25 / 13,818 | 工程设计较强，异常清理仍有漏洞 | supervisor 独占进程、Windows 原子 Job 分配、PID 身份核对、启动取消、快照对账、网络超时和响应限长 | 接管进程停止失败仍清除记录；启动/回滚分支集中；阻塞命令提交和停止等待需要明确上界；核心状态机测试偏 Unix |
| application | 11 / 4,337 | 服务边界基本合理，恢复流程风险最高 | repository/runtime 通过接口注入；导入有计划与执行分离；版本检测可替换 | 恢复未使用事务；目录复制先删除后复制；路径重叠未拒绝；RemoveUserData 缺路径归属与运行状态保护 |
| app | 27 / 24,310 | 功能与测试丰富，职责集中 | AppState 不依赖 GPUI；耗时复制/诊断已有 worker；通知、日志上限和快照恢复机制 | 后台复制没有任务占用状态；ui/state 过大；部分磁盘和版本探测仍在 UI 回调中同步执行；业务限制过多依赖界面 |

`ui.rs` 6,951 行，其中测试模块从 3,908 行开始；`state.rs` 5,480 行，其中测试模块从 2,656 行开始。两者确实应拆，但不能将全部行数称作生产逻辑。`text.rs` 主要是本地化文案，不应仅因行数多而高优先级重构。

## 优先修复的问题

### 1. P1：源/目标目录重叠会删除备份或递归复制

位置：`crates/application/src/browser_data.rs:132`、`:142`、`:167`、`:188`。

`same_directory` 只比较相等。若从当前 profile 数据目录内部的备份恢复，目标是源的祖先，`remove_dir_all(to)` 会先删除备份源，再因源不存在返回错误。本次隔离运行已复现。反过来，备份目标位于源内部时，新建的目标会进入递归遍历，可能持续复制直到路径或磁盘限制。

修复：先对所有 profile 的源/目标进行统一预检；已有路径解析真实路径，未创建的目标解析其已有父目录并规范化剩余部分；拒绝相等、双向祖先关系，以及不同 profile 之间的交叉覆盖。明确 Windows junction/reparse point 和符号链接策略。

验收：相同目录、父目录、子目录、通过链接形成的重叠、目标尚不存在、跨 profile 重叠均在首次写入前失败；原数据和备份均保持不变。

### 2. P1：浏览器数据恢复失败时无法保住旧数据

位置：`crates/application/src/browser_data.rs:142`。

即使路径完全独立，当前实现也先删除目标，再复制源。源文件读取失败、空间不足或程序退出都会使原数据丢失，只留下不完整目录。复制报告不能补偿已经删除的 cookies/session。

修复：复制到目标同级的唯一临时目录，完整成功后才切换；保留旧目录直至新目录接管成功。Windows 的非空目录替换需要“旧目录改名 → 新目录改名 → 清理旧目录”的可恢复协议及操作日志，不能笼统假设单次 rename 能解决全部情况。失败返回部分执行进度。

验收：在第 N 个文件注入复制错误、模拟空间不足/退出，旧目录仍可恢复；重启能识别未完成切换。

### 3. P1：配置恢复先清空原配置，失败后不回滚

位置：`crates/application/src/restore.rs:100`、`:127`。

`apply_restore` 逐条删除所有 profile/proxy/core 后才调用 `apply_import`；`plan_restore` 不会预先完成所有领域校验。无效名称等可解析但无效的记录，到插入阶段才拒绝。已复现返回失败报告时原 core/profile 均已不存在。磁盘或数据库写入错误同样会留下中间状态。

修复：restore 使用独立的严格语义：先全量校验、检查重复 ID 和引用，再在一个数据库事务中替换。任一错误回滚。普通 import 的尽力导入可以保留，但不能将其部分成功语义直接套用于覆盖恢复。凭据省略的备份应在预检阶段明确其可恢复程度。

验收：无效记录、缺失引用、重复 ID、第 N 次写入失败均不改变旧配置；成功恢复一次性提交。

### 4. P1：复制任务期间仍可启动浏览器或重复发起复制

位置：`crates/app/src/ui.rs:1395`；`crates/app/src/state.rs:1689`、`:1890`、`:1904`。

界面每次点击都创建新线程，只将任务创建时的 running 集合传给 worker。没有持久的复制占用状态；start/restart 也未检查复制任务。因此复制过程中可以启动同一 profile，或再次执行恢复，造成源被浏览器写入、两个 worker 相互删除目标。启动命令已入队但尚未反映到快照时，还存在额外窗口。

修复：由应用用例层统一持有按 ProfileId 管理的操作租约，覆盖已排队的启动和复制/恢复全过程；UI 只展示忙碌和进度。worker 完成、失败或异常退出都释放租约。禁止相同目标的并行恢复；退出时明确等待或取消策略。

验收：使用可暂停的 fake worker，验证第二次恢复、start/restart、配置替换在占用期间被拒绝，完成/失败后可继续。

### 5. P1：启动目标可以作为浏览器开关进入 argv

位置：`crates/domain/src/validation.rs:8`；`crates/runtime/src/planner.rs:85`。

`StartTarget::Url(String)` 可以保存 `--no-proxy-server` 等内容，`validate_profile` 不校验它，planner 又将其原样追加为独立命令行参数。导入数据由此可进入浏览器开关位置。本次已验证该值通过校验；其绕过代理的最终效果未用真实浏览器测试。

修复：定义经校验的启动目标，至少拒绝开关形式、NUL 和无效 URL；按产品允许的 scheme 做校验，明确处理 about:blank 和本地文件。反序列化、服务入口和最终 planner 均确保约束成立。不要仅在 UI 表单校验，也不要把依赖于浏览器解析规则的参数分隔符当作唯一保护。

验收：JSON 导入、直接服务调用、直接 planner 调用都拒绝开关形式目标；正常 http/https 及明确支持的其它目标不回归。

### 6. P1：接管会话停止失败仍丢弃恢复记录

位置：`crates/runtime/src/supervisor.rs:102`、`:924`、`:959`。

`Held::Adopted` 调用 `journal::stop`，但失败只写日志，`kill` 不返回结果。`stop_session` 随后无条件移除 Xray 配置和 session journal，并发布 Stopped。若进程不可识别、权限不足或未能退出，实际仍运行，却失去跟踪和重试依据。这与启动恢复路径中保留 unresolved journal 的策略不一致。

修复：停止返回结构化结果，区分已退出、身份不符、无法读取和终止失败；只清理已确认结束的会话。失败保留记录/配置和可重试状态；同步梳理 owned child 的错误处理及等待上界。此结论来自调用链静态核对，未在本机制造进程权限故障。

验收：用假 inspector/tree 注入读取失败、拒绝终止和超时，确认不会发布虚假的 Stopped，记录保留且可重试；正常停止仍清理全部资源。

### 7. P2：SQLite 将所有约束失败误报为重复 ID

位置：`crates/storage/src/sqlite.rs:258`。

profile insert 捕获统一 ConstraintViolation 后固定返回“id already exists”。实际上缺失 core/proxy 的外键错误也走这里。服务创建接口不校验所有引用，因而此路径可达；错误文案会误导排障和调用方分支。

修复：按 SQLite extended_code 区分主键/唯一、外键、NOT NULL/CHECK，统一映射到明确的 StorageError；不要让调用方解析英文字符串。

验收：重复 ID、缺失 core、缺失 proxy 和普通数据库失败得到不同且准确的结果；内存实现与 SQLite 的共享契约保持一致。

## 其余结构与性能优化

### domain

- 保留纯模型与规则的边界。先解决启动目标这一真实缺口，再考虑将原始反序列化 DTO 转成 ValidatedProfile/ValidatedProxy；无需一次性将所有 String 包成新类型。
- 对含凭据类型实现脱敏 Debug。目前 ProxyProfile/ProxyOutbound 派生 Debug 会包含凭据，这是未来误记录风险，本次没有确认生产日志泄漏。
- URI parser 增加性质测试或 fuzz：不 panic、超长输入有界、错误信息不包含凭据。与真实 Xray 是否接受某配置分开验证。

### storage

- 增加配置级事务能力，优先服务严格 restore。保留现有 repository 接口，避免为了事务重写所有 CRUD。
- `migrations.rs:70` 的 `unwrap_or(0)` 应传播查询失败；明确拒绝高于当前支持版本的 schema，增加升级/失败回滚测试。
- 将 `get/list` 共享的行解码提取成每实体一个函数；列表排序补稳定的 ID 次序，避免同秒 created_at 顺序不稳定。
- 数据库 `ON DELETE SET NULL` 与“引用中的代理不能删除”的服务规则不一致。普通 UI 路径已有使用检查，不是当前每次删除都会直连；仍建议将外键改为 RESTRICT，使并发调用和未来入口也无法悄悄解除代理。
- 内存仓库缺少 SQLite 外键约束，不能只用内存测试证明引用一致性。增加两种实现共用的 repository 契约测试及专属迁移测试。
- 暂不增加连接池。当前单连接互斥符合小型本地应用规模；先测 UI 阻塞和事务时间，再决定是否需要 WAL、分页或缓存。

### runtime

- 将 supervisor 中的单次启动资源集中成 RAII guard，统一端口、子进程、临时配置与 journal 的回滚，成功时转移给 ActiveSession。状态转换与平台 I/O 分开测试。
- command facade 对有界 channel 使用阻塞 send，而 UI 同步调用它。补充队列满时的明确错误/超时策略和退出优先级，避免压力下窗口等待；事件通道已有 try_send，不应退化为阻塞。
- 复用 `journal::stop` 和 supervisor 的停止结果契约，避免两套清理规则分叉。
- 把可通过 fake components 测试的状态机从 Unix-only 模块拆出，Windows 同样跑；实际 Job/process-group 行为继续保留平台测试。

### application

- `profile_service.rs:118` 的 RemoveUserData 接受任意记录目录，先删记录再删目录，失败仅 warn。当前 UI 使用 KeepUserData，因此属于公开服务的潜在危险能力。若没有需求，移除此模式；若保留，要求目录归属证明、运行状态租约，并返回可重试的部分失败。
- 合并 RuntimeService 的 start/restart 参数装载逻辑；把引用解析放入一个可测试函数。
- Core/Proxy 的 `insert` 实际委托 upsert。明确 insert/update/upsert 契约，避免未来绕过 plan 的调用覆盖既有记录。
- seed 当前取时钟纳秒低 32 位，应改为随机源并注入生成器；不承诺 32 位种子绝不重复。

### app

- 按功能拆成 profiles、cores、proxies、backup、settings、runtime_details 的 view/controller；共享 AppState 保留轻量协调，业务互斥下沉 application。
- 把 ui/state 的测试按功能移出主文件，保留现有行为测试，避免先调整视觉或行为导致重构难评审。
- `on_redetect_core` 同步进入版本探测；配置 import/restore/export 也有同步磁盘/数据库工作。迁到受控 worker，返回 task ID 和结构化结果，不再每个功能独立裸 spawn。
- 只在数据或运行态版本变化时重算列表。以 100/1,000/10,000 个 profile 测量筛选、快照刷新、内存和帧延迟，获得数据后决定索引、虚拟列表和缓存；本次没有性能基准，不能断言已经存在规模瓶颈。

## 推荐实施顺序与验收

| 阶段 | 交付 | 主要 crate | 验收重点 |
|---|---|---|---|
| A：数据与输入安全 | 路径统一预检；目录暂存切换；启动目标验证；复制操作租约 | domain/application/app/runtime | 错误路径无写入；复制故障可恢复；并发启动/恢复被拒绝；URL 不能进入开关位置 |
| B：状态与事务正确性 | restore 单事务；停止失败保留 journal；错误分类与代理引用约束 | storage/application/runtime | 写入故障完整回滚；停止失败不假报成功；引用不被静默解除 |
| C：渐进重构 | 启动资源 guard；共享停止契约；按功能拆 ui/state；统一 worker 结果协议 | runtime/app/application | 现有行为测试不回归；跨平台状态机测试覆盖；UI 无同步长探测 |
| D：性能与工程门禁 | 规模基准；故障矩阵；URI fuzz；明确最低 Rust 版本与工具链 | 全 workspace | Windows/Linux gate 均通过；真实引擎验收独立执行并记录环境；按基准决定优化 |

各阶段拆小 PR：先加入能失败的行为回归测试，再修对应问题；正确性修复与模块搬迁分开提交。对 schema 变化写迁移，不直接修改初始迁移冒充兼容升级。

完成标准应优先衡量“失败时是否保住数据、运行态是否真实、边界能否被绕过”。文件变短、测试数量上升或 Clippy 零警告都不能替代这些标准。
