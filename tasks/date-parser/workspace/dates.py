import re

# Tried in order. First match wins.
PATTERNS = [
    (r"(\d{1,2})/(\d{1,2})/(\d{4})", ("month", "day", "year")),   # 3/14/2026
    (r"(\d{4})-(\d{1,2})-(\d{1,2})", ("year", "month", "day")),   # 2026-03-14
    (r"(\d{1,2})-(\d{1,2})-(\d{4})", ("day", "month", "year")),   # 14-03-2026
]


def parse_date(text):
    """Parse a date into (year, month, day).

    Supported formats:
      2026-03-14   ISO, year first
      3/14/2026    US, month first
      14-03-2026   European, day first

    Returns None if the text contains no recognisable date, and raises
    ValueError if the date is not a real calendar date.
    """
    for pattern, order in PATTERNS:
        m = re.search(pattern, text)
        if m:
            parts = dict(zip(order, (int(g) for g in m.groups())))
            return (parts["year"], parts["month"], parts["day"])
    return None
