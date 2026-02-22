// 文件职责：
// 1. 统一数值/Token/密钥脱敏展示格式。
// 2. 将展示层格式规则集中，避免页面各处格式不一致。

export function fmt2(value) {
  return Number.isFinite(Number(value)) ? Number(value).toFixed(2) : "--";
}

export function fmtInt(value) {
  return Number.isFinite(Number(value)) ? String(Math.trunc(Number(value))) : "--";
}

export function fmtTokenM(value) {
  const raw = Number(value);
  if (!Number.isFinite(raw)) {
    return "--";
  }
  const million = raw / 1_000_000;
  const abs = Math.abs(million);
  let decimals = 2;
  if (abs >= 100) {
    decimals = 0;
  } else if (abs >= 10) {
    decimals = 1;
  } else if (abs >= 1) {
    decimals = 2;
  } else if (abs >= 0.1) {
    decimals = 3;
  } else if (abs >= 0.01) {
    decimals = 4;
  } else if (abs >= 0.001) {
    decimals = 5;
  } else {
    decimals = 6;
  }
  let formatted = million.toFixed(decimals).replace(/\.?0+$/, "");
  if (formatted === "-0") {
    formatted = "0";
  }
  return `${formatted}M`;
}

export function maskSecret(value) {
  const raw = String(value || "");
  if (!raw) {
    return "--";
  }
  if (raw.length <= 8) {
    return "****";
  }
  return `${raw.slice(0, 4)}****${raw.slice(-4)}`;
}

export function usageSummary(usage) {
  if (!usage.length) {
    return "--";
  }
  return usage
    .slice(0, 2)
    .map((row) => {
      const model = String(row.model || "--");
      const total = fmtTokenM(row.tokenTotal);
      const count = fmtInt(row.messages);
      return `${model}（总Token ${total}，消息 ${count} 条）`;
    })
    .join(" | ");
}
