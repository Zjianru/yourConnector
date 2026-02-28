// 文件职责：
// 1. 解析聊天消息中的目录启动提案（launch proposal）。
// 2. 生成“引用确认启动”文案，并解析确认指令。

function canonicalToolName(raw) {
  const text = String(raw || "").trim().toLowerCase().replace(/[\s_]+/g, "-");
  if (!text) return "";
  if (text.includes("openclaw")) return "openclaw";
  if (text.includes("opencode") || text === "open-code") return "opencode";
  if (text.includes("codex")) return "codex";
  if (text.includes("claude")) return "claude-code";
  return "";
}

function normalizeCwd(raw) {
  const text = String(raw || "").trim();
  if (!text) return "";
  return text.replace(/^['"]|['"]$/g, "");
}

function parseProposalObject(raw) {
  if (!raw || typeof raw !== "object") return null;
  const toolName = canonicalToolName(raw.toolName || raw.tool || raw.targetTool || raw.provider);
  const cwd = normalizeCwd(raw.cwd || raw.path || raw.directory || raw.dir);
  if (!toolName || !cwd) return null;
  return {
    toolName,
    cwd,
  };
}

function parseProposalBlock(block) {
  const text = String(block || "").trim();
  if (!text) return null;
  try {
    const parsed = JSON.parse(text);
    const proposal = parseProposalObject(parsed);
    if (proposal) return proposal;
  } catch (_) {
    // fallback to key-value format
  }

  const rows = {};
  text.split(/\r?\n/).forEach((line) => {
    const match = String(line || "").match(/^\s*([A-Za-z][A-Za-z0-9_]*)\s*[:=]\s*(.+)$/);
    if (!match) return;
    rows[match[1]] = String(match[2] || "").trim();
  });
  return parseProposalObject(rows);
}

export function parseLaunchProposalsFromText(rawText) {
  const text = String(rawText || "");
  if (!text) return [];
  const proposals = [];
  const blocks = text.matchAll(/```(?:yc-launch|launch|json)?\s*([\s\S]*?)```/gi);
  for (const match of blocks) {
    const proposal = parseProposalBlock(match[1]);
    if (proposal) proposals.push(proposal);
  }

  const markerMatches = text.matchAll(/#launch-proposal\s+(\{[^\n]+\})/gi);
  for (const match of markerMatches) {
    const proposal = parseProposalBlock(match[1]);
    if (proposal) proposals.push(proposal);
  }

  const seen = new Set();
  return proposals.filter((item) => {
    const key = `${item.toolName}::${item.cwd}`;
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

export function buildLaunchConfirmDraft(proposal) {
  const toolName = canonicalToolName(proposal?.toolName || proposal?.tool || "");
  const cwd = normalizeCwd(proposal?.cwd || "");
  if (!toolName || !cwd) return "";
  const payload = JSON.stringify({ toolName, cwd });
  return `> 启动提案：${toolName} @ ${cwd}\n#launch-confirm ${payload}`;
}

export function parseLaunchConfirmFromText(rawText) {
  const text = String(rawText || "");
  if (!text) return null;
  const lines = text.split(/\r?\n/);
  for (const line of lines) {
    const trimmed = String(line || "").trim();
    if (!trimmed.toLowerCase().startsWith("#launch-confirm")) continue;
    const payload = trimmed.slice("#launch-confirm".length).trim();
    if (!payload) continue;
    try {
      const obj = JSON.parse(payload);
      const parsed = parseProposalObject(obj);
      if (parsed) return parsed;
    } catch (_) {
      // ignore malformed line
    }
  }
  return null;
}
