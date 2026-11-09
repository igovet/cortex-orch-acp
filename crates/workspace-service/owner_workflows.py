"""Closed explicit workflow controls. Task descriptions never dispatch slash commands."""
import machine_tools as m


def describe(params):
    from cortex_runtime.acp import messages
    action, scope = params.get("action"), params.get("scope", "")
    if action not in ("harvest", "harvest-refresh", "skill-management") or not isinstance(scope, str) or len(scope) > 30000:
        raise m.Refused("invalid_workflow", "Choose a supported workflow and a scope up to 30000 characters.")
    scope = scope.strip()
    if action == "skill-management":
        if not scope:
            raise m.Refused("description_required", "Describe the skill to create, revise or import.")
        from cortex_runtime.orchestration.skill_proposals import workflow_instruction
        return {"description": scope + "\n\nThe user explicitly invoked /skill-management. " +
                workflow_instruction("the user invoked the registered command")}
    refresh = action == "harvest-refresh"
    goal = scope or ("Update the project's existing source-backed documentation" if refresh else
                     "Create complete source-backed documentation for the project")
    instruction = messages.render("harvest_refresh_requested" if refresh else "harvest_requested",
                                  "en", scope=(" (" + scope + ")" if scope else ""))
    return {"description": goal + "\n\n" + instruction}
