.PHONY: setup-hooks install dev preflight preflight-fast help

# ── Setup ─────────────────────────────────────────────────

setup-hooks: ## Configure git to use .githooks/ for hooks
	git config core.hooksPath .githooks
	@echo "Git hooks configured: .githooks/"

# ── Build & Install ──────────────────────────────────────

install: ## Build release + install to ~/.local/bin
	cd rust && cargo install --path . --force --locked --root "$$HOME/.local"
	@echo "Installed: $$("$$HOME/.local/bin/sobre" --version)"

# The new binary is copied beside the old one, then renamed over it: an agent
# may be running the old one, which cannot be overwritten in place.
dev: ## Quick debug build + copy to ~/.local/bin
	cd rust && cargo build
	@mkdir -p "$$HOME/.local/bin"
	cd rust && cp "$${CARGO_TARGET_DIR:-target}/debug/sobre" "$$HOME/.local/bin/.sobre.new"
	mv -f "$$HOME/.local/bin/.sobre.new" "$$HOME/.local/bin/sobre"
	@echo "Dev installed: $$("$$HOME/.local/bin/sobre" --version)"

# ── CI gates ─────────────────────────────────────────────
# CI runs scripts/preflight.sh too; it also checks dependency licences and
# advisories with cargo-deny (rust/deny.toml), which these targets do not.

preflight: ## The CI gates: lint + the whole test suite
	scripts/preflight.sh full

preflight-fast: ## The CI lint gates only (what pre-push runs)
	scripts/preflight.sh

# ── Help ──────────────────────────────────────────────────

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | \
		awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-18s\033[0m %s\n", $$1, $$2}'

.DEFAULT_GOAL := help
