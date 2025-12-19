sample-run-1:
	rm logs.txt | \
	rm -rf output/* && cargo run -- -i ./tests/testdata/ubuntu-24.04.3-desktop-amd64.iso.torrent -o ./output/ > ./output/logs.txt

sample-run-2:
	rm -rf output/* && cargo run -- -i ./tests/testdata/ubuntu-24.04.3-desktop-amd64.iso.torrent -o ./output/

format:
	cargo fmt --all

lint:
	cargo clippy -- -D warnings

test:
	cargo test
