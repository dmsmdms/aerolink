# AeroLink — top-level build entry point.
# Every target delegates to a script in tools/, so the scripts stay usable on their own.
# Artifacts go to build/.

PICO_SDK_PATH ?= /opt/pico-sdk
PICO_BOARD    ?= pico
BUILD_TYPE    ?= Release
ANDROID_VARIANT ?= release

export PICO_SDK_PATH PICO_BOARD BUILD_TYPE ANDROID_VARIANT

TOOLS := $(CURDIR)/tools

.DEFAULT_GOAL := all
.PHONY: all rp2040 android frontend backend \
        clean clean-rp2040 clean-android clean-frontend clean-backend \
        test test-rp2040 test-android test-backend test-frontend e2e bench-server vectors \
        flash hwtest-rp2040 install-apk help

all: rp2040 android backend ## Build everything (default)

rp2040: ## Build RP2040 firmware -> build/aerolink-rp2040.uf2 (loader + app, PC) and .bin (app, phone updates)
	@$(TOOLS)/build_rp2040.sh

android: ## Build Android robot app -> build/aerolink-<abi>.apk, carries the firmware of make rp2040 (ANDROID_VARIANT=debug for bench tests)
	@$(TOOLS)/build_android.sh

frontend: ## Build operator console + landing -> build/www (pre-compressed .br/.gz)
	@$(TOOLS)/build_frontend.sh

backend: frontend ## Build Rust backend + nginx/systemd -> build/aerolink.deb (ships build/www)
	@$(TOOLS)/build_backend.sh

test: test-rp2040 test-android test-backend test-frontend ## Run all unit tests

test-rp2040: ## Run firmware host tests (codec, control, link)
	@$(TOOLS)/test_rp2040.sh

test-android: ## Run Android JVM unit tests
	@$(TOOLS)/test_android.sh

test-backend: ## Run backend tests (incl. HTTP/WebSocket end-to-end)
	@$(TOOLS)/test_backend.sh

test-frontend: ## Run frontend unit tests (vitest)
	@$(TOOLS)/test_frontend.sh

e2e: frontend ## Console end-to-end test: backend + fake robot in headless Chromium
	@$(TOOLS)/e2e_console.sh

bench-server: frontend ## Backend on the LAN + console on this PC for the robot app on a phone (BENCH_USB=1: over the cable)
	@$(TOOLS)/bench_server.sh

vectors: ## Regenerate protocol/vectors.txt from tools/gen_vectors.py
	@python3 -I $(TOOLS)/gen_vectors.py

clean: clean-rp2040 clean-android clean-frontend clean-backend ## Remove all build artifacts
	@rm -rf $(CURDIR)/build
	@echo "✅ All cleanup tasks completed."

clean-rp2040: ## Clean firmware build
	@$(TOOLS)/clean_rp2040.sh

clean-android: ## Clean Android build
	@$(TOOLS)/clean_android.sh

clean-frontend: ## Clean frontend build
	@$(TOOLS)/clean_frontend.sh

clean-backend: ## Clean backend build
	@$(TOOLS)/clean_backend.sh

flash: ## Flash firmware via picotool (board in BOOTSEL mode or running AeroLink fw)
	@$(TOOLS)/flash_rp2040.sh

hwtest-rp2040: rp2040 ## Loader test on a bare board in BOOTSEL (overwrites its flash; make flash afterwards)
	@$(TOOLS)/hwtest_rp2040.py $(SCENARIOS)

install-apk: ## Install build/aerolink-<abi>.apk matching the connected phone via adb
	@$(TOOLS)/install_android.sh

help: ## Show this help
	@echo "Usage: make [target] [VAR=value ...]"
	@echo
	@echo "Targets:"
	@grep -hE '^[a-zA-Z0-9_-]+:.*## ' $(MAKEFILE_LIST) | \
		awk 'BEGIN {FS = ":.*## "}; {printf "  \033[36m%-14s\033[0m %s\n", $$1, $$2}'
	@echo
	@echo "Variables:"
	@echo "  PICO_SDK_PATH  Pico SDK location        (current: $(PICO_SDK_PATH))"
	@echo "  PICO_BOARD     Pico SDK board name      (current: $(PICO_BOARD))"
	@echo "  BUILD_TYPE     Firmware CMake build type (current: $(BUILD_TYPE))"
	@echo "  ANDROID_VARIANT release or debug         (current: $(ANDROID_VARIANT))"
