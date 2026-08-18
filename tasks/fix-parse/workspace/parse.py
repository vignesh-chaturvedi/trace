import sys

total = 0
for line in open("data.csv"):
    name, qty = line.strip().split(",")
    total += int(qty)

print(total)
