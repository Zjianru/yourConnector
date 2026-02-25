SHELL := /bin/zsh

IOS_SIM ?= iPhone 17 Pro
ANDROID_TARGETS ?= aarch64
ANDROID_DEVICE ?=
ANDROID_MANIFEST_PATCH ?= ./scripts/mobile/ensure-android-camera-permissions.sh
ANDROID_APK_SIGN_SCRIPT ?= ./scripts/mobile/sign-android-apk.sh
ANDROID_SECURE_STORE_SRC ?= app/mobile/src-tauri/android/SecureStoreBridge.kt
ANDROID_SECURE_STORE_DST ?= app/mobile/src-tauri/gen/android/app/src/main/java/dev/yourconnector/mobile/SecureStoreBridge.kt
ANDROID_UNSIGNED_APK_PATH ?= app/mobile/src-tauri/gen/android/app/build/outputs/apk/universal/release/app-universal-release-unsigned.apk
ANDROID_SIGNED_APK_PATH ?= app/mobile/src-tauri/gen/android/app/build/outputs/apk/universal/release/app-universal-release-signed.apk
ANDROID_KEYSTORE_PATH ?=
ANDROID_KEY_ALIAS ?=
ANDROID_KEYSTORE_PASSWORD ?=
ANDROID_KEY_PASSWORD ?=
PAIR_DIR ?= $(HOME)/.config/yourconnector/sidecar
PAIR_RELAY_WS ?= ws://127.0.0.1:18080/v1/ws
PAIR_NAME ?=
PAIR_ARGS ?=

.DEFAULT_GOAL := help

.PHONY: check check-governance check-all
.PHONY: run-relay run-sidecar stop-relay stop-sidecar restart-relay restart-sidecar
.PHONY: install-tauri-cli boot-ios-sim stop-mobile-tauri-ios repair-ios-sim
.PHONY: run-mobile-tauri-ios run-mobile-tauri-ios-dev run-mobile-tauri-ios-dev-clean
.PHONY: sync-mobile-tauri-android-secure-store ensure-mobile-tauri-android
.PHONY: init-mobile-tauri-android run-mobile-tauri-android-dev
.PHONY: build-mobile-tauri-android-apk build-mobile-tauri-android-aab
.PHONY: sign-mobile-tauri-android-apk build-mobile-tauri-android-apk-signed build-mobile-tauri-android-apk-test
.PHONY: pairing show-pairing show-pairing-code show-pairing-link show-pairing-qr show-pairing-json simulate-ios-scan
.PHONY: self-debug-loop
.PHONY: help

help:
	@echo "yourConnector 常用命令："
	@echo "  make check                          # 工作区编译检查"
	@echo "  make check-governance               # 代码注释/行长/文档一致性门禁"
	@echo "  make check-all                      # 全量门禁（编译+测试+lint+治理）"
	@echo "  make run-relay                      # 启动 relay"
	@echo "  make run-sidecar                    # 启动 sidecar"
	@echo "  make run-mobile-tauri-ios           # 构建并安装 iOS App（推荐）"
	@echo "  make run-mobile-tauri-ios-dev       # iOS dev 模式（热更新）"
	@echo "  make init-mobile-tauri-android      # 初始化 Android 工程（首次一次）"
	@echo "  make run-mobile-tauri-android-dev   # Android dev 模式（真机/模拟器）"
	@echo "  make build-mobile-tauri-android-apk # 构建 Android unsigned APK（内部中间产物）"
	@echo "  make build-mobile-tauri-android-apk-test \\" 
	@echo "      ANDROID_KEYSTORE_PATH=... ANDROID_KEY_ALIAS=... \\" 
	@echo "      ANDROID_KEYSTORE_PASSWORD=... [ANDROID_KEY_PASSWORD=...]"
	@echo "  make build-mobile-tauri-android-apk-signed \\" 
	@echo "      ANDROID_KEYSTORE_PATH=... ANDROID_KEY_ALIAS=... \\" 
	@echo "      ANDROID_KEYSTORE_PASSWORD=... [ANDROID_KEY_PASSWORD=...]"
	@echo "  make build-mobile-tauri-android-aab # 构建 Android AAB（默认 aarch64）"
	@echo "  make pairing PAIR_ARGS='--show all' # 统一配对命令（relay 签发）"
	@echo "  make show-pairing                   # 输出配对信息 + 终端二维码"
	@echo "  make show-pairing-link              # 输出 yc://pair 链接"
	@echo "  make simulate-ios-scan              # 模拟二维码扫码（simctl openurl）"
	@echo "  make self-debug-loop                # 自动闭环：检查+服务+iOS启动+扫码+日志扫描"
	@echo "  make repair-ios-sim                 # 修复模拟器异常状态"

sync-mobile-tauri-android-secure-store:
	@if [ -f "$(ANDROID_SECURE_STORE_SRC)" ]; then \
		mkdir -p "$$(dirname "$(ANDROID_SECURE_STORE_DST)")"; \
		cp "$(ANDROID_SECURE_STORE_SRC)" "$(ANDROID_SECURE_STORE_DST)"; \
	else \
		echo "missing android secure store source: $(ANDROID_SECURE_STORE_SRC)"; \
		exit 1; \
	fi

check:
	cargo check --workspace

check-governance:
	./scripts/check-governance.sh
	./scripts/check-doc-consistency.sh

check-all:
	cargo check --workspace
	cargo fmt --check
	cargo clippy --workspace --all-targets -- -D warnings
	cargo test --workspace
	find app/mobile/ui/js -name '*.js' -print0 | xargs -0 -I{} sh -c 'node --check "$$1" && node --check --input-type=module < "$$1"' _ "{}"
	$(MAKE) check-governance

run-relay:
	cargo run -p yc-relay

run-sidecar:
	cargo run -p yc-sidecar

stop-relay:
	pkill -x yc-relay >/dev/null 2>&1 || true
	pkill -f 'cargo run -p yc-relay' >/dev/null 2>&1 || true

stop-sidecar:
	pkill -x yc-sidecar >/dev/null 2>&1 || true
	pkill -f 'cargo run -p yc-sidecar' >/dev/null 2>&1 || true

restart-relay:
	$(MAKE) stop-relay
	$(MAKE) run-relay

restart-sidecar:
	$(MAKE) stop-sidecar
	$(MAKE) run-sidecar

install-tauri-cli:
	cargo install tauri-cli --locked

boot-ios-sim:
	xcrun simctl boot "$(IOS_SIM)" >/dev/null 2>&1 || true

stop-mobile-tauri-ios:
	pkill -f 'cargo tauri ios dev' >/dev/null 2>&1 || true
	pkill -f 'cargo tauri ios build' >/dev/null 2>&1 || true
	pkill -f 'xcodebuild -allowProvisioningUpdates' >/dev/null 2>&1 || true
	pkill -f 'simctl spawn .* log stream.*dev.yourconnector.mobile' >/dev/null 2>&1 || true

repair-ios-sim:
	$(MAKE) stop-mobile-tauri-ios
	xcrun simctl terminate booted dev.yourconnector.mobile >/dev/null 2>&1 || true
	xcrun simctl shutdown all >/dev/null 2>&1 || true
	killall Simulator >/dev/null 2>&1 || true
	killall com.apple.CoreSimulator.CoreSimulatorService >/dev/null 2>&1 || true
	@for i in $$(seq 1 20); do \
		if xcrun simctl list devices available >/dev/null 2>&1; then \
			break; \
		fi; \
		sleep 1; \
	done
	xcrun simctl boot "$(IOS_SIM)" >/dev/null 2>&1 || true
	xcrun simctl bootstatus "$(IOS_SIM)" -b >/dev/null 2>&1 || true

run-mobile-tauri-ios:
	$(MAKE) stop-mobile-tauri-ios
	$(MAKE) boot-ios-sim IOS_SIM="$(IOS_SIM)"
	rm -rf "app/mobile/src-tauri/gen/apple/build/arm64-sim"
	rm -rf "app/mobile/src-tauri/gen/apple/build/yourconnector-mobile-tauri_iOS.xcarchive"
	cd app/mobile/src-tauri && cargo tauri ios build --debug -t aarch64-sim
	# 默认保留已安装应用数据，避免每次调试都丢失配对状态。
	xcrun simctl install booted "app/mobile/src-tauri/gen/apple/build/arm64-sim/yourConnector Mobile.app"
	xcrun simctl launch booted dev.yourconnector.mobile

run-mobile-tauri-ios-dev:
	$(MAKE) stop-mobile-tauri-ios
	$(MAKE) boot-ios-sim IOS_SIM="$(IOS_SIM)"
	cd app/mobile/src-tauri && cargo tauri ios dev "$(IOS_SIM)"

run-mobile-tauri-ios-dev-clean:
	$(MAKE) repair-ios-sim IOS_SIM="$(IOS_SIM)"
	xcrun simctl uninstall booted dev.yourconnector.mobile >/dev/null 2>&1 || true
	rm -rf app/mobile/src-tauri/target/aarch64-apple-ios-sim
	cd app/mobile/src-tauri && cargo tauri ios dev "$(IOS_SIM)"

ensure-mobile-tauri-android:
	@if [ -d app/mobile/src-tauri/gen/android ]; then \
		echo "Android 工程已初始化：app/mobile/src-tauri/gen/android"; \
	else \
		cd app/mobile/src-tauri && cargo tauri android init --ci; \
	fi
	@$(MAKE) -s sync-mobile-tauri-android-secure-store
	@if [ -x "$(ANDROID_MANIFEST_PATCH)" ]; then \
		"$(ANDROID_MANIFEST_PATCH)"; \
	else \
		echo "missing android manifest patch script: $(ANDROID_MANIFEST_PATCH)"; \
		exit 1; \
	fi

init-mobile-tauri-android: ensure-mobile-tauri-android

run-mobile-tauri-android-dev: ensure-mobile-tauri-android
	cd app/mobile/src-tauri && cargo tauri android dev $(if $(strip $(ANDROID_DEVICE)),"$(ANDROID_DEVICE)",)

build-mobile-tauri-android-apk: ensure-mobile-tauri-android
	cd app/mobile/src-tauri && cargo tauri android build --apk --target $(ANDROID_TARGETS)
	@echo "unsigned apk: $(ANDROID_UNSIGNED_APK_PATH)"

build-mobile-tauri-android-aab: ensure-mobile-tauri-android
	cd app/mobile/src-tauri && cargo tauri android build --aab --target $(ANDROID_TARGETS)

sign-mobile-tauri-android-apk:
	@if [ -x "$(ANDROID_APK_SIGN_SCRIPT)" ]; then \
		:; \
	else \
		echo "missing android apk sign script: $(ANDROID_APK_SIGN_SCRIPT)"; \
		exit 1; \
	fi
	@if [ -n "$(ANDROID_KEYSTORE_PATH)" ] && [ -n "$(ANDROID_KEY_ALIAS)" ] && [ -n "$(ANDROID_KEYSTORE_PASSWORD)" ]; then \
		:; \
	else \
		echo "missing signing vars. required: ANDROID_KEYSTORE_PATH ANDROID_KEY_ALIAS ANDROID_KEYSTORE_PASSWORD"; \
		exit 1; \
	fi
	"$(ANDROID_APK_SIGN_SCRIPT)" \
		--in "$(ANDROID_UNSIGNED_APK_PATH)" \
		--out "$(ANDROID_SIGNED_APK_PATH)" \
		--keystore "$(ANDROID_KEYSTORE_PATH)" \
		--alias "$(ANDROID_KEY_ALIAS)" \
		--store-pass "$(ANDROID_KEYSTORE_PASSWORD)" \
		$(if $(strip $(ANDROID_KEY_PASSWORD)),--key-pass "$(ANDROID_KEY_PASSWORD)",)

build-mobile-tauri-android-apk-signed: build-mobile-tauri-android-apk sign-mobile-tauri-android-apk

build-mobile-tauri-android-apk-test: build-mobile-tauri-android-apk-signed

pairing:
	@./scripts/pairing.sh \
		--pair-dir "$(PAIR_DIR)" \
		--relay "$(PAIR_RELAY_WS)" \
		$(if $(strip $(PAIR_NAME)),--name "$(PAIR_NAME)",) \
		$(PAIR_ARGS)

show-pairing:
	@$(MAKE) -s pairing PAIR_DIR="$(PAIR_DIR)" PAIR_RELAY_WS="$(PAIR_RELAY_WS)" PAIR_NAME="$(PAIR_NAME)" PAIR_ARGS="--show all"

show-pairing-code:
	@$(MAKE) -s pairing PAIR_DIR="$(PAIR_DIR)" PAIR_RELAY_WS="$(PAIR_RELAY_WS)" PAIR_NAME="$(PAIR_NAME)" PAIR_ARGS="--show code"

show-pairing-link:
	@$(MAKE) -s pairing PAIR_DIR="$(PAIR_DIR)" PAIR_RELAY_WS="$(PAIR_RELAY_WS)" PAIR_NAME="$(PAIR_NAME)" PAIR_ARGS="--show link"

show-pairing-qr:
	@$(MAKE) -s pairing PAIR_DIR="$(PAIR_DIR)" PAIR_RELAY_WS="$(PAIR_RELAY_WS)" PAIR_NAME="$(PAIR_NAME)" PAIR_ARGS="--show qr"

show-pairing-json:
	@$(MAKE) -s pairing PAIR_DIR="$(PAIR_DIR)" PAIR_RELAY_WS="$(PAIR_RELAY_WS)" PAIR_NAME="$(PAIR_NAME)" PAIR_ARGS="--show json"

simulate-ios-scan:
	@$(MAKE) -s pairing PAIR_DIR="$(PAIR_DIR)" PAIR_RELAY_WS="$(PAIR_RELAY_WS)" PAIR_NAME="$(PAIR_NAME)" PAIR_ARGS="--show link --simulate-ios-scan"

self-debug-loop:
	@./scripts/self-debug-loop.sh
