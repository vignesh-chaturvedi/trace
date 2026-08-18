from tokenize import find_span


def highlight(text, needle, marker="**"):
    """Wrap the first occurrence of `needle` in `text` with `marker`."""
    span = find_span(text, needle)
    if span is None:
        return text
    start, end = span
    return text[:start] + marker + text[start:end] + marker + text[end:]
