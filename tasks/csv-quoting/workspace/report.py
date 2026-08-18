def load_rows(path):
    rows = []
    with open(path) as fh:
        header = fh.readline().strip().split(",")
        for line in fh:
            line = line.strip()
            if not line:
                continue
            rows.append(dict(zip(header, line.split(","))))
    return rows


def total_amount(path="sales.csv"):
    """Sum the `amount` column."""
    return sum(int(row["amount"]) for row in load_rows(path))


def by_region(path="sales.csv"):
    """Return {region: total} for every region in the file."""
    totals = {}
    for row in load_rows(path):
        totals[row["region"]] = totals.get(row["region"], 0) + int(row["amount"])
    return totals


if __name__ == "__main__":
    print(total_amount())
