#!/usr/bin/env bash
set -eu
cat > cache.py <<'PYEOF'
class LRUCache:
    """Least-recently-used cache with a fixed capacity."""

    def __init__(self, capacity):
        self.capacity = capacity
        self._data = {}
        self._order = []

    def get(self, key):
        if key not in self._data:
            return None
        self._touch(key)
        return self._data[key]

    def put(self, key, value):
        self._data[key] = value
        self._touch(key)
        while len(self._data) > self.capacity:
            oldest = self._order.pop(0)
            del self._data[oldest]

    def _touch(self, key):
        if key in self._order:
            self._order.remove(key)
        self._order.append(key)

    def __len__(self):
        return len(self._data)
PYEOF
