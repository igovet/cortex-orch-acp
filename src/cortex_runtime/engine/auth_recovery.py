"""Recognize native engine auth loss without inspecting native credentials."""
from . import failure_classification
from .client import EngineProtocolError


def is_auth_loss(error):
    # Runtime permission/channel errors are not native-account failures.
    if not isinstance(error, EngineProtocolError) and not isinstance(getattr(error, 'kind', None), str):
        return False
    if failure_classification.route_failure(error) is not None:
        return False
    return failure_classification.classify(
        getattr(error, 'kind', None),
        message=str(getattr(error, 'message', '') or '')[:500],
    ) == failure_classification.AUTHORIZATION_LOSS


def login_blocker(engine=None):
    command = {'codex-acp': 'codex login', 'claude-agent-acp': 'claude /login'}.get(engine)
    commands = command or 'claude /login or codex login'
    return 'engine_authentication_required: Native engine authentication needs attention. Re-login with ' + commands + ', then resume.'
