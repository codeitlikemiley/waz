.PHONY: build install clean test publish

# Build release binary
build:
	cargo build --release

# Build, install to both cargo bin and local bin, and reload shell integration
install: build
	cargo install --path . --force
	@mkdir -p $(HOME)/.local/bin
	cp $(HOME)/.cargo/bin/waz $(HOME)/.local/bin/waz
	@echo ""
	@echo "✅ waz installed to ~/.cargo/bin/waz and ~/.local/bin/waz"
	@echo "👉 Open a new terminal tab or run: source <(waz init zsh)"

# Run tests
test:
	cargo test

# Clean build artifacts
clean:
	cargo clean

# Publish to crates.io with auto version bump
# Fetches latest version from crates.io, bumps patch, updates Cargo.toml,
# runs dry-run, then asks for confirmation before publishing.
publish:
	@echo "📦 Fetching latest version from crates.io..."
	@CURRENT=$$(curl -sf https://crates.io/api/v1/crates/waz | sed -n 's/.*"max_version":"\([^"]*\)".*/\1/p'); \
	if [ -z "$$CURRENT" ]; then \
		echo "❌ Failed to fetch version from crates.io"; \
		exit 1; \
	fi; \
	echo "   Current version on crates.io: $$CURRENT"; \
	MAJOR=$$(echo $$CURRENT | cut -d. -f1); \
	MINOR=$$(echo $$CURRENT | cut -d. -f2); \
	PATCH=$$(echo $$CURRENT | cut -d. -f3); \
	NEW_PATCH=$$((PATCH + 1)); \
	NEW_VERSION="$$MAJOR.$$MINOR.$$NEW_PATCH"; \
	echo "   Bumping to: $$NEW_VERSION"; \
	echo ""; \
	sed -i '' "s/^version = \".*\"/version = \"$$NEW_VERSION\"/" Cargo.toml; \
	cargo generate-lockfile 2>/dev/null; \
	git add Cargo.toml Cargo.lock; \
	git commit -m "chore: bump version to v$$NEW_VERSION"; \
	git tag "v$$NEW_VERSION"; \
	echo ""; \
	echo "🔍 Running dry-run..."; \
	echo ""; \
	if cargo publish --dry-run; then \
		echo ""; \
		echo "✅ Dry-run passed! Ready to publish $$NEW_VERSION"; \
		echo ""; \
		printf "🚀 Publish waz@$$NEW_VERSION to crates.io? [y/N] "; \
		read CONFIRM; \
		if [ "$$CONFIRM" = "y" ] || [ "$$CONFIRM" = "Y" ]; then \
			cargo publish; \
			echo ""; \
			echo "✅ Published waz@$$NEW_VERSION to crates.io"; \
			echo "👉 Run 'git push && git push --tags' to push"; \
		else \
			echo "⏭ Publish cancelled. Reverting version bump..."; \
			git tag -d "v$$NEW_VERSION"; \
			git reset --soft HEAD~1; \
			git checkout Cargo.toml Cargo.lock; \
			echo "  Reverted to $$CURRENT"; \
		fi; \
	else \
		echo ""; \
		echo "❌ Dry-run failed. Reverting version bump..."; \
		git tag -d "v$$NEW_VERSION"; \
		git reset --soft HEAD~1; \
		sed -i '' "s/^version = \".*\"/version = \"$$CURRENT\"/" Cargo.toml; \
		cargo generate-lockfile 2>/dev/null; \
		echo "  Reverted to $$CURRENT. Fix errors above, then try again."; \
	fi
