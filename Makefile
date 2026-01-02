sample-run-1: clean
	LOG_LEVEL=DEBUG RUST_BACKTRACE=1 cargo run -- -i ./tests/testdata/ubuntu-24.04.3-desktop-amd64.iso.torrent -o ./output/ --max-download-rate 2M --max-upload-rate 100K --max-peers 100

sample-run-2: clean
	LOG_LEVEL=INFO RUST_BACKTRACE=1 cargo run -- -i ./tests/testdata/ubuntu-24.04.3-desktop-amd64.iso.torrent -o ./output/ --max-download-rate 2M --max-upload-rate 100K --max-peers 100

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
