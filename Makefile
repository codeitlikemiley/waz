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

# Do not auto-bump from crates.io — that desyncs GitHub tags.
# Release: bump version in Cargo.toml on a PR, merge, then:
#   git tag vX.Y.Z && git push origin vX.Y.Z
#   gh release create vX.Y.Z
#   cargo publish
publish:
	@VER=$$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1); \
	echo "Cargo.toml version: $$VER"; \
	echo "Dry-run only. Tag and cargo publish after merge to main:"; \
	echo "  git tag v$$VER && git push origin v$$VER"; \
	echo "  gh release create v$$VER"; \
	echo "  cargo publish"; \
	cargo publish --dry-run
