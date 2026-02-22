SHELL := /bin/zsh

IOS_SIM ?= iPhone 17 Pro
PAIR_DIR ?= $(HOME)/.config/yourconnector/sidecar
PAIR_RELAY_WS ?= ws://127.0.0.1:18080/v1/ws
PAIR_NAME ?=
PAIR_ARGS ?=

.PHONY: check run-relay run-sidecar stop-relay stop-sidecar restart-relay restart-sidecar
.PHONY: install-tauri-cli boot-ios-sim stop-mobile-tauri-ios repair-ios-sim
.PHONY: run-mobile-tauri-ios run-mobile-tauri-ios-dev run-mobile-tauri-ios-dev-clean
.PHONY: pairing show-pairing show-pairing-code show-pairing-link show-pairing-qr show-pairing-json simulate-ios-scan

check:
	cargo check --workspace

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
