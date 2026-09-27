"""Every observed ACP action remains visible, including partial updates."""
import threading
import pytest
from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime


def emitter(cards, **kwargs):
    return OrchestratingRuntime.__new__(OrchestratingRuntime)._engine_tool_call_emitter(cards.append, **kwargs)


@pytest.mark.parametrize('title',['Terminal','Read','Write','Wait','update_plan','mcp__cortex__read_report',
    'mcp.cortex.plan_submit','mcp__cortex__draft_write','mcp__other__inspect','spawn_agent','Thinking','Agent approval',None])
def test_observed_titles_cannot_hide_actions_or_control_lifecycle(title):
    cards=[];cancel=threading.Event();emit=emitter(cards,cancel_event=cancel)
    raw={'path':'.cortex/example','command':'print(1)','values':list(range(3000))}
    emit('one','other',title,'pending',None,None,raw)
    assert cards[0]['new'] is True
    assert cards[0]['raw_input']==raw
    emit('one',None,None,'in_progress')
    emit('one',None,None,'completed',None,[{'type':'text','text':'full observed output'}],None,{'exit_code':0})
    assert len(cards)==3
    assert len({card['tool_call_id'] for card in cards})==1
    assert [card['status'] for card in cards]==['pending','in_progress','completed']
    assert [card['new'] for card in cards]==[True,False,False]
    assert cards[-1]['content']==[{'type':'text','text':'full observed output'}]
    assert cards[-1]['raw_output']=={'exit_code':0}
    assert not cancel.is_set()


def test_calls_within_the_task_budget_are_never_collapsed_and_later_ones_are_counted():
    """Paseo retains every card, so a task's cards are bounded in total: the
    first COORDINATOR_CARD_BUDGET calls are complete own cards, later calls
    are counted on one aggregate card and a failure still gets its own card."""
    from cortex_runtime.acp import wire_budget
    budget = wire_budget.COORDINATOR_CARD_BUDGET
    cards=[];emit=emitter(cards)
    for i in range(300):emit(str(i),'read','Inspect','completed',None,None,{'path':str(i)},{'body':'a'*5000})
    own=[c for c in cards if c['tool_call_id'].startswith('call:coordinator:')]
    assert len(own)==len({c['tool_call_id'] for c in own})==budget
    assert all(len(c['raw_output']['body'])==5000 for c in own)
    folded=[c for c in cards if c['tool_call_id'].startswith(wire_budget.FOLDED_CARD_PREFIX)]
    assert folded[0]['new'] is True and len(folded)==1
    assert folded[0]['raw_output']['furtherToolCalls']==1
    emit('late','edit','Edit','failed',None,None,{'path':'x'},{'error':'write conflict'})
    assert cards[-1]['tool_call_id']=='call:coordinator:late' and cards[-1]['status']=='failed'
    assert cards[-1]['raw_output']=={'error':'write conflict'}
    emit.close_group()
    assert cards[-1]['tool_call_id'].startswith(wire_budget.FOLDED_CARD_PREFIX)
    assert cards[-1]['raw_output']['furtherToolCalls']==300-budget


def test_secrets_are_redacted_without_losing_diff_or_error():
    cards=[];emit=emitter(cards)
    diff={'type':'diff','path':'src/a','oldText':'one','newText':'two'}
    emit('edit','edit','Edit','failed',[{'path':'src/a'}],[diff],{'token':'private-token','text':'a'*9000},{'error':'write conflict'})
    assert cards[0]['raw_input']['token']=='[REDACTED]'
    assert len(cards[0]['raw_input']['text'])==9000
    assert cards[0]['content']==[diff]
    assert cards[0]['raw_output']=={'error':'write conflict'}
