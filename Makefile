sample-run-1: clean
	RUST_BACKTRACE=1 cargo run -- -i ./tests/testdata/ubuntu-24.04.3-desktop-amd64.iso.torrent -o ./output/

sample-run-2: clean
	RUST_BACKTRACE=1 cargo run -- -i ./tests/testdata/alpine-standard-3.23.2-aarch64.iso.torrent -o ./output/

clean:
	rm -rf output/*

format:
	cargo fmt --all

lint:
	cargo clippy -- -D warnings

test:
	cargo test

coverage:
	cargo tarpaulin --all-targets --engine llvm --out Stdout
