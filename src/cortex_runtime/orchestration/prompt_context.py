"""Prompt-only projection of orchestration context for coordinator decisions."""


def coordinator_prompt_context(context):
    """Compact superseded review-cycle nodes without changing persisted context.

    The context returned by TaskMachine.context remains the complete public
    task state. This projection is applied only when serializing it into a
    coordinator prompt.
    """
    nodes = context.get("nodes")
    cycle = context.get("review_cycle")
    if (not isinstance(nodes, list) or not isinstance(cycle, int)
            or isinstance(cycle, bool) or cycle < 1):
        return context

    by_id = {
        node.get("node_id"): node
        for node in nodes
        if isinstance(node, dict) and isinstance(node.get("node_id"), str)
    }
    review_id = f"_review_{cycle}"
    if review_id not in by_id:
        return context

    current_fix_prefix = f"_fix_{cycle}_"
    current_fixes = {
        node_id for node_id in by_id
        if node_id.startswith(current_fix_prefix)
    }
    if current_fixes:
        # A review has requested changes and its latest fix batch is underway.
        current_ids = {review_id, *current_fixes}
    elif cycle == 1:
        plan = context.get("plan")
        tasks = plan.get("tasks") if isinstance(plan, dict) else None
        plan_ids = {
            task.get("id") for task in tasks or ()
            if isinstance(task, dict) and isinstance(task.get("id"), str)
        }
        # The first review evaluates the original plan nodes.
        if not plan_ids or not plan_ids.issubset(by_id):
            return context
        current_ids = {review_id, *plan_ids}
    else:
        # Review N evaluates the fixes produced by review N-1.
        prior_fix_prefix = f"_fix_{cycle - 1}_"
        prior_fixes = {
            node_id for node_id in by_id
            if node_id.startswith(prior_fix_prefix)
        }
        if not prior_fixes:
            return context
        current_ids = {review_id, *prior_fixes}

    historical_nodes = [node for node in nodes if node.get("node_id") not in current_ids]
    if not historical_nodes:
        return context

    projected = dict(context)
    projected_nodes = []
    historical_check_gaps = []
    for node in nodes:
        node_id = node.get("node_id")
        if node_id in current_ids:
            projected_nodes.append(node)
            continue

        result = node.get("result")
        result = result if isinstance(result, dict) else {}
        projected_nodes.append({
            "node_id": node_id,
            "role": node.get("role"),
            "status": node.get("status"),
            "report_ref": result.get("report_ref"),
            "verdict": result.get("verdict"),
        })
        for check in result.get("checks") or ():
            if not isinstance(check, dict):
                continue
            outcome = check.get("outcome")
            if (outcome in {"not_run", "unverified"}
                    or (outcome == "failed" and not check.get("closed_by"))):
                historical_check_gaps.append({
                    "node_id": node_id,
                    "check": check,
                })

    projected["nodes"] = projected_nodes
    projected["historical_check_gaps"] = historical_check_gaps
    return projected
