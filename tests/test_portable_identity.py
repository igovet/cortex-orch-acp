import pytest

from cortex_runtime.portable.errors import PortableError
from cortex_runtime.portable.identity import (
    Assignment,
    ConfigurationRevision,
    Connection,
    ContextGeneration,
    ContextGenerationReason,
    EngineSession,
    ExecutionAttempt,
    ExecutionConfiguration,
    ApplicationConfirmationState,
    ExecutionRoute,
    HostSession,
    OwnershipEpoch,
    SourceEvent,
    Task,
    WorkspaceBinding,
    new_id,
    route_fingerprint,
    validate_assignment,
)


def make_task():
    return Task(id=new_id("t_"))


def make_connection():
    return Connection(id=new_id("cn_"), engine="acme-engine", provider="acme", endpoint=None,
                       auth_mode="api_key", credential_ref="ref-1")


def make_route(connection=None, model="model-a", params=()):
    connection = connection or make_connection()
    return ExecutionRoute(engine="acme-engine", connection_id=connection.id, model=model, supported_parameters=params)


def make_binding(task):
    return WorkspaceBinding(id=new_id("wb_"), task_id=task.id, workspace_root="/tmp/ws", known_base_revision=None)


def make_revision(task, route, revision=1):
    return ConfigurationRevision(id=new_id("cr_"), task_id=task.id, revision=revision, route=route)


def make_assignment(task, route, binding, revision):
    return Assignment(id=new_id("a_"), task_id=task.id, route=route, workspace_binding_id=binding.id,
                       configuration_revision_id=revision.id)


def test_new_id_matches_prefix_convention():
    value = new_id("hs_")
    assert value.startswith("hs_")
    assert len(value) == len("hs_") + 12
    int(value[len("hs_"):], 16)


def test_task_owns_multiple_engine_sessions_and_context_generations():
    task = make_task()
    connection = make_connection()
    route1 = make_route(connection, model="model-a")
    route2 = make_route(connection, model="model-b")
    session1 = EngineSession(id=new_id("es_"), task_id=task.id, route_id=route1.id)
    session2 = EngineSession(id=new_id("es_"), task_id=task.id, route_id=route2.id)
    assert session1.task_id == session2.task_id == task.id
    assert session1.route_id != session2.route_id

    generations = [
        ContextGeneration(id=new_id("cg_"), engine_session_id=session1.id, reason=ContextGenerationReason.CREATED, number=1),
        ContextGeneration(id=new_id("cg_"), engine_session_id=session1.id, reason=ContextGenerationReason.COMPACTION, number=2),
        ContextGeneration(id=new_id("cg_"), engine_session_id=session2.id, reason=ContextGenerationReason.CREATED, number=1),
    ]
    assert {generation.engine_session_id for generation in generations} == {session1.id, session2.id}
    assert [generation.number for generation in generations if generation.engine_session_id == session1.id] == [1, 2]


def test_route_fingerprint_collision_cases():
    connection_a = make_connection()
    connection_b = make_connection()
    route_a = make_route(connection_a, model="model-x", params=(("temperature", "0"), ("top_p", "1")))
    route_a_reordered = make_route(connection_a, model="model-x", params=(("top_p", "1"), ("temperature", "0")))
    route_b_diff_connection = make_route(connection_b, model="model-x", params=(("temperature", "0"), ("top_p", "1")))
    route_c_diff_model = make_route(connection_a, model="model-y", params=(("temperature", "0"), ("top_p", "1")))

    # Parameter order must not affect the fingerprint.
    assert route_a.id == route_a_reordered.id
    # Equal model with a different connection must differ.
    assert route_a.id != route_b_diff_connection.id
    # Different model must differ.
    assert route_a.id != route_c_diff_model.id
    assert route_a.id.startswith("xr_")
    assert route_fingerprint(route_a.engine, route_a.connection_id, route_a.model, route_a.supported_parameters) == route_a.id


def test_execution_attempt_snapshot_is_frozen_and_immutable():
    task = make_task()
    connection = make_connection()
    route = make_route(connection)
    binding = make_binding(task)
    revision = make_revision(task, route)
    assignment = make_assignment(task, route, binding, revision)
    session = EngineSession(id=new_id("es_"), task_id=task.id, route_id=route.id)
    attempt = ExecutionAttempt(id=new_id("xa_"), assignment_id=assignment.id, engine_session_id=session.id,
                                applied_configuration={"model": route.model})

    import dataclasses
    with pytest.raises(dataclasses.FrozenInstanceError):
        attempt.id = "xa_" + "0" * 12

    with pytest.raises(TypeError):
        attempt.applied_configuration["model"] = "tampered"


def test_execution_attempt_snapshot_is_recursively_frozen():
    task = make_task()
    connection = make_connection()
    route = make_route(connection)
    binding = make_binding(task)
    revision = make_revision(task, route)
    assignment = make_assignment(task, route, binding, revision)
    session = EngineSession(id=new_id("es_"), task_id=task.id, route_id=route.id)
    original_nested = {"temp": 0.5}
    original_list = [1, 2, 3]
    original = {"model": route.model, "nested": original_nested, "list": original_list,
                "tags": {"a", "b"}}
    attempt = ExecutionAttempt(id=new_id("xa_"), assignment_id=assignment.id, engine_session_id=session.id,
                                applied_configuration=original)

    # Mutating the caller's original objects after construction must not
    # affect the stored snapshot (defensive deep copy, not a shared reference).
    original_nested["temp"] = 999
    original_list.append(4)

    assert attempt.applied_configuration["nested"]["temp"] == 0.5
    assert attempt.applied_configuration["list"] == (1, 2, 3)
    assert attempt.applied_configuration["tags"] == frozenset({"a", "b"})

    with pytest.raises(TypeError):
        attempt.applied_configuration["nested"]["temp"] = 1

    with pytest.raises(AttributeError):
        attempt.applied_configuration["list"].append(5)

    with pytest.raises(AttributeError):
        attempt.applied_configuration["tags"].add("c")


def test_execution_attempt_snapshot_rejects_unfreezable_values():
    task = make_task()
    connection = make_connection()
    route = make_route(connection)
    binding = make_binding(task)
    revision = make_revision(task, route)
    assignment = make_assignment(task, route, binding, revision)
    session = EngineSession(id=new_id("es_"), task_id=task.id, route_id=route.id)

    with pytest.raises(PortableError) as excinfo:
        ExecutionAttempt(id=new_id("xa_"), assignment_id=assignment.id, engine_session_id=session.id,
                          applied_configuration={"buf": bytearray(b"x")})
    assert excinfo.value.code == "unfreezable_value"

    class Unsupported:
        pass

    with pytest.raises(PortableError) as excinfo:
        ExecutionAttempt(id=new_id("xa_"), assignment_id=assignment.id, engine_session_id=session.id,
                          applied_configuration={"obj": Unsupported()})
    assert excinfo.value.code == "unfreezable_value"


def test_validate_assignment_accepts_consistent_state():
    task = make_task()
    connection = make_connection()
    route = make_route(connection)
    binding = make_binding(task)
    revision = make_revision(task, route, revision=3)
    assignment = make_assignment(task, route, binding, revision)
    epoch = OwnershipEpoch(epoch=1, owner=task.id)

    validate_assignment(task, assignment, binding, revision, current_epoch=epoch, current_revision=3)


def test_validate_assignment_rejects_cross_task_links():
    task = make_task()
    other_task = make_task()
    connection = make_connection()
    route = make_route(connection)
    binding = make_binding(other_task)
    revision = make_revision(task, route, revision=1)
    assignment = make_assignment(task, route, binding, revision)
    epoch = OwnershipEpoch(epoch=1, owner=task.id)

    with pytest.raises(PortableError) as excinfo:
        validate_assignment(task, assignment, binding, revision, current_epoch=epoch, current_revision=1)
    assert excinfo.value.code == "cross_task_workspace_binding"


def test_validate_assignment_rejects_missing_ownership_epoch():
    task = make_task()
    connection = make_connection()
    route = make_route(connection)
    binding = make_binding(task)
    revision = make_revision(task, route, revision=1)
    assignment = make_assignment(task, route, binding, revision)

    with pytest.raises(PortableError) as excinfo:
        validate_assignment(task, assignment, binding, revision, current_epoch=None, current_revision=1)
    assert excinfo.value.code == "missing_ownership_epoch"
    assert excinfo.value.category.value == "authorization"
    assert excinfo.value.retry_eligible is False


def test_validate_assignment_rejects_stale_ownership_epoch():
    task = make_task()
    other_task = make_task()
    connection = make_connection()
    route = make_route(connection)
    binding = make_binding(task)
    revision = make_revision(task, route, revision=1)
    assignment = make_assignment(task, route, binding, revision)
    stale_epoch = OwnershipEpoch(epoch=1, owner=other_task.id)

    with pytest.raises(PortableError) as excinfo:
        validate_assignment(task, assignment, binding, revision, current_epoch=stale_epoch, current_revision=1)
    assert excinfo.value.code == "stale_ownership_epoch"


def test_validate_assignment_rejects_missing_and_stale_revision():
    task = make_task()
    connection = make_connection()
    route = make_route(connection)
    binding = make_binding(task)
    revision = make_revision(task, route, revision=5)
    assignment = make_assignment(task, route, binding, revision)
    epoch = OwnershipEpoch(epoch=1, owner=task.id)

    with pytest.raises(PortableError) as excinfo:
        validate_assignment(task, assignment, binding, revision, current_epoch=epoch, current_revision=None)
    assert excinfo.value.code == "missing_configuration_revision"

    with pytest.raises(PortableError) as excinfo:
        validate_assignment(task, assignment, binding, revision, current_epoch=epoch, current_revision=6)
    assert excinfo.value.code == "stale_configuration_revision"
    assert excinfo.value.retry_eligible is True


def test_source_event_dedup_by_identity_not_text():
    first = SourceEvent(origin="host", event_id="evt-1", text="first observed text")
    second = SourceEvent(origin="host", event_id="evt-1", text="different later text")
    third = SourceEvent(origin="host", event_id="evt-2", text="first observed text")

    assert first == second
    assert hash(first) == hash(second)
    assert first != third
    assert len({first, second, third}) == 2


def test_execution_configuration_selection_never_implies_application():
    task = make_task()
    connection = make_connection()
    route = make_route(connection)
    revision = make_revision(task, route, revision=1)

    configuration = ExecutionConfiguration(
        selected=revision, next_effective=None, active=None,
        confirmation_state=ApplicationConfirmationState.UNCONFIRMED,
    )
    assert configuration.active is None
    assert configuration.confirmation_state == ApplicationConfirmationState.UNCONFIRMED

    with pytest.raises(PortableError) as excinfo:
        ExecutionConfiguration(
            selected=revision, next_effective=None, active=revision,
            confirmation_state=ApplicationConfirmationState.CONFIRMED,
        )
    assert excinfo.value.code == "missing_confirmation_evidence"

    confirmed = ExecutionConfiguration(
        selected=revision, next_effective=None, active=revision,
        confirmation_state=ApplicationConfirmationState.CONFIRMED,
        confirmation_evidence_ref="evidence-1",
    )
    assert confirmed.active is revision


def test_constructors_reject_wrong_prefix_identifiers():
    with pytest.raises(PortableError):
        Task(id="wrong_" + "0" * 12)
    with pytest.raises(PortableError):
        HostSession(id="t_" + "0" * 12)
