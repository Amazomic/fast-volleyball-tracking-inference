import hashlib
from functools import lru_cache
from pathlib import Path


def signature(path):
    path = Path(path)
    stat = path.stat()
    return _signature(str(path), stat.st_size, stat.st_mtime_ns)


@lru_cache(maxsize=4096)
def _signature(path, size, modified):
    raw = Path(path).read_bytes()
    digest = hashlib.sha256(raw).hexdigest()
    try:
        import cv2
        import numpy as np
    except ImportError:
        return digest, None
    image = cv2.imdecode(np.frombuffer(raw, dtype=np.uint8), cv2.IMREAD_COLOR)
    if image is None:
        return digest, None
    thumb = cv2.resize(image, (16, 32), interpolation=cv2.INTER_AREA)
    gray = cv2.resize(cv2.cvtColor(image, cv2.COLOR_BGR2GRAY), (9, 16), interpolation=cv2.INTER_AREA)
    bits = 0
    for value in (gray[:, 1:] > gray[:, :-1]).flat:
        bits = (bits << 1) | int(value)
    return digest, (thumb.tobytes(), bits)


def similar(left, right):
    if left[0] == right[0]:
        return True
    if left[1] is None or right[1] is None:
        return False
    a, ah = left[1]
    b, bh = right[1]
    if len(a) != len(b) or not a or (ah ^ bh).bit_count() > 6:
        return False
    return sum(abs(x - y) for x, y in zip(a, b)) / len(a) <= 7
