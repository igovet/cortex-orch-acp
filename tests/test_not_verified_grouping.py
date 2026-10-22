"""Not-verified items with one blocker render as one line (2026-10-22: the
same device limit was listed about ten times in one final answer)."""
from cortex_runtime.orchestration.state import disclosed_synthesis


def test_items_sharing_a_blocker_render_as_one_line():
    devices = 'No real screen reader or physical device was available.'
    text = disclosed_synthesis('Done.', [
        dict(item='screen reader pass', blocker=devices),
        dict(item='real phone pass', blocker=' No real screen reader or  physical device was available. '),
        dict(item='native zoom', blocker='The browser exposes no native zoom control.'),
    ], {}, 'en')
    lines = [line for line in text.splitlines() if line.startswith('- ')]
    assert len(lines) == 2
    assert lines[0].startswith('- screen reader pass; real phone pass — ') and lines[0].endswith(devices)
    assert lines[1].startswith('- native zoom — ')
