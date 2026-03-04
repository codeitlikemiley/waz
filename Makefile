.PHONY: build install clean test

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
