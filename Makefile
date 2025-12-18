sample-run:
	cd output && rm -rf *  && cd .. && cargo run -- -i ./tests/testdata/ubuntu-24.04.3-desktop-amd64.iso.torrent -o ./output/

test:
	# Print the stdout and stderr to the file = 2>&1
	RUST_BACKTRACE=1 cargo run . > output.txt 2>&1

format:
	cargo fmt --all

lint:
	cargo clippy -- -D warnings
