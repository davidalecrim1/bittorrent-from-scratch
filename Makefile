test:
	# Print the stdout and stderr to the file
	rm output.txt && RUST_BACKTRACE=1 cargo run . > output.txt 2>&1