"""Debian ordering metadata, separate from the stamped runtime identity."""
from datetime import datetime
import re


def build_stamp(value):
    if not re.fullmatch(r"[0-9]{20}", value):
        raise ValueError("build stamp must be UTC YYYYMMDDhhmmssffffff")
    try:
        datetime.strptime(value, "%Y%m%d%H%M%S%f")
    except ValueError:
        raise ValueError("invalid UTC build stamp") from None
    return value


def debian_version(identity, stamp):
    match = re.fullmatch(r"(1\.0\.6)\+sha256\.([0-9a-f]{16})", identity)
    if not match:
        raise ValueError("invalid package version")
    # 'z' sorts after 's' in every legacy +sha256.<hash>, even f-prefixed ones.
    return match[1] + "+z" + build_stamp(stamp) + ".sha256." + match[2]
