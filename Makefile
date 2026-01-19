sample-run-1: clean
	LOG_LEVEL=DEBUG RUST_BACKTRACE=1 cargo run -- \
		-i ./tests/testdata/ubuntu-24.04.3-desktop-amd64.iso.torrent \
		-o ./output/ \
		--max-download-rate 2M \
		--max-upload-rate 100K \
		--max-peers 15 \
		--log-dir ./logs

sample-run-2: clean
	LOG_LEVEL=INFO RUST_BACKTRACE=1 cargo run -- \
		-i ./tests/testdata/ubuntu-24.04.3-desktop-amd64.iso.torrent \
		-o ./output/ \
		--max-download-rate 2M \
		--max-upload-rate 100K \
		--max-peers 15 \
		--log-dir ./logs

sample-run-3: clean
	LOG_LEVEL=DEBUG RUST_BACKTRACE=1 cargo run -- \
		-i ./tests/testdata/alpine-standard-3.23.2-aarch64.iso.torrent \
		-o ./output/ \
		--max-download-rate 2M \
		--max-upload-rate 100K \
		--max-peers 15 \
		--log-dir ./logs \
		--no-tracker

sample-run-4: clean
	LOG_LEVEL=DEBUG RUST_BACKTRACE=1 cargo run -- \
		-i ./tests/testdata/alpine-standard-3.23.2-aarch64.iso.torrent \
		-o ./output/ \
		--max-download-rate 2M \
		--max-upload-rate 100K \
		--max-peers 15 \
		--log-dir ./logs

sample-run-5: clean
	LOG_LEVEL=DEBUG RUST_BACKTRACE=1 cargo run -- \
		-i ./tests/testdata/archlinux-2026.01.01-x86_64.iso.torrent \
		-o ./output/ \
		--max-download-rate 2M \
		--max-upload-rate 100K \
		--max-peers 15 \
		--log-dir ./logs

sample-run-magnet-1: clean
	LOG_LEVEL=DEBUG RUST_BACKTRACE=1 cargo run -- \
		--input "magnet:?xt=urn:btih:1e873cd33f55737aaaefc0c282c428593c16e106&dn=archlinux-2026.01.01-x86_64.iso" \
		--output ./output/ \
		--name archlinux-2026.01.01-x86_64.iso \
		--max-download-rate 2M \
		--max-upload-rate 100K \
		--max-peers 15 \
		--log-dir ./logs

build:
	cargo build --release

install:
	cargo build --release
	@mkdir -p ~/.local/bin
	@cp target/release/bittorrent-from-scratch ~/.local/bin/bittorrent
	@echo "Installed to ~/.local/bin/bittorrent"
	@echo "Make sure ~/.local/bin is in your PATH"

clean:
	@rm -rf output/*
	@rm -rf target/*

format:
	cargo fmt --all

lint:
	cargo clippy -- -D warnings

test:
	cargo test

coverage:
	cargo tarpaulin --all-targets --engine llvm --out Stdout --timeout 30

coverage-by-file:
	@./scripts/coverage-by-file.sh

# Source: https://releases.ubuntu.com/noble/SHA256SUMS
verify-hash-ubuntu-image:
	@echo "Expected Hash"
	@echo "faabcf33ae53976d2b8207a001ff32f4e5daae013505ac7188c9ea63988f8328"
	@echo "Actual Hash"
	@shasum -a 256 ./output/ubuntu-24.04.3-desktop-amd64.iso

# Source: https://archlinux.org/download/
verify-hash-archlinux:
	@echo "Expected Hash"
	@echo "16502a7c18eed827ecead95c297d26f9f4bd57c4b3e4a8f4e2b88cf60e412d6f"
	@echo "Actual Hash"
	@shasum -a 256 ./output/archlinux-2026.01.01-x86_64.iso
