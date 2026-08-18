from words import reverse_words

def test_basic():
    assert reverse_words("one two three") == "three two one"

def test_single():
    assert reverse_words("solo") == "solo"

def test_empty():
    assert reverse_words("") == ""

def test_collapses_whitespace():
    assert reverse_words("  a   b  ") == "b a"

if __name__ == "__main__":
    test_basic(); test_single(); test_empty(); test_collapses_whitespace()
    print("all tests passed")
