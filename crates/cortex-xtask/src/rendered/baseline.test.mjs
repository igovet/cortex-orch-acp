import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { bindPeer, capabilityFrame, classify, correlate, decoder, exactHistoryRow, followFrame, isSubscription, markerIn, parsePeerTrace, rateSummary, sqliteWindow } from './baseline.mjs';

test('delivery comparison changes only the actual negotiated method in the control', () => {
  const response = {jsonrpc:'2.0',id:'k',result:{methods:['native.board','native.history.follow'],machine:true}};
  for (const arm of ['pull','push']) {
    const frame = capabilityFrame(structuredClone(response), {id:'k'}, arm);
    assert.equal(frame.readUInt32BE(),frame.length-4);
    const actual = JSON.parse(frame.subarray(4));
    if (arm === 'pull') actual.result.methods.push('native.history.follow');
    assert.deepEqual(actual,response);
  }
  assert.throws(()=>capabilityFrame({...response,result:{methods:[]}}, {id:'k'}, 'pull'),/real history-follow/);
});

test('private marker requires aligned project, task, execution, generation and cursor continuity', () => {
  const ids = {projectId:'p',taskId:'t',executionId:'e'};
  const request = {method:'native.history.follow',params:ids};
  const state = {ack:true,generation:'g',cursor:'nt1.first'};
  const marker = {kind:'assistant',text:'Post-question coordinator text',sequence:'3',historyId:'h'};
  const event = {...ids,type:'native_history_page',generation:'g',throughSequence:'9',fromCursor:'nt1.first',page:{items:[marker],tailCursor:'nt1.next'}};
  for (const bad of [{taskId:'other'},{projectId:'other'},{executionId:'other'},{generation:'other'},{fromCursor:null}]) {
    assert.throws(()=>followFrame(request,{...event,...bad},{...state}));
  }
  assert.throws(()=>followFrame({method:'service.subscribe'},event,state),/unrelated socket/);
  assert.equal(followFrame(request,event,state),marker);
  assert.equal(state.cursor,'nt1.next');
  assert.throws(()=>followFrame(request,event,state),/discontinuous/);
});

test('SQLite SQL coverage distinguishes uninstrumented binaries and counts only the measured time', () => {
  const record = (time,kind) => {const bytes=Buffer.alloc(16);bytes.writeBigUInt64LE(BigInt(time));bytes.writeBigUInt64LE(BigInt(kind),8);return bytes;};
  const trace = Buffer.concat([[1,1],[2,2],[6,4],[7,5],[8,3],[9,7]].map(([time,kind])=>record(time,kind)));
  assert.equal(sqliteWindow(trace,4,9).statements,2);
  assert.equal(sqliteWindow(trace,4,9).readonlyResultStatements,1);
  assert.equal(sqliteWindow(trace,4,9).resultRows,1);
  assert.equal(sqliteWindow(trace,4,9).closes,0);
  assert.deepEqual(sqliteWindow(Buffer.alloc(0),4,9),{coverage:'uninstrumented'});
  assert.throws(()=>sqliteWindow(trace.subarray(1),4,9),/truncated/);
  assert.throws(()=>sqliteWindow(trace.subarray(16),4,9),/unavailable/);
});

test('counts sent history and detail operations without treating project reads as full refreshes', () => {
  assert.equal(classify({method: 'native.query', params: {operation: 'history.page'}}), 'history');
  assert.equal(classify({method: 'workspace', params: {operation: 'history.page'}}), 'history');
  assert.equal(classify({method: 'native.query', params: {operation: 'tasks.get'}}), 'detail');
  assert.equal(classify({method: 'machine.call', params: {op: 'product.facts'}}), 'detail');
  assert.equal(classify({method: 'service.capabilities'}), 'fullRefresh');
  for (const method of ['projects.list', 'service.subscribe', 'native.board']) assert.equal(classify({method}), undefined);
});

test('tracks both native subscription handshake routes', () => {
  assert.equal(isSubscription('service.subscribe'), true);
  assert.equal(isSubscription('service.subscribe.code'), true);
  assert.equal(isSubscription('projects.list'), false);
});

test('retains complete zero bins, excludes warmup and preserves totals in the fractional tail', () => {
  const s = 1e9;
  const zero = {history: 0, detail: 0, fullRefresh: 0};
  const events = [
    {time: s - 1, kind: 'detail'}, {time: s, kind: 'history'},
    {time: 3 * s, kind: 'detail'}, {time: 4 * s - 1, kind: 'fullRefresh'},
    {time: 4 * s, kind: 'history'},
  ];
  assert.deepEqual(rateSummary(events, s, 3.5 * s), {
    durationNs: 2.5 * s, windows: [{...zero, history: 1}, zero],
    totals: {...zero, history: 1, detail: 1},
  });
  assert.deepEqual(rateSummary([], s, 3 * s).windows, [zero, zero]);
  assert.throws(() => rateSummary([], s, s + 2), /incomplete/);
});

test('joins to the first visible captured pixels, not a later positive or a pre-existing label', () => {
  const frames = [
    {begin: 70, end: 80, visible: false},
    {begin: 110, end: 112, visible: false},
    {begin: 150, end: 156, visible: true},
    {begin: 180, end: 185, visible: true},
  ];
  assert.deepEqual(correlate(100, frames), {latencyUpperNs: 56, latencyLowerNs: 10, frameGapNs: 46});
  assert.throws(() => correlate(null, frames), /missing text/);
  assert.throws(() => correlate(100, frames.slice(0, 2)), /no captured frame/);
  assert.throws(() => correlate(100, frames.slice(1)), /no pre-arrival/);
  assert.throws(() => correlate(155, frames), /unbracketed/);
  assert.throws(() => correlate(100, [frames[1], frames[0]]), /unordered/);
});

test('accepts a fixture text field and not a tool label or a quoted lookalike', () => {
  assert.equal(markerIn({result: [{text: 'Native fixture progress.'}]}), true);
  assert.equal(markerIn({result: [{text: 'Native fixture progress. Extra text.'}]}), true);
  assert.equal(markerIn({result: [{title: 'Native fixture progress.'}]}), false);
  assert.equal(markerIn({result: [{text: 'Quote: Native fixture progress.'}]}), false);
});

test('binds one complete emission to the exact task, project, session and durable row', () => {
  const trace = '547 fixture-1 110 115\n';
  const emission = parsePeerTrace(trace);
  const id = 'wh_' + createHash('sha256').update('["display",3]').digest('hex').slice(0, 32);
  const scope = {taskId: 'wt_' + 'a'.repeat(32), projectId: 'wp_' + 'b'.repeat(32),
    historyId: id, historySequence: '4'};
  const binding = {taskId: scope.taskId, projectId: scope.projectId, sessionId: 'fixture-1',
    displaySessionId: 'display', sourceSequence: 3,
    runtimeTaskId: 't_' + 'c'.repeat(32), attemptId: 'xa_' + 'd'.repeat(32)};
  assert.deepEqual(bindPeer(emission, [binding], scope, 100, 140), binding);
  for (const wrong of [{sessionId: 'fixture-2'}, {taskId: 'other'}, {projectId: 'other'}]) {
    assert.throws(() => bindPeer(emission, [{...binding, ...wrong}], scope, 100, 140), /matching/);
  }
  assert.throws(() => bindPeer(emission, [{...binding, sourceSequence: 4}], scope, 100, 140), /same durable history/);
  assert.throws(() => bindPeer(emission, [binding, binding], scope, 100, 140), /matching/);
  assert.throws(() => bindPeer(emission, [binding], scope, 100, 114), /clock window/);
  for (const bad of ['', trace + trace, trace.trim(), '547 fixture-1 115 110\n']) {
    assert.throws(() => parsePeerTrace(bad), /emission/);
  }
});

test('requires one exact assistant row and brackets pixels against both ends of the write', () => {
  const row = {kind: 'assistant', text: 'Post-question coordinator text', historyId: 'h', sequence: '2'};
  assert.equal(exactHistoryRow({items: [{...row, kind: 'tool'}, row]}), row);
  const paragraph = {...row, text: '\n\n' + row.text};
  assert.equal(exactHistoryRow({items: [paragraph]}), paragraph);
  assert.equal(exactHistoryRow({items: [{...row, text: row.text + '!'}]}), undefined);
  assert.throws(() => exactHistoryRow({items: [row, row]}), /ambiguous/);
  const frames = [{begin: 90, end: 95, visible: false}, {begin: 120, end: 125, visible: false},
    {begin: 130, end: 135, visible: true}, {begin: 150, end: 155, visible: true}];
  assert.deepEqual(correlate(101, frames, 107), {latencyUpperNs: 34, latencyLowerNs: 13, frameGapNs: 15});
  assert.throws(() => correlate(101, frames, 131), /unbracketed/);
});

test('handles split and coalesced IPC chunks without dropping or duplicating requests', () => {
  const seen = [];
  const make = message => {
    const body = Buffer.from(JSON.stringify(message));
    const head = Buffer.alloc(4);
    head.writeUInt32BE(body.length);
    return Buffer.concat([head, body]);
  };
  const messages = [{id: '1', method: 'native.query'}, {id: '2', method: 'machine.call'}];
  const first = make(messages[0]);
  const stream = decoder((message, time) => seen.push({message, time}));
  stream.push(first.subarray(0, 2), 10);
  assert.equal(stream.complete(), false);
  stream.push(Buffer.concat([first.subarray(2), make(messages[1])]), 20);
  assert.equal(stream.complete(), true);
  assert.deepEqual(seen, messages.map(message => ({message, time: 20})));
  assert.throws(() => stream.push(Buffer.alloc(4), 30), /invalid observation frame length/);
});
