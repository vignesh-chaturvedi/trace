def find_span(text, needle):
    """Return (start, end) offsets of `needle` in `text`, or None.

    Offsets are byte offsets into the UTF-8 encoding of `text`.
    """
    data = text.encode("utf-8")
    target = needle.encode("utf-8")
    at = data.find(target)
    if at < 0:
        return None
    return (at, at + len(target))
