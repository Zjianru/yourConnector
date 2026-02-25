#!/usr/bin/env bash

# 文件职责：
# 1. 为 Tauri 生成的 AndroidManifest 自动补齐扫码所需权限。
# 2. 保持幂等，重复执行不会产生重复条目。

set -euo pipefail

MANIFEST_PATH="${1:-app/mobile/src-tauri/gen/android/app/src/main/AndroidManifest.xml}"

if [[ ! -f "${MANIFEST_PATH}" ]]; then
  echo "[android-manifest] skip: not found: ${MANIFEST_PATH}"
  exit 0
fi

insert_after_once() {
  local anchor="$1"
  local line="$2"

  if grep -Fq "${line}" "${MANIFEST_PATH}"; then
    return 0
  fi

  local tmp
  tmp="$(mktemp)"
  awk -v anchor="${anchor}" -v line="${line}" '
    {
      print $0
      if (!inserted && index($0, anchor) > 0) {
        print line
        inserted = 1
      }
    }
  ' "${MANIFEST_PATH}" > "${tmp}"
  mv "${tmp}" "${MANIFEST_PATH}"
}

insert_after_once '<uses-permission android:name="android.permission.INTERNET" />' '    <uses-permission android:name="android.permission.CAMERA" />'
insert_after_once '<uses-permission android:name="android.permission.CAMERA" />' '    <uses-permission android:name="android.permission.RECORD_AUDIO" />'
insert_after_once '<uses-permission android:name="android.permission.RECORD_AUDIO" />' '    <uses-permission android:name="android.permission.MODIFY_AUDIO_SETTINGS" />'

insert_after_once '<uses-feature android:name="android.software.leanback" android:required="false" />' '    <uses-feature android:name="android.hardware.camera" android:required="false" />'
insert_after_once '<uses-feature android:name="android.hardware.camera" android:required="false" />' '    <uses-feature android:name="android.hardware.camera.autofocus" android:required="false" />'

echo "[android-manifest] ensured camera permissions: ${MANIFEST_PATH}"
