#!/usr/bin/env bash

# 文件职责：
# 1. 校验文档导航是否收录必需文档。
# 2. 校验 README/docs 是否含本机绝对路径，并检查相对路径引用是否有效。
# 3. 作为治理门禁的一部分，阻断失效文档链接提交。

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DOC_NAV="${ROOT_DIR}/docs/文档导航-v2.md"

required_docs=(
  "README.md"
  "app/mobile/README.md"
  "docs/文档导航-v2.md"
  "docs/已完成功能验收-v1.md"
  "docs/里程碑与待办-v1.md"
  "docs/代码治理与注释规范-v1.md"
  "docs/质量门禁与检查规范-v1.md"
  "docs/系统日志与归档-v1.md"
  "docs/分发安装与卸载-v1.md"
  "docs/跨宿主联调测试-v1.md"
  "docs/工具接入核心组件-v1.md"
  "docs/配对与宿主机接入/00-设计总览与决策-v1.md"
  "docs/配对与宿主机接入/01-用户流程与页面交互-v1.md"
  "docs/配对与宿主机接入/02-协议与安全方案-v1.md"
  "docs/配对与宿主机接入/03-实施计划与验收-v1.md"
)

missing_files=()
missing_nav=()
broken_paths=()
absolute_paths=()

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
  rg --no-filename -o '/Users/[^ )`"'"'"'\n\r\t]*' \
    "${ROOT_DIR}/README.md" "${ROOT_DIR}/app/mobile/README.md" "${ROOT_DIR}/docs"/*.md "${ROOT_DIR}/docs/配对与宿主机接入"/*.md \
    | sort -u
)

# 从 README/app-mobile/全部 docs 中提取相对路径引用并校验。
while IFS= read -r raw_path; do
  [[ -z "${raw_path}" ]] && continue
  path="${raw_path%%[.,，。;；:!?！？)]}"

  # 跳过文档中的占位示例与命令片段，避免误判为真实仓库路径。
  case "${path}" in
    *"<"*|*">"*|*"YYYY-MM-DD"*|*'$('*) continue ;;
  esac

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
  rg --no-filename -o '(README\.md|app/mobile/README\.md|docs/[^ )`"'"'"'\n\r\t]*|services/[^ )`"'"'"'\n\r\t]*|protocol/[^ )`"'"'"'\n\r\t]*|scripts/[^ )`"'"'"'\n\r\t]*|\.github/[^ )`"'"'"'\n\r\t]*|logs/[^ )`"'"'"'\n\r\t]*)' \
    "${ROOT_DIR}/README.md" "${ROOT_DIR}/app/mobile/README.md" "${ROOT_DIR}/docs"/*.md "${ROOT_DIR}/docs/配对与宿主机接入"/*.md \
    | sort -u
)

if (( ${#missing_files[@]} > 0 || ${#missing_nav[@]} > 0 || ${#absolute_paths[@]} > 0 || ${#broken_paths[@]} > 0 )); then
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

  exit 1
fi

echo "[check-doc-consistency] 通过：文档导航与路径引用一致。"
