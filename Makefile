sample-run-1:
	rm -rf output/* && RUST_BACKTRACE=1 cargo run -- -i ./tests/testdata/ubuntu-24.04.3-desktop-amd64.iso.torrent -o ./output/

format:
	cargo fmt --all

lint:
	cargo clippy -- -D warnings

test:
	cargo test

coverage:
	cargo tarpaulin --all-targets --engine llvm --out Stdout
