//! 配对信息高亮输出。

use super::bootstrap_client::PairBootstrapData;

/// 终端高亮样式：重置。
const ANSI_RESET: &str = "\x1b[0m";
/// 终端高亮样式：粗体。
const ANSI_BOLD: &str = "\x1b[1m";
/// 终端高亮样式：青色。
const ANSI_CYAN: &str = "\x1b[36m";
/// 终端高亮样式：亮白。
const ANSI_WHITE: &str = "\x1b[97m";

/// 打印 sidecar 视角的配对区块。
pub(crate) fn print_pairing_banner(data: &PairBootstrapData) {
    println!(
        "{cyan}{bold}\n╔══════════════════════════════════════════════════════════════╗\n\
         ║                    首次配对（宿主机）                   ║\n\
         ╚══════════════════════════════════════════════════════════════╝{reset}",
        cyan = ANSI_CYAN,
        bold = ANSI_BOLD,
        reset = ANSI_RESET
    );

    if let Some(code) = data.pair_code.as_ref() {
        println!(
            "{white}{bold}配对码:{reset} {white}{code}{reset}",
            white = ANSI_WHITE,
            bold = ANSI_BOLD,
            reset = ANSI_RESET,
            code = code
        );
    }
    println!(
        "{white}{bold}Relay WS:{reset} {relay}",
        white = ANSI_WHITE,
        bold = ANSI_BOLD,
        reset = ANSI_RESET,
        relay = data.relay_ws_url
    );
    println!(
        "{white}{bold}systemId:{reset} {sid}",
        white = ANSI_WHITE,
        bold = ANSI_BOLD,
        reset = ANSI_RESET,
        sid = data.system_id
    );
    println!(
        "{white}{bold}短时票据:{reset} {ticket}",
        white = ANSI_WHITE,
        bold = ANSI_BOLD,
        reset = ANSI_RESET,
        ticket = data.pair_ticket
    );
    println!(
        "{white}{bold}宿主机名:{reset} {name}",
        white = ANSI_WHITE,
        bold = ANSI_BOLD,
        reset = ANSI_RESET,
        name = data.host_name
    );
    println!(
        "{white}{bold}配对链接:{reset} {link}",
        white = ANSI_WHITE,
        bold = ANSI_BOLD,
        reset = ANSI_RESET,
        link = data.pair_link
    );
    println!(
        "{white}{bold}提示:{reset} 链接为短时票据，过期后请重新执行 `yc-sidecar pairing show --format link` 获取最新链接。",
        white = ANSI_WHITE,
        bold = ANSI_BOLD,
        reset = ANSI_RESET
    );
    println!(
        "{white}{bold}模拟扫码(iOS):{reset} {cmd}\n",
        white = ANSI_WHITE,
        bold = ANSI_BOLD,
        reset = ANSI_RESET,
        cmd = data.simctl_command
    );
}
