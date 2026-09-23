//! proxies messages.
use super::*;

impl Text {
    pub fn delete_proxy_confirm(&self, name: &str, endpoint: &str) -> String {
        match self.lang {
            Lang::En => format!("\"{name}\" ({endpoint}) will be removed."),
            Lang::Zh => format!("将删除「{name}」（{endpoint}）。"),
        }
    }

    pub fn proxy_in_use(&self, name: &str, used_by: &str) -> String {
        match self.lang {
            Lang::En => format!(
                "\"{name}\" is assigned to {used_by}. Assign those profiles to another proxy or to Direct first."
            ),
            Lang::Zh => {
                format!("「{name}」已分配给 {used_by}。请先把这些档案改派给别的代理，或改为直连。")
            }
        }
    }

    /// A proxy test that is still running.
    pub fn proxy_testing(&self) -> String {
        match self.lang {
            Lang::En => "testing...".to_string(),
            Lang::Zh => "测试中……".to_string(),
        }
    }

    /// How long the engine took to answer, in the engine's own terms.
    ///
    /// Named as the *test's* elapsed time rather than as a latency: it is one
    /// request through a whole path, TLS setup included, and calling it a ping
    /// would be a claim the number does not support.
    pub fn proxy_test_elapsed(&self, millis: u128) -> String {
        match self.lang {
            Lang::En => format!("the test took {millis} ms"),
            Lang::Zh => format!("测试耗时 {millis} 毫秒"),
        }
    }

    /// The accessible name of the control that takes a failed row to the log
    /// line the engine wrote, which is where the fault's own words are.
    pub fn proxy_fault_help(&self, fault: &str) -> String {
        match self.lang {
            Lang::En => format!("{fault}. Open the log for the engine's own account."),
            Lang::Zh => format!("{fault}。打开日志查看引擎自己的说法。"),
        }
    }

    /// The whole of what a proxy dials, for the tooltip on a truncated column.
    ///
    /// Credentials are never part of this sentence because they are never part of
    /// the endpoint: a password in a hover is a password on a shared screen.
    pub fn proxy_endpoint_help(&self, endpoint: &str) -> String {
        match self.lang {
            Lang::En => format!("{endpoint} - credentials are never shown here"),
            Lang::Zh => format!("{endpoint}——凭据不会显示在这里"),
        }
    }

    /// The address a passing test was measured at.
    pub fn proxy_exit(&self, exit_ip: &str) -> String {
        match self.lang {
            Lang::En => format!("exit {exit_ip}"),
            Lang::Zh => format!("出口 {exit_ip}"),
        }
    }

    /// A failing test, named by class. The engine's own words are the log's.
    pub fn proxy_no_traffic(&self, class: &str) -> String {
        match self.lang {
            Lang::En => format!("no traffic ({class})"),
            Lang::Zh => format!("无流量（{class}）"),
        }
    }

    /// How a passing test was taken, for the row's tooltip.
    pub fn proxy_left_from(&self, exit_ip: &str, millis: u128, scope: &str) -> String {
        match self.lang {
            Lang::En => format!("left from {exit_ip} in {millis} ms, {scope}"),
            Lang::Zh => format!("{millis} 毫秒内从 {exit_ip} 出去，{scope}"),
        }
    }

    /// A failing test's full sentence. `fault` is the runtime's own account.
    pub fn proxy_no_traffic_detail(&self, fault: &str) -> String {
        match self.lang {
            Lang::En => format!("no traffic reached the endpoint: {fault}"),
            Lang::Zh => format!("没有流量到达端点：{fault}"),
        }
    }

    /// The chip for a proxy that is not there any more. It is shown rather than
    /// dropped, so a profile does not silently read as having none.
    pub fn missing_proxy(&self, id: &str) -> String {
        match self.lang {
            Lang::En => format!("(missing proxy {id})"),
            Lang::Zh => format!("（代理 {id} 已缺失）"),
        }
    }

    pub fn proxy_created(&self, name: &str) -> String {
        match self.lang {
            Lang::En => format!("Created proxy {name}"),
            Lang::Zh => format!("已创建代理 {name}"),
        }
    }

    pub fn proxy_saved(&self, name: &str) -> String {
        match self.lang {
            Lang::En => {
                format!("Saved {name}. Running profiles keep the proxy they started with.")
            }
            Lang::Zh => format!("已保存{name}。运行中的档案仍使用启动时的代理。"),
        }
    }

    pub fn proxy_deleted(&self, name: &str) -> String {
        match self.lang {
            Lang::En => format!("Deleted proxy {name}"),
            Lang::Zh => format!("已删除代理 {name}"),
        }
    }

    pub fn proxy_test_busy(&self, id: &str) -> String {
        match self.lang {
            Lang::En => format!("proxy {id} is already being tested"),
            Lang::Zh => format!("代理 {id} 正在测试中"),
        }
    }

    /// The identifier of a proxy the caller asked for and did not get.
    pub fn proxy_not_found(&self, id: &str) -> String {
        match self.lang {
            Lang::En => format!("proxy {id}"),
            Lang::Zh => format!("代理 {id}"),
        }
    }

    pub fn proxy_traffic_from(&self, name: &str, exit_ip: &str) -> String {
        match self.lang {
            Lang::En => format!("{name}: traffic leaves from {exit_ip}"),
            Lang::Zh => format!("{name}：流量从 {exit_ip} 出去"),
        }
    }

    pub fn proxy_no_traffic_reached(&self, name: &str, fault: &str) -> String {
        match self.lang {
            Lang::En => format!("{name}: no traffic reached the endpoint. {fault}"),
            Lang::Zh => format!("{name}：没有流量到达端点。{fault}"),
        }
    }

    /// The refusal of a start whose proxy did not answer.
    ///
    /// Named rather than counted, and the profile comes first: the reader pressed
    /// Start on that profile, so the proxy is the reason rather than the subject.
    /// The fault follows, because which stage of the path failed is what says
    /// whether to wait, to fix the proxy, or to fix the endpoint the check asks.
    pub fn proxy_blocks_start(&self, profile: &str, proxy: &str, fault: &str) -> String {
        match self.lang {
            Lang::En => format!(
                "Did not start {profile}: it uses {proxy}, and no traffic got through. {fault}"
            ),
            Lang::Zh => format!("未启动 {profile}：它使用代理 {proxy}，流量没有通过。{fault}"),
        }
    }

    /// The line that says a start waited for its proxy and then went ahead.
    pub fn started_through_proxy(&self, profile: &str, proxy: &str, exit_ip: &str) -> String {
        match self.lang {
            Lang::En => format!("Started {profile} through {proxy}, leaving from {exit_ip}"),
            Lang::Zh => format!("已通过代理 {proxy} 启动 {profile}，出口地址 {exit_ip}"),
        }
    }

    /// Why a start that was waiting for its proxy was called off: the proxy is no
    /// longer the one that was being asked.
    pub fn proxy_check_dropped(&self) -> &'static str {
        match self.lang {
            Lang::En => "the proxy was changed while it was being checked",
            Lang::Zh => "检查代理的过程中代理被修改了",
        }
    }

    /// Why a start that was waiting for its proxy was called off: no answer came.
    pub fn proxy_check_timed_out(&self) -> &'static str {
        match self.lang {
            Lang::En => "the check did not answer in time",
            Lang::Zh => "代理检查超时未返回结果",
        }
    }

    pub fn proxy_test_log(&self, name: &str, detail: &str) -> String {
        match self.lang {
            Lang::En => format!("proxy test: {name} - {detail}"),
            Lang::Zh => format!("代理测试：{name} — {detail}"),
        }
    }

    pub fn egress_confirmed(&self) -> String {
        match self.lang {
            Lang::En => "fingerprint confirmed by reading the running browser".to_string(),
            Lang::Zh => "通过读取运行中的浏览器确认了指纹".to_string(),
        }
    }

    pub fn egress_left_from(&self, exit_ip: &str) -> String {
        match self.lang {
            Lang::En => format!("; traffic left from {exit_ip}"),
            Lang::Zh => format!("；流量从 {exit_ip} 出去"),
        }
    }

    pub fn egress_unreadable(&self, reason: &str) -> String {
        match self.lang {
            Lang::En => format!("; the exit address was not read: {reason}"),
            Lang::Zh => format!("；未能读回出口地址：{reason}"),
        }
    }

    /// The address question, as the details panel reads it back.
    pub fn traffic_left_from(&self, exit_ip: &str) -> String {
        match self.lang {
            Lang::En => format!("traffic left from {exit_ip}"),
            Lang::Zh => format!("流量从 {exit_ip} 出去"),
        }
    }

    pub fn exit_address_not_read(&self, reason: &str) -> String {
        match self.lang {
            Lang::En => format!("exit address not read: {reason}"),
            Lang::Zh => format!("未能读回出口地址：{reason}"),
        }
    }

    /// A proxy test's fault class, in the window's language. The class is a
    /// closed set, which is why it can be translated where the engine's own
    /// words cannot.
    pub fn fault_class(&self, class: FaultClass) -> String {
        match (self.lang, class) {
            (Lang::En, FaultClass::Engine) => "engine".to_string(),
            (Lang::En, FaultClass::Config) => "configuration".to_string(),
            (Lang::En, FaultClass::Auth) => "authentication".to_string(),
            (Lang::En, FaultClass::Dns) => "name resolution".to_string(),
            (Lang::En, FaultClass::Unreachable) => "unreachable".to_string(),
            (Lang::En, FaultClass::Timeout) => "timeout".to_string(),
            (Lang::En, FaultClass::Tls) => "tls".to_string(),
            (Lang::En, FaultClass::Http(code)) => format!("http {code}"),
            (Lang::En, FaultClass::Reading) => "unreadable answer".to_string(),
            (Lang::En, FaultClass::Other) => "unclassified".to_string(),
            (Lang::Zh, FaultClass::Engine) => "引擎".to_string(),
            (Lang::Zh, FaultClass::Config) => "配置".to_string(),
            (Lang::Zh, FaultClass::Auth) => "认证失败".to_string(),
            (Lang::Zh, FaultClass::Dns) => "域名解析".to_string(),
            (Lang::Zh, FaultClass::Unreachable) => "不可达".to_string(),
            (Lang::Zh, FaultClass::Timeout) => "超时".to_string(),
            (Lang::Zh, FaultClass::Tls) => "TLS".to_string(),
            (Lang::Zh, FaultClass::Http(code)) => format!("HTTP {code}"),
            (Lang::Zh, FaultClass::Reading) => "应答无法解析".to_string(),
            (Lang::Zh, FaultClass::Other) => "未分类".to_string(),
        }
    }
}
