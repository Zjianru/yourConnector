#!/usr/bin/env bash

# 文件职责：
# 1. 为 Tauri 生成的 AndroidManifest 自动补齐扫码所需权限。
# 2. 保持幂等，重复执行不会产生重复条目。

set -euo pipefail

MANIFEST_PATH="${1:-app/mobile/src-tauri/gen/android/app/src/main/AndroidManifest.xml}"
APP_BUILD_GRADLE_PATH="${2:-app/mobile/src-tauri/gen/android/app/build.gradle.kts}"

if [[ ! -f "${MANIFEST_PATH}" ]]; then
  echo "[android-manifest] skip: not found: ${MANIFEST_PATH}"
  exit 0
fi

ensure_cleartext_traffic_enabled() {
  if [[ ! -f "${APP_BUILD_GRADLE_PATH}" ]]; then
    echo "[android-manifest] skip cleartext patch: not found: ${APP_BUILD_GRADLE_PATH}"
    return 0
  fi

  if ! grep -Fq 'manifestPlaceholders["usesCleartextTraffic"] = "false"' "${APP_BUILD_GRADLE_PATH}"; then
    return 0
  fi

  local tmp
  tmp="$(mktemp)"
  sed 's/manifestPlaceholders\["usesCleartextTraffic"\] = "false"/manifestPlaceholders["usesCleartextTraffic"] = "true"/g' \
    "${APP_BUILD_GRADLE_PATH}" > "${tmp}"
  mv "${tmp}" "${APP_BUILD_GRADLE_PATH}"
}

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

insert_pairing_intent_filter_once() {
  if grep -Fq 'android:scheme="yc"' "${MANIFEST_PATH}"; then
    return 0
  fi

  local tmp
  tmp="$(mktemp)"
  awk '
    {
      print $0
      if (!inserted && index($0, "</intent-filter>") > 0) {
        print "            <intent-filter>"
        print "                <action android:name=\"android.intent.action.VIEW\" />"
        print "                <category android:name=\"android.intent.category.DEFAULT\" />"
        print "                <category android:name=\"android.intent.category.BROWSABLE\" />"
        print "                <data android:scheme=\"yc\" android:host=\"pair\" />"
        print "            </intent-filter>"
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
insert_pairing_intent_filter_once
ensure_cleartext_traffic_enabled

echo "[android-manifest] ensured camera permissions, yc://pair deep link, and cleartext relay traffic: ${MANIFEST_PATH}"
