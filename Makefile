# Forge: common tasks. Run `make` or `make help` to list them.
# On Windows use GNU make from Git Bash or WinGet (winget install ezwinports.make). On Linux, plain `make`.
#
# Building for both systems:
#   make release-windows   Windows build (run it on Windows)
#   make release-linux     Linux build (run it on Linux; on Windows it goes through WSL2)
#   make release-all       both, from a Windows machine with WSL2, or one at a time on each system
#   make package-all       dist/Forge-Setup-<version>.exe and dist/forge-<version>-linux-x86_64.tar.gz

CARGO  ?= cargo
APP    := forge-app
# Folder or file to open with `make run`, e.g. `make run PATH_ARG=../my-project`.
PATH_ARG ?= .
SITE_PORT ?= 8099
VERSION := $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)

# Which system `make` itself is running on.
ifeq ($(OS),Windows_NT)
HOST := windows
EXE  := .exe
else
HOST := $(shell uname -s | tr '[:upper:]' '[:lower:]')
EXE  :=
endif

# Linux builds made from Windows (through WSL2) go in their own folder so they never mix with the Windows ones.
LINUX_TARGET_DIR ?= target-linux
LINUX_NAME  := forge-$(VERSION)-linux-x86_64
LINUX_STAGE := dist/$(LINUX_NAME)
# The Windows-style path of this folder, which `wsl --cd` wants.
WIN_DIR := $(shell pwd -W 2>/dev/null || pwd)

# Packages the Linux build needs on Ubuntu or Debian (other distributions have the same libraries under other names).
LINUX_DEPS := build-essential pkg-config clang cmake libfontconfig-dev libwayland-dev libxkbcommon-dev libxkbcommon-x11-dev \
              libx11-xcb-dev libxcb1-dev libssl-dev libzstd-dev libvulkan1 libasound2-dev

.DEFAULT_GOAL := help
.PHONY: help run build release release-windows release-linux release-all test check fmt fmt-check lint clean \
        installer package-windows package-linux package-all linux-deps site site-docker-build site-docker-run

help: ## List the available tasks
	@grep -E '^[a-zA-Z_-]+:.*## ' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*## "} {printf "  make %-16s %s\n", $$1, $$2}'
	@echo ""
	@echo "  Running on: $(HOST).  Version: $(VERSION)."

run: ## Run Forge on a folder or file (PATH_ARG=...)
	$(CARGO) run -p $(APP) -- $(PATH_ARG)

build: ## Build everything (dev profile)
	$(CARGO) build --workspace

release: ## Optimised build for this system (LTO, slow first time)
	$(CARGO) build --release -p $(APP)

release-windows: ## Optimised Windows build: target/release/forge.exe (run on Windows)
ifeq ($(HOST),windows)
	$(CARGO) build --release -p $(APP)
else
	@echo "The Windows build has to be made on Windows (it needs the MSVC build tools). Run 'make release-windows' there." && false
endif

release-linux: ## Optimised Linux build (on Windows: through WSL2, into target-linux/)
ifeq ($(HOST),windows)
	wsl.exe --cd "$(WIN_DIR)" -e bash -lc 'CARGO_TARGET_DIR=$(LINUX_TARGET_DIR) cargo build --release -p $(APP)'
else ifeq ($(HOST),linux)
	$(CARGO) build --release -p $(APP)
else
	@echo "The Linux build has to be made on Linux or in WSL2 (this is $(HOST))." && false
endif

release-all: release-windows release-linux ## Windows and Linux builds (needs Windows with WSL2, or run each on its own system)

linux-deps: ## Install the libraries the Linux build needs (Ubuntu / Debian; asks for sudo; inside WSL on Windows)
ifeq ($(HOST),windows)
	wsl.exe --cd "$(WIN_DIR)" -e bash -lc 'sudo apt-get update && sudo apt-get install -y $(LINUX_DEPS) && (command -v cargo >/dev/null || echo "Install Rust inside WSL first: https://rustup.rs")'
else ifeq ($(HOST),linux)
	sudo apt-get update && sudo apt-get install -y $(LINUX_DEPS)
else
	@echo "Linux packages are only installed on Linux." && false
endif

test: ## Run the whole workspace's tests
	$(CARGO) test --workspace

check: ## Type-check without building
	$(CARGO) check --workspace

fmt: ## Format the code
	$(CARGO) fmt --all

fmt-check: ## Fail if the code is not formatted
	$(CARGO) fmt --all -- --check

lint: ## Clippy, warnings as errors
	$(CARGO) clippy --workspace -- -D warnings

installer: package-windows ## Same as package-windows

package-windows: ## Build dist/Forge-Setup-<version>.exe (Windows, needs Inno Setup 6)
ifeq ($(HOST),windows)
	powershell -NoProfile -ExecutionPolicy Bypass -File installer/build.ps1
else
	@echo "The installer is built on Windows with Inno Setup 6." && false
endif

# The Linux package is a folder with the program, a desktop entry, the icon and an install script, as a .tar.gz.
package-linux: release-linux ## Build dist/forge-<version>-linux-x86_64.tar.gz (needs a Linux build)
	rm -rf "$(LINUX_STAGE)" && mkdir -p "$(LINUX_STAGE)" dist
ifeq ($(HOST),windows)
	cp "$(LINUX_TARGET_DIR)/release/forge" "$(LINUX_STAGE)/forge"
else
	cp "target/release/forge" "$(LINUX_STAGE)/forge"
endif
	cp installer/linux/forge.desktop installer/linux/install.sh assets/forge-logo.svg README.md "$(LINUX_STAGE)/"
	chmod +x "$(LINUX_STAGE)/forge" "$(LINUX_STAGE)/install.sh"
	tar -C dist -czf "dist/$(LINUX_NAME).tar.gz" "$(LINUX_NAME)"
	rm -rf "$(LINUX_STAGE)"
	@echo "Built dist/$(LINUX_NAME).tar.gz"

package-all: package-windows package-linux ## Windows installer and Linux archive (Windows with WSL2)

site: ## Serve the landing page on port 8099 (SITE_PORT=...)
	cd site && python -m http.server $(SITE_PORT) --bind 127.0.0.1

SITE_IMAGE ?= forge-site
SITE_DOCKER_PORT ?= 8080

site-docker-build: ## Build the landing page image (Dockerfile.site)
	docker build -f Dockerfile.site -t $(SITE_IMAGE) .

site-docker-run: site-docker-build ## Run the landing page image at http://localhost:$(SITE_DOCKER_PORT)
	docker run --rm -p $(SITE_DOCKER_PORT):8080 $(SITE_IMAGE)

clean: ## Remove build output (including the WSL Linux build folder)
	$(CARGO) clean
	rm -rf "$(LINUX_TARGET_DIR)" "$(LINUX_STAGE)"
