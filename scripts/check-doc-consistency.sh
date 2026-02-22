#!/usr/bin/env bash

# 文件职责：
# 1. 校验文档导航是否收录必需文档。
# 2. 校验 README/docs 内绝对路径引用是否有效。
# 3. 作为治理门禁的一部分，阻断失效文档链接提交。

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DOC_NAV="${ROOT_DIR}/docs/文档导航-v2.md"

required_docs=(
  "${ROOT_DIR}/README.md"
  "${ROOT_DIR}/app/mobile/README.md"
  "${ROOT_DIR}/docs/文档导航-v2.md"
  "${ROOT_DIR}/docs/已完成功能验收-v1.md"
  "${ROOT_DIR}/docs/里程碑与待办-v1.md"
  "${ROOT_DIR}/docs/代码治理与注释规范-v1.md"
  "${ROOT_DIR}/docs/质量门禁与检查规范-v1.md"
  "${ROOT_DIR}/docs/配对与宿主机接入/00-设计总览与决策-v1.md"
  "${ROOT_DIR}/docs/配对与宿主机接入/01-用户流程与页面交互-v1.md"
  "${ROOT_DIR}/docs/配对与宿主机接入/02-协议与安全方案-v1.md"
  "${ROOT_DIR}/docs/配对与宿主机接入/03-实施计划与验收-v1.md"
)

missing_files=()
missing_nav=()
broken_paths=()

for path in "${required_docs[@]}"; do
  if [[ ! -f "${path}" ]]; then
    missing_files+=("${path}")
    continue
  fi
  if [[ -f "${DOC_NAV}" ]] && ! rg -F -q "${path}" "${DOC_NAV}"; then
    missing_nav+=("${path}")
  fi
done

# 从 README/app-mobile/全部 docs 中提取绝对路径引用并校验。
while IFS= read -r raw_path; do
  [[ -z "${raw_path}" ]] && continue

  # 跳过带通配符的路径，按目录前缀校验。
  if [[ "${raw_path}" == *"*"* ]]; then
    prefix="${raw_path%%\**}"
    prefix="${prefix%/}"
    if [[ -z "${prefix}" || ! -d "${prefix}" ]]; then
      broken_paths+=("${raw_path}")
    fi
    continue
  fi

  if [[ ! -e "${raw_path}" ]]; then
    broken_paths+=("${raw_path}")
  fi
done < <(
  rg --no-filename -o '/Users/codez/develop/yourConnector[^ )`"'"'"'\n\r\t]*' \
    "${ROOT_DIR}/README.md" "${ROOT_DIR}/app/mobile/README.md" "${ROOT_DIR}/docs"/*.md "${ROOT_DIR}/docs/配对与宿主机接入"/*.md \
    | sort -u
)

if (( ${#missing_files[@]} > 0 || ${#missing_nav[@]} > 0 || ${#broken_paths[@]} > 0 )); then
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

  if (( ${#broken_paths[@]} > 0 )); then
    echo
    echo "- 无效绝对路径引用："
    for item in "${broken_paths[@]}"; do
      echo "  ${item}"
    done
  fi

  exit 1
fi

echo "[check-doc-consistency] 通过：文档导航与路径引用一致。"
