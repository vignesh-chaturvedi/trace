class LRUCache:
    """Least-recently-used cache with a fixed capacity.

    Both get() and put() count as a use.
    """

    def __init__(self, capacity):
        self.capacity = capacity
        self._data = {}
        self._order = []          # least-recently-used first

    def get(self, key):
        if key not in self._data:
            return None
        self._touch(key)
        return self._data[key]

    def put(self, key, value):
        self._data[key] = value
        self._touch(key)
        if len(self._data) > self.capacity:
            oldest = self._order.pop(0)
            del self._data[oldest]

    def _touch(self, key):
        self._order.append(key)

    def __len__(self):
        return len(self._data)
