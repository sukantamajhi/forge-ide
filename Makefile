# Forge: common tasks. Run `make` or `make help` to list them.
# Works with GNU make from Git Bash or WinGet (winget install ezwinports.make).

CARGO  ?= cargo
APP    := forge-app
# Folder or file to open with `make run`, e.g. `make run PATH_ARG=../my-project`.
PATH_ARG ?= .
SITE_PORT ?= 8099

.DEFAULT_GOAL := help
.PHONY: help run build release test check fmt fmt-check lint clean installer site

help: ## List the available tasks
	@grep -E '^[a-zA-Z_-]+:.*## ' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*## "} {printf "  make %-12s %s\n", $$1, $$2}'

run: ## Run Forge on a folder or file (PATH_ARG=...)
	$(CARGO) run -p $(APP) -- $(PATH_ARG)

build: ## Build everything (dev profile)
	$(CARGO) build --workspace

release: ## Optimised build of the app (LTO, slow first time)
	$(CARGO) build --release -p $(APP)

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

installer: ## Build dist/Forge-Setup-<version>.exe (needs Inno Setup 6)
	powershell -NoProfile -ExecutionPolicy Bypass -File installer/build.ps1

site: ## Serve the landing page at http://127.0.0.1:$(SITE_PORT)
	cd site && python -m http.server $(SITE_PORT) --bind 127.0.0.1

clean: ## Remove build output
	$(CARGO) clean
