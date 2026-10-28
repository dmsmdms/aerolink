# AeroLink — top-level build entry point.
# Every target delegates to a script in tools/, so the scripts stay usable on their own.
# Artifacts go to build/.

PICO_SDK_PATH ?= /opt/pico-sdk
PICO_BOARD    ?= pico
BUILD_TYPE    ?= Release

export PICO_SDK_PATH PICO_BOARD BUILD_TYPE

TOOLS := $(CURDIR)/tools

.DEFAULT_GOAL := all
.PHONY: all rp2040 android backend \
        clean clean-rp2040 clean-android clean-backend \
        test test-rp2040 test-android vectors \
        flash install-apk help

all: rp2040 android backend ## Build everything (default)

rp2040: ## Build RP2040 firmware -> build/aerolink-rp2040.uf2
	@$(TOOLS)/build_rp2040.sh

android: ## Build Android robot app -> build/aerolink.apk
	@$(TOOLS)/build_android.sh

backend: ## Build Rust backend -> build/aerolink.deb
	@$(TOOLS)/build_backend.sh

test: test-rp2040 test-android ## Run all unit tests

test-rp2040: ## Run firmware host tests (codec, control, link)
	@$(TOOLS)/test_rp2040.sh

test-android: ## Run Android JVM unit tests
	@$(TOOLS)/test_android.sh

vectors: ## Regenerate protocol/vectors.txt from tools/gen_vectors.py
	@python3 -I $(TOOLS)/gen_vectors.py

clean: clean-rp2040 clean-android clean-backend ## Remove all build artifacts
	@rm -rf $(CURDIR)/build
	@echo "✅ All cleanup tasks completed."

clean-rp2040: ## Clean firmware build
	@$(TOOLS)/clean_rp2040.sh

clean-android: ## Clean Android build
	@$(TOOLS)/clean_android.sh

clean-backend: ## Clean backend build
	@$(TOOLS)/clean_backend.sh

flash: ## Flash firmware via picotool (board in BOOTSEL mode or running AeroLink fw)
	@$(TOOLS)/flash_rp2040.sh

install-apk: ## Install build/aerolink.apk on the connected phone via adb
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
