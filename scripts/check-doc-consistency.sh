#!/usr/bin/env bash

# 文件职责：
# 1. 校验文档导航是否收录必需文档。
# 2. 校验 README/docs 是否含本机绝对路径，并检查相对路径引用是否有效。
# 3. 作为治理门禁的一部分，阻断失效文档链接提交。

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DOC_NAV="${ROOT_DIR}/docs/文档导航.md"

required_docs=(
  "README.md"
  "CONTRIBUTING.md"
  "app/mobile/README.md"
  "docs/文档导航.md"
  "docs/代码事实总索引.md"
  "docs/架构与数据流.md"
  "docs/API与事件协议.md"
  "docs/CLI与环境变量.md"
  "docs/开发与调试指南.md"
  "docs/已完成功能验收.md"
  "docs/里程碑与待办.md"
  "docs/代码治理与注释规范.md"
  "docs/质量门禁与检查规范.md"
  "docs/系统日志与归档.md"
  "docs/分发安装与卸载.md"
  "docs/跨宿主联调测试.md"
  "docs/配对与宿主机接入/00-设计总览与决策.md"
  "docs/配对与宿主机接入/01-用户流程与页面交互.md"
  "docs/配对与宿主机接入/02-协议与安全方案.md"
  "docs/配对与宿主机接入/03-实施计划与验收.md"
  "docs/运维与工具管理/00-能力总览与代码边界.md"
  "docs/运维与工具管理/01-页面交互与状态机.md"
  "docs/运维与工具管理/02-协议事件与失败处理.md"
  "docs/运维与工具管理/03-代码映射与验收口径.md"
  "docs/聊天与报告/00-能力总览与代码边界.md"
  "docs/聊天与报告/01-页面交互与会话模型.md"
  "docs/聊天与报告/02-事件协议与执行链路.md"
  "docs/聊天与报告/03-代码映射与验收口径.md"
  "docs/工具详情与数据采集/00-能力总览与代码边界.md"
  "docs/工具详情与数据采集/01-openclaw.v1-详情模型.md"
  "docs/工具详情与数据采集/02-opencode.v1-详情模型.md"
  "docs/工具详情与数据采集/03-采集调度与降级策略.md"
)

missing_files=()
missing_nav=()
broken_paths=()
absolute_paths=()
uncovered_code_paths=()
doc_targets=("${ROOT_DIR}/README.md" "${ROOT_DIR}/CONTRIBUTING.md" "${ROOT_DIR}/app/mobile/README.md")
CODE_INDEX_DOC="${ROOT_DIR}/docs/代码事实总索引.md"

emit_code_coverage_targets() {
  cd "${ROOT_DIR}"

  echo "Makefile"

  [[ -d ".github/workflows" ]] && rg --files .github/workflows
  [[ -d "app/mobile/src-tauri/src" ]] && rg --files app/mobile/src-tauri/src
  [[ -d "app/mobile/ui/js" ]] && rg --files app/mobile/ui/js | rg -v '/vendor/'
  [[ -d "services/relay/src" ]] && rg --files services/relay/src
  [[ -d "services/sidecar/src" ]] && rg --files services/sidecar/src
  [[ -d "protocol/rust/src" ]] && rg --files protocol/rust/src
  [[ -d "scripts/dist" ]] && rg --files scripts/dist

  [[ -f "scripts/pairing.sh" ]] && echo "scripts/pairing.sh"
  [[ -f "scripts/check-governance.sh" ]] && echo "scripts/check-governance.sh"
  [[ -f "scripts/check-doc-consistency.sh" ]] && echo "scripts/check-doc-consistency.sh"
  [[ -f "scripts/self-debug-loop.sh" ]] && echo "scripts/self-debug-loop.sh"
}

while IFS= read -r file; do
  doc_targets+=("${file}")
done < <(find "${ROOT_DIR}/docs" -type f -name '*.md' | sort)

for rel in "${required_docs[@]}"; do
  abs="${ROOT_DIR}/${rel}"
  if [[ ! -f "${abs}" ]]; then
    missing_files+=("${rel}")
    continue
  fi
  if [[ -f "${DOC_NAV}" ]] && ! rg -F -q "${rel}" "${DOC_NAV}"; then
    missing_nav+=("${rel}")
  fi
done

# 禁止文档出现本机绝对路径。
while IFS= read -r raw_path; do
  [[ -z "${raw_path}" ]] || absolute_paths+=("${raw_path}")
done < <(
  rg --no-filename -o '/Users/[^ )`"\n\r\t]*' \
    "${doc_targets[@]}" \
    | sort -u
)

# 从 README/app-mobile/全部 docs 中提取相对路径引用并校验。
while IFS= read -r raw_path; do
  [[ -z "${raw_path}" ]] && continue
  path="${raw_path%%[.,，。;；:!?！？)\`]}"

  # 跳过文档中的占位示例与命令片段，避免误判为真实仓库路径。
  case "${path}" in
    *"<"*|*">"*|*"YYYY-MM-DD"*|*'$('* ) continue ;;
  esac

  # 兼容 app/mobile/ui/js/services/* 路径，避免被正则截断为根目录 services/* 误报。
  if [[ "${path}" == services/* && ! -e "${ROOT_DIR}/${path}" && -e "${ROOT_DIR}/app/mobile/ui/js/${path}" ]]; then
    continue
  fi

  # 跳过带通配符的路径，按目录前缀校验。
  if [[ "${path}" == *"*"* ]]; then
    prefix="${path%%\**}"
    prefix="${prefix%/}"
    if [[ -z "${prefix}" || ! -d "${ROOT_DIR}/${prefix}" ]]; then
      broken_paths+=("${path}")
    fi
    continue
  fi

  if [[ ! -e "${ROOT_DIR}/${path}" ]]; then
    broken_paths+=("${path}")
  fi
done < <(
  rg --no-filename -o '(README\.md|CONTRIBUTING\.md|app/mobile/README\.md|docs/[^ )`"\n\r\t]+|services/[^ )`"\n\r\t]+|protocol/[^ )`"\n\r\t]+|scripts/[^ )`"\n\r\t]+|\.github/[^ )`"\n\r\t]+|logs/[^ )`"\n\r\t]+)' \
    "${doc_targets[@]}" \
    | sort -u
)

if [[ -f "${CODE_INDEX_DOC}" ]]; then
  while IFS= read -r rel_path; do
    [[ -z "${rel_path}" ]] && continue
    if ! rg -F -q "${rel_path}" "${CODE_INDEX_DOC}"; then
      uncovered_code_paths+=("${rel_path}")
    fi
  done < <(emit_code_coverage_targets | sort -u)
fi

if (( ${#missing_files[@]} > 0 || ${#missing_nav[@]} > 0 || ${#absolute_paths[@]} > 0 || ${#broken_paths[@]} > 0 || ${#uncovered_code_paths[@]} > 0 )); then
  echo "[check-doc-consistency] 发现文档一致性问题："

  if (( ${#missing_files[@]} > 0 )); then
    echo
    echo "- 缺失必需文档："
    for item in "${missing_files[@]}"; do
      echo "  ${item}"
    done
  fi

  if (( ${#missing_nav[@]} > 0 )); then
    echo
    echo "- 文档导航未收录："
    for item in "${missing_nav[@]}"; do
      echo "  ${item}"
    done
  fi

  if (( ${#absolute_paths[@]} > 0 )); then
    echo
    echo "- 存在本机绝对路径（应改为仓库相对路径）："
    for item in "${absolute_paths[@]}"; do
      echo "  ${item}"
    done
  fi

  if (( ${#broken_paths[@]} > 0 )); then
    echo
    echo "- 无效相对路径引用："
    for item in "${broken_paths[@]}"; do
      echo "  ${item}"
    done
  fi

  if (( ${#uncovered_code_paths[@]} > 0 )); then
    echo
    echo "- 代码事实总索引未覆盖的关键源码路径："
    for item in "${uncovered_code_paths[@]}"; do
      echo "  ${item}"
    done
  fi

  exit 1
fi

echo "[check-doc-consistency] 通过：文档导航与路径引用一致。"
