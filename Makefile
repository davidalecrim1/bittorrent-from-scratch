sample-run-1: clean
	LOG_LEVEL=DEBUG RUST_BACKTRACE=1 cargo run -- \
		-i ./tests/testdata/ubuntu-24.04.3-desktop-amd64.iso.torrent \
		-o ./output/ \
		--max-download-rate 2M \
		--max-upload-rate 100K \
		--max-peers 15

sample-run-2: clean
	LOG_LEVEL=INFO RUST_BACKTRACE=1 cargo run -- \
		-i ./tests/testdata/ubuntu-24.04.3-desktop-amd64.iso.torrent \
		-o ./output/ \
		--max-download-rate 2M \
		--max-upload-rate 100K \
		--max-peers 15

build:
	cargo build --release

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
	cargo tarpaulin --all-targets --engine llvm --out Stdout

coverage-by-file:
	@./scripts/coverage-by-file.sh

# Source: https://releases.ubuntu.com/noble/SHA256SUMS
verify-hash-ubuntu-image:
	@echo "Expected Hash"
	@echo "faabcf33ae53976d2b8207a001ff32f4e5daae013505ac7188c9ea63988f8328"
	@echo "Actual Hash"
	@shasum -a 256 ./output/ubuntu-24.04.3-desktop-amd64.iso
