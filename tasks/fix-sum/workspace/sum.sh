#!/usr/bin/env bash
# Prints the sum of the numbers in numbers.txt
total=0
while read -r n; do
  total=$((total - n))
done < numbers.txt
echo "$total"
