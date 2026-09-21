"""Observe model-dependent ACP selectors without ever sending a prompt."""
from .discovery import parse_observation


def observe_model_efforts(call, session_id, options, *, timeout):
    observation = parse_observation(options)
    model_option = next((o for o in observation.options if o.category == 'model' and o.type == 'select'), None)
    if model_option is None:
        return {}
    levels = {}
    try:
        for model, _ in observation.model_options():
            result = call('session/set_config_option', dict(sessionId=session_id, configId=model_option.id, value=model), timeout=timeout)
            selected = parse_observation(result['configOptions'])
            selected_model = next(o for o in selected.options if o.category == 'model')
            if selected_model.current_value != model:
                raise ValueError('model selection was not confirmed')
            # An absent selector is observed lack of support, not a fallback
            # to the initial model's effort choices.
            levels[model] = selected.model_thought_levels.get(model, ())
    finally:
        call('session/set_config_option', dict(sessionId=session_id, configId=model_option.id, value=model_option.current_value), timeout=timeout)
    return levels
