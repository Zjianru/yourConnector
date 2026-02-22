#!/usr/bin/env bash

# 文件职责：
# 1. 校验业务源码的文件头注释、函数头注释与行长门禁。
# 2. 将治理规则固化为可阻断的 CI/本地检查脚本。
# 3. 仅覆盖业务源码目录，排除 scripts 与 target 生成目录。

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MAX_LEN="${YC_MAX_LINE_LEN:-140}"

cd "${ROOT_DIR}"

python3 - "${ROOT_DIR}" "${MAX_LEN}" <<'PY'
import pathlib
import re
import sys

root = pathlib.Path(sys.argv[1])
max_len = int(sys.argv[2])

source_roots = [
    root / "app/mobile/ui/js",
    root / "app/mobile/src-tauri/src",
    root / "services/relay/src",
    root / "services/sidecar/src",
    root / "protocol/rust/src",
]

file_errors = []
function_errors = []
line_errors = []

js_fn_pattern = re.compile(r"^\s*export\s+(?:async\s+)?function\s+([A-Za-z0-9_]+)")
rs_fn_pattern = re.compile(r"^\s*(?:pub(?:\(crate\))?\s+)?(?:async\s+)?fn\s+([A-Za-z0-9_]+)")


def list_source_files():
    files = []
    for base in source_roots:
        if not base.exists():
            continue
        for path in sorted(base.rglob("*")):
            if not path.is_file():
                continue
            if "target" in path.parts:
                continue
            if path.suffix not in {".js", ".rs"}:
                continue
            files.append(path)
    return files


def first_non_empty_line(lines):
    for raw in lines:
        if raw.strip():
            return raw.strip()
    return ""


def check_header(path, lines):
    first = first_non_empty_line(lines)
    if not first:
        return
    if path.suffix == ".js":
        ok = first.startswith("//") or first.startswith("/**")
    else:
        ok = first.startswith("//") or first.startswith("//!") or first.startswith("///")
    if not ok:
        file_errors.append(f"{path}:1 文件头注释缺失")


def check_line_length(path, lines):
    for idx, line in enumerate(lines, start=1):
        if len(line) > max_len:
            line_errors.append(f"{path}:{idx} 行长 {len(line)} > {max_len}")


def has_jsdoc(lines, index, window=8):
    start = max(0, index - window)
    for j in range(index - 1, start - 1, -1):
        if lines[j].lstrip().startswith("/**"):
            return True
        if lines[j].strip() and not lines[j].lstrip().startswith("*") and not lines[j].lstrip().startswith("//"):
            return False
    return False


def has_rust_doc(lines, index, window=6):
    start = max(0, index - window)
    for j in range(index - 1, start - 1, -1):
        striped = lines[j].strip()
        if striped.startswith("///"):
            return True
        if striped and not striped.startswith("#") and not striped.startswith("//"):
            return False
    return False


def check_js_functions(path, lines):
    for idx, line in enumerate(lines):
        m = js_fn_pattern.match(line)
        if not m:
            continue
        if not has_jsdoc(lines, idx):
            function_errors.append(
                f"{path}:{idx + 1} export function `{m.group(1)}` 缺少 JSDoc"
            )


def check_rust_functions(path, lines):
    depth = 0
    pending_cfg_test = False
    test_module_depth_stack = []

    for idx, line in enumerate(lines):
        stripped = line.strip()

        if stripped.startswith("#[cfg(test)]"):
            pending_cfg_test = True

        if pending_cfg_test and re.match(r"^(?:pub\s+)?mod\s+[A-Za-z0-9_]+\s*\{", stripped):
            test_module_depth_stack.append(depth)
            pending_cfg_test = False
        elif pending_cfg_test and stripped and not stripped.startswith("#"):
            pending_cfg_test = False

        in_test_module = bool(test_module_depth_stack) and depth > test_module_depth_stack[-1]

        fn_match = rs_fn_pattern.match(line)
        if fn_match and not in_test_module:
            fn_name = fn_match.group(1)
            if not has_rust_doc(lines, idx):
                function_errors.append(
                    f"{path}:{idx + 1} rust function `{fn_name}` 缺少 `///` 注释"
                )

        depth += line.count("{")
        depth -= line.count("}")

        while test_module_depth_stack and depth <= test_module_depth_stack[-1]:
            test_module_depth_stack.pop()


def main():
    files = list_source_files()
    if not files:
        print("[check-governance] 未找到业务源码文件")
        return 1

    for path in files:
        text = path.read_text(encoding="utf-8")
        lines = text.splitlines()
        check_header(path, lines)
        check_line_length(path, lines)
        if path.suffix == ".js":
            check_js_functions(path, lines)
        elif path.suffix == ".rs":
            check_rust_functions(path, lines)

    has_error = any([file_errors, function_errors, line_errors])

    if has_error:
        print("[check-governance] 发现治理违规：")
        if file_errors:
            print("\n- 文件头注释问题：")
            for item in file_errors:
                print(f"  {item}")
        if function_errors:
            print("\n- 函数注释问题：")
            for item in function_errors:
                print(f"  {item}")
        if line_errors:
            print("\n- 行长问题：")
            for item in line_errors:
                print(f"  {item}")
        return 1

    print("[check-governance] 通过：文件头/函数注释/行长均符合规则。")
    return 0


sys.exit(main())
PY
