test:
	cargo run . 2> output.txt || tail -n 20 output.txt