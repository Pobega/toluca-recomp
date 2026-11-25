ADB ?= $(shell command -v adb 2>/dev/null || echo $(HOME)/.local/bin/adb)

# NextUI (PORTS.pak) SD card layout (TrimUI Brick / Smart Pro, RGSP / H700, etc.)
NEXTUI_PORTS_DIR ?= /mnt/SDCARD/Roms/Ports (PORTS)
NEXTUI_DOTPORTS_DIR ?= $(NEXTUI_PORTS_DIR)/.ports
NEXTUI_GAMEDIR ?= $(NEXTUI_DOTPORTS_DIR)/toluca

DIST_BIN_DIR := dist/aarch64
TOOLBOX_RUN := $(if $(wildcard /run/.containerenv),,toolbox run --container fedora-toolbox-44)

.PHONY: all build test-aarch64 host test package push push-fast logs

all: build

build:
	bash scripts/build-aarch64.sh

test-aarch64:
	bash scripts/build-aarch64.sh --test

host:
	$(TOOLBOX_RUN) cargo build --release -p toluca-recomp -p toluca-runtime

test:
	$(TOOLBOX_RUN) cargo test --workspace

package:
	@if [ ! -f "$(DIST_BIN_DIR)/toluca-recomp" ] || [ ! -f "$(DIST_BIN_DIR)/toluca-runtime" ] || [ ! -f "$(DIST_BIN_DIR)/libs/libavcodec.so.61" ]; then \
		bash scripts/build-aarch64.sh; \
	fi
	bash scripts/package-portmaster.sh

# Push release files to a NextUI device over ADB without clobbering gamedata/ or toluca.conf.
# Clears cached AOT/shader artifacts so PortMaster's patcher cleanly recompiles on next launch.
push:
	@if [ ! -f "$(DIST_BIN_DIR)/toluca-recomp" ] || [ ! -f "$(DIST_BIN_DIR)/toluca-runtime" ] || [ ! -f "$(DIST_BIN_DIR)/libs/libavcodec.so.61" ]; then \
		bash scripts/build-aarch64.sh; \
	fi
	@$(ADB) get-state >/dev/null
	@echo "==> Pushing Toluca.sh to NextUI Ports directories..."
	$(ADB) push "portmaster/Toluca.sh" "$(NEXTUI_PORTS_DIR)/Toluca.sh"
	$(ADB) push "portmaster/Toluca.sh" "$(NEXTUI_DOTPORTS_DIR)/Toluca.sh"
	@echo "==> Preparing $(NEXTUI_GAMEDIR)/ directories (preserving gamedata/ and toluca.conf)..."
	$(ADB) shell 'mkdir -p "$(NEXTUI_GAMEDIR)/bin" "$(NEXTUI_GAMEDIR)/tools" "$(NEXTUI_GAMEDIR)/gamedata" "$(NEXTUI_GAMEDIR)/cache" "$(NEXTUI_GAMEDIR)/libs.aarch64" "$(NEXTUI_GAMEDIR)/licenses"'
	@echo "==> Pushing release files to $(NEXTUI_GAMEDIR)/..."
	$(ADB) push \
		"portmaster/README.md" \
		"portmaster/port.json" \
		"portmaster/toluca.gptk" \
		"$(NEXTUI_GAMEDIR)/"
	$(ADB) push "portmaster/gamedata/README.txt" "$(NEXTUI_GAMEDIR)/gamedata/README.txt"
	$(ADB) push "portmaster/tools/patchscript" "$(NEXTUI_GAMEDIR)/tools/patchscript"
	$(ADB) push portmaster/licenses/* "$(NEXTUI_GAMEDIR)/licenses/"
	$(ADB) push $(DIST_BIN_DIR)/libs/libav*.so.* "$(NEXTUI_GAMEDIR)/libs.aarch64/"
	$(ADB) push \
		"$(DIST_BIN_DIR)/toluca-recomp" \
		"$(DIST_BIN_DIR)/toluca-runtime" \
		"$(NEXTUI_GAMEDIR)/bin/"
	@echo "==> Clearing AOT/shader cache so PortMaster patcher runs on boot..."
	$(ADB) shell 'chmod +x "$(NEXTUI_GAMEDIR)/bin/toluca-recomp" "$(NEXTUI_GAMEDIR)/bin/toluca-runtime" "$(NEXTUI_GAMEDIR)/tools/patchscript" 2>/dev/null || true; rm -f "$(NEXTUI_GAMEDIR)/cache/toluca_code.bin" "$(NEXTUI_GAMEDIR)/cache/sh2pc.sha1" "$(NEXTUI_GAMEDIR)/cache/toluca-progs.bin"; sync'
	@echo "==> Done! Launch Toluca on your NextUI device to run the PortMaster patcher."

# Fast dev deploy: pushes updated launcher and aarch64 binaries while preserving gamedata/,
# toluca.conf, and the already-compiled cache/toluca_code.bin (skips on-device recompilation).
push-fast:
	@if [ ! -f "$(DIST_BIN_DIR)/toluca-recomp" ] || [ ! -f "$(DIST_BIN_DIR)/toluca-runtime" ] || [ ! -f "$(DIST_BIN_DIR)/libs/libavcodec.so.61" ]; then \
		bash scripts/build-aarch64.sh; \
	fi
	@$(ADB) get-state >/dev/null
	@echo "==> Pushing Toluca.sh to NextUI Ports directories..."
	$(ADB) push "portmaster/Toluca.sh" "$(NEXTUI_PORTS_DIR)/Toluca.sh"
	$(ADB) push "portmaster/Toluca.sh" "$(NEXTUI_DOTPORTS_DIR)/Toluca.sh"
	@echo "==> Pushing updated binaries and scripts to $(NEXTUI_GAMEDIR)/ (preserving gamedata/ and cache/)..."
	$(ADB) shell 'mkdir -p "$(NEXTUI_GAMEDIR)/bin" "$(NEXTUI_GAMEDIR)/tools" "$(NEXTUI_GAMEDIR)/gamedata" "$(NEXTUI_GAMEDIR)/cache" "$(NEXTUI_GAMEDIR)/libs.aarch64" "$(NEXTUI_GAMEDIR)/licenses"'
	$(ADB) push "portmaster/toluca.gptk" "$(NEXTUI_GAMEDIR)/toluca.gptk"
	$(ADB) push "portmaster/tools/patchscript" "$(NEXTUI_GAMEDIR)/tools/patchscript"
	$(ADB) push portmaster/licenses/* "$(NEXTUI_GAMEDIR)/licenses/"
	$(ADB) push $(DIST_BIN_DIR)/libs/libav*.so.* "$(NEXTUI_GAMEDIR)/libs.aarch64/"
	$(ADB) push \
		"$(DIST_BIN_DIR)/toluca-recomp" \
		"$(DIST_BIN_DIR)/toluca-runtime" \
		"$(NEXTUI_GAMEDIR)/bin/"
	$(ADB) shell 'chmod +x "$(NEXTUI_GAMEDIR)/bin/toluca-recomp" "$(NEXTUI_GAMEDIR)/bin/toluca-runtime" "$(NEXTUI_GAMEDIR)/tools/patchscript" 2>/dev/null || true; sync'
	@echo "==> Done! Launch Toluca on your NextUI device."

logs:
	@$(ADB) shell 'cat "$(NEXTUI_GAMEDIR)/patchlog.txt" 2>/dev/null; echo "---"; cat "$(NEXTUI_GAMEDIR)/log.txt" 2>/dev/null'
