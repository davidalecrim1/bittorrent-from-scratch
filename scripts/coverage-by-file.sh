#!/bin/bash
# Display test coverage by file, sorted by percentage

echo "Coverage by file (sorted by percentage):"
echo "=========================================="

# Run tarpaulin and extract coverage data
cargo tarpaulin --all-targets --engine llvm --out Stdout 2>&1 | \
    grep "^||" | \
    grep -E "src/.+\.rs: [0-9]+/[0-9]+" | \
    sed 's/^|| //' | \
    while IFS= read -r line; do
        # Extract filename and coverage numbers
        if [[ $line =~ ^(src/[^:]+\.rs):[[:space:]]*([0-9]+)/([0-9]+) ]]; then
            file="${BASH_REMATCH[1]}"
            covered="${BASH_REMATCH[2]}"
            total="${BASH_REMATCH[3]}"

            # Calculate percentage
            if [ "$total" -gt 0 ]; then
                percentage=$(awk "BEGIN {printf \"%.2f\", ($covered/$total)*100}")
                # Format output with percentage for sorting
                printf "%06.2f %-50s %s/%s (%.2f%%)\n" "$percentage" "$file" "$covered" "$total" "$percentage"
            fi
        fi
    done | \
    sort -rn | \
    cut -d' ' -f2-

echo ""
