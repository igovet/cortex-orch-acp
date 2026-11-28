// Test-only observation of desktop IPC and of actual X11 root pixels.
import { createServer, createConnection } from 'node:net';
import { spawn } from 'node:child_process';
import { Transform } from 'node:stream';
import { createHash } from 'node:crypto';
import { chmodSync, existsSync, readFileSync, renameSync, statSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';

const MARKER = 'Native fixture progress.';
const PEER_MARKER = 'Post-question coordinator text';
const MAX_FRAME = 8 * 1024 * 1024;
const SECOND = 1_000_000_000;
const now = () => Number(process.hrtime.bigint());

export function classify({method, params} = {}) {
  const operation = method === 'workspace' || method === 'native.query' ? params?.operation : method;
  if (operation === 'history.page') return 'history';
  if (['tasks.get', 'executions.get', 'decisions.get', 'decisions.list', 'results.get'].includes(operation)
      || method === 'machine.call' && params?.op === 'product.facts') return 'detail';
  // The desktop real port negotiates capabilities on each machine-wide coherent
  // read, not on project-scoped reads. Do not count reconnect's projects.list.
  if (method === 'service.capabilities') return 'fullRefresh';
  return undefined;
}

export function isSubscription(method) {
  return method === 'service.subscribe' || method === 'service.subscribe.code';
}

export function markerIn(value) {
  if (!value || typeof value !== 'object') return false;
  if (typeof value.text === 'string' && value.text.startsWith(MARKER)) return true;
  return Object.values(value).some(markerIn);
}

export function exactHistoryRow(page) {
  const rows = page?.items?.filter(row => row.kind === 'assistant'
    && (row.text === PEER_MARKER || row.text === '\n\n' + PEER_MARKER));
  if (rows?.length > 1) throw new Error('ambiguous fixture history fragment');
  return rows?.[0];
}

const FOLLOW = 'native.history.follow';

export function capabilityFrame(message, request, arm) {
  const methods = message.result?.methods;
  if (message.jsonrpc !== '2.0' || message.id !== request.id || !Array.isArray(methods)
      || methods.filter(x => x === FOLLOW).length !== 1 || !['pull','push'].includes(arm)) {
    throw new Error('matched comparison requires one real history-follow capability');
  }
  if (arm === 'pull') message.result.methods = methods.filter(x => x !== FOLLOW);
  const body = Buffer.from(JSON.stringify(message));
  const prefix = Buffer.alloc(4);
  prefix.writeUInt32BE(body.length);
  return Buffer.concat([prefix,body]);
}

export function followFrame(request, event, state) {
  if (request?.method !== FOLLOW || !state.ack) throw new Error('private history on an unacknowledged or unrelated socket');
  for (const key of ['projectId', 'taskId', 'executionId']) {
    if (typeof event[key] !== 'string' || !event[key] || event[key] !== request.params?.[key]) {
      throw new Error('private history scope mismatch: ' + key);
    }
  }
  if (event.type === 'native_history_resync') return;
  if (event.type !== 'native_history_page' || !Array.isArray(event.page?.items)
      || typeof event.generation !== 'string' || !event.generation
      || typeof event.throughSequence !== 'string' || !/^(0|[1-9]\d{0,19})$/.test(event.throughSequence)
      || (state.through != null && BigInt(event.throughSequence) <= state.through)
      || event.fromCursor !== state.cursor
      || (state.generation != null && state.generation !== event.generation)) {
    throw new Error('incomplete or discontinuous private history frame');
  }
  state.generation = event.generation;
  state.through = BigInt(event.throughSequence);
  state.cursor = event.page.tailCursor ?? null;
  return exactHistoryRow(event.page);
}

export function parsePeerTrace(text) {
  const match = /^([1-9]\d{0,9}) (fixture-[1-9]\d{0,7}) (\d{1,16}) (\d{1,16})\n$/.exec(text);
  if (!match) throw new Error('missing, duplicate or malformed peer emission record');
  const [, pid, sessionId, first, last] = match;
  const [begin, end] = [Number(first), Number(last)];
  if (![begin, end].every(Number.isSafeInteger) || begin <= 0 || end < begin) {
    throw new Error('invalid peer emission clock');
  }
  return {pid: Number(pid), sessionId, begin, end};
}

export function bindPeer(emission, bindings, scope, start, history) {
  const [binding] = Array.isArray(bindings) ? bindings : [];
  if (bindings?.length !== 1 || !binding) {
    throw new Error('peer emission lacks one matching durable fixture binding; rows=' + (bindings?.length ?? 'invalid'));
  }
  for (const [field, matches] of [
    ['task', binding.taskId === scope.taskId], ['project', binding.projectId === scope.projectId],
    ['execution', scope.delivery !== 'native_history_page' || binding.executionId === scope.executionId],
    ['session', binding.sessionId === emission.sessionId],
    ['runtime task', /^t_[a-f0-9]{32}$/.test(binding.runtimeTaskId)],
    ['attempt', /^xa_[a-f0-9]{32}$/.test(binding.attemptId)],
    ['display session', typeof binding.displaySessionId === 'string' && binding.displaySessionId],
    ['source sequence', Number.isSafeInteger(binding.sourceSequence) && binding.sourceSequence >= 0],
  ]) {
    if (!matches) throw new Error('peer emission lacks matching fixture ' + field);
  }
  const rowId = 'wh_' + createHash('sha256').update(JSON.stringify(
    [binding.displaySessionId, binding.sourceSequence])).digest('hex').slice(0, 32);
  if (scope.historyId !== rowId || scope.historySequence !== String(binding.sourceSequence + 1)) {
    throw new Error('peer fragment is not the same durable history row');
  }
  if (!(start < emission.begin && emission.end <= history)) {
    throw new Error('peer emission outside the measured clock window');
  }
  return binding;
}

export function rateSummary(events, start, end) {
  const size = Math.floor((end - start) / SECOND);
  if (size < 1 || size > 120) throw new Error('incomplete request observation window');
  const totals = { history: 0, detail: 0, fullRefresh: 0 };
  const windows = Array.from({length: size}, () => ({...totals}));
  for (const {time, kind} of events) {
    if (time < start || time >= end) continue;
    totals[kind]++;
    const bin = windows[Math.floor((time - start) / SECOND)];
    if (bin) bin[kind]++;
  }
  return {durationNs: end - start, totals, windows};
}

export function sqliteWindow(records, start, end) {
  if (!records.length) return {coverage:'uninstrumented'};
  if (records.length % 16 || records.length > 64000000) throw new Error('truncated or overflowing SQLite trace');
  const labels = ['invalid', 'start', 'opens', 'otherStatements', 'readonlyResultStatements', 'resultRows', 'triggerSteps', 'closes'];
  const totals = Object.fromEntries(labels.slice(2).map(label => [label,0]));
  let starts = 0, openBeforeEnd = 0;
  for (let at = 0; at < records.length; at += 16) {
    const time = Number(records.readBigUInt64LE(at)), kind = Number(records.readBigUInt64LE(at+8));
    if (!Number.isSafeInteger(time) || time <= 0 || !labels[kind] || !kind) throw new Error('invalid SQLite trace');
    if (kind === 1) {
      if (at || time >= start) throw new Error('SQLite trace started late or duplicated');
      starts++;
    } else if (kind === 2 && time < end) openBeforeEnd++;
    if (time >= start && time < end) totals[labels[kind]]++;
  }
  if (starts !== 1 || !openBeforeEnd) throw new Error('SQLite connection observation unavailable');
  return {coverage:'service process, all rusqlite connections and threads', ...totals,
    statements:totals.otherStatements + totals.readonlyResultStatements};
}

export function correlate(arrival, frames, arrivalEnd = arrival) {
  if (!Number.isSafeInteger(arrival)) throw new Error('missing text-bearing history reply');
  let previous, seenBefore = false;
  for (const frame of frames) {
    if (frame.begin > frame.end || previous && previous.end > frame.begin) {
      throw new Error('unordered frame capture');
    }
    if (frame.visible) {
      if (!previous || frame.begin < arrivalEnd) throw new Error('unbracketed first visible frame');
      if (!seenBefore) throw new Error('no pre-arrival negative frame');
      return {
        latencyUpperNs: frame.end - arrival,
        latencyLowerNs: Math.max(0, previous.begin - arrivalEnd),
        frameGapNs: frame.end - previous.begin,
      };
    }
    seenBefore ||= frame.end < arrival;
    previous = frame;
  }
  throw new Error('no captured frame contains the fragment');
}

// Forward bytes unchanged, and independently decode complete frames. The
// observation timestamp is the arrival of the last byte in this read callback.
export function decoder(callback) {
  let pending = Buffer.alloc(0);
  return {
    push(chunk, time) {
      pending = pending.length ? Buffer.concat([pending, chunk]) : chunk;
      while (pending.length >= 4) {
        const len = pending.readUInt32BE(0);
        if (len < 1 || len > MAX_FRAME) throw new Error('invalid observation frame length');
        if (pending.length < len + 4) break;
        let message;
        try { message = JSON.parse(pending.toString('utf8', 4, len + 4)); }
        catch { throw new Error('invalid observation frame JSON'); }
        callback(message, time, len + 4);
        pending = pending.subarray(len + 4);
      }
    },
    complete() { return pending.length === 0; },
  };
}

function ocr(header, image, peerTiming) {
  return new Promise((resolve, reject) => {
    const cmd = spawn('tesseract', ['stdin', 'stdout', '--psm', '11', '-l', 'eng'], {
      stdio: ['pipe', 'pipe', 'ignore'],
      env: {...process.env, OMP_THREAD_LIMIT: '1'},
    });
    let output = '';
    cmd.on('error', reject);
    cmd.stdin.on('error', reject);
    cmd.stdout.on('data', data => { output += data; });
    const deadline = setTimeout(() => cmd.kill('SIGKILL'), 5000);
    cmd.on('close', code => {
      clearTimeout(deadline);
      if (code !== 0) reject(new Error('pixel OCR failed'));
      else resolve((peerTiming ? /\bpost[- ]question coordinator text\b/i : /\bnative fixture progress\b/i)
        .test(output.replace(/\s+/g, ' ')));
    });
    cmd.stdin.end(Buffer.concat([header, image]));
  });
}

function capture(window, arrival, ready, fail, peerTiming) {
  const spawnTime = now();
  const helper = peerTiming ? '/harness/baseline-frame' : '/opt/cortex-render/baseline-frame';
  const proc = spawn(helper, [window], {stdio: ['ignore', 'pipe', 'ignore']});
  let pending = Buffer.alloc(0), header, length, size, complete = false, resolving;
  const frames = [];
  let count = 0, first, last, maxGap = 0, maxCapture = 0, maxReadLag = 0;
  proc.on('error', () => fail('frame helper did not start'));
  proc.stdout.on('data', chunk => {
    if (complete) return;
    const received = now();
    pending = pending.length ? Buffer.concat([pending, chunk]) : chunk;
    if (!header) {
      const split = pending.indexOf(10);
      if (split < 0 && pending.length < 24) return;
      const match = /^(\d+) (\d+)$/.exec(pending.toString('ascii', 0, split));
      if (!match || +match[1] > 1920 || +match[2] > 1200 || +match[1] * +match[2] < 1) {
        return fail('invalid frame geometry');
      }
      size = [+match[1], +match[2]];
      length = size[0] * size[1];
      header = Buffer.from('P5\n' + size.join(' ') + '\n255\n');
      pending = pending.subarray(split + 1);
    }
    while (pending.length >= 16 + length) {
      // The helper and Node are Linux processes in the same container/time
      // namespace. Check clock ordering using the parent spawn/pipe brackets.
      const begin = Number(pending.readBigUInt64LE(0));
      const end = Number(pending.readBigUInt64LE(8));
      if (!Number.isSafeInteger(end) || begin < spawnTime || begin < (last?.end ?? 0)
          || end < begin || end > received) return fail('capture clock alignment failed');
      if (!arrival()) {
        if (frames.length === 12) frames.shift();
      }
      if ((!arrival() || begin <= arrival() + 2 * SECOND) && frames.length < 512) {
        const image = pending.subarray(16, 16 + length);
        const previous = frames.at(-1)?.image;
        frames.push({begin, end, image: previous?.equals(image) ? previous : Buffer.from(image)});
      } else if (arrival() && begin <= arrival() + 2 * SECOND) {
        return fail('frame retention overflow');
      }
      first ??= begin;
      maxGap = Math.max(maxGap, last ? begin - last.begin : 0);
      maxCapture = Math.max(maxCapture, end - begin);
      maxReadLag = Math.max(maxReadLag, received - end);
      last = {begin, end};
      pending = pending.subarray(16 + length);
      if (++count === 1) ready();
    }
  });
  const done = new Promise(resolve => { resolving = resolve; });
  proc.on('close', () => {
    if (!complete) fail('frame capture ended early');
    resolving();
  });
  return {
    async stop() { complete = true; proc.kill(); await done; },
    async analyze(time, timeEnd = time) {
      const matches = [];
      let previous;
      for (const frame of frames) {
        const visible = previous?.image.equals(frame.image) ? previous.visible : await ocr(header, frame.image, peerTiming);
        previous = {image: frame.image, visible};
        matches.push({begin: frame.begin, end: frame.end, visible});
        if (visible) break;
      }
      const firstVisible = matches.at(-1);
      const previousNegative = matches.at(-2);
      return {...correlate(time, matches, timeEnd), ...(peerTiming ? {firstVisible, previousNegative} : {}),
        frameCount: count, frameWidth: size[0], frameHeight: size[1],
        firstFrameBeginNs: first, maxGapNs: maxGap, maxCaptureNs: maxCapture, maxReadLagNs: maxReadLag};
    },
  };
}

async function observe(dir, backend, front, peerTrace, sqliteTrace, arm) {
  const readTrace = () => {
    const size = statSync(peerTrace).size;
    if (!size) return undefined;
    if (size > 128) throw new Error('peer trace overflow or duplicate emission');
    return parsePeerTrace(readFileSync(peerTrace, 'ascii'));
  };
  let liveEmission, firstPush, firstPull;
  const publish = (name, value) => {
    writeFileSync(join(dir, name + '.tmp'), JSON.stringify(value), {mode: 0o600});
    renameSync(join(dir, name + '.tmp'), join(dir, name));
  };
  const fail = reason => {
    if (!failure) { failure = reason; publish('failure', incomplete(reason)); }
  };
  let failure, start, end, sampler, arrival, scope, expectedTask;
  const warmupStart = now();
  let subscriptions = 0, warmupRefreshes = 0, activeRequests = 0, connections = 0;
  const events = [], clients = new Set(), warmupMethods = new Set();
  const requests = new Map();
  const stream = {opens:0, acknowledgements:0, closes:0, resyncs:0, pages:0, rows:0, bytes:0, decisionPages:0, markerPushes:0, markerPulls:0,
    projectionCountersThroughLastFrame:null, resyncReasons:{}};
  const incomplete = reason => reason + '; incomplete wire observation ' +
    JSON.stringify({arm:arm ?? 'transparent',activeRequests,stream});
  let capabilitiesCompared = 0;
  let subscribeRequests = 0, subscribeReplies = 0, subscribeClosed = 0;
  const server = createServer(client => {
    if (clients.size >= 256) { fail('too many observed connections'); client.destroy(); return; }
    connections++;
    const service = createConnection(backend);
    clients.add(client);
    let request, subscribed = false, req, res;
    const follow = {ack:false, cursor:null, generation:null, terminal:false};
    req = decoder((message, time) => {
      if (request || message.jsonrpc !== '2.0' || typeof message.method !== 'string' || !message.id) {
        throw new Error('unsupported request envelope');
      }
      request = message;
      if (request.method === FOLLOW) {
        follow.cursor = request.params?.cursor ?? null;
        follow.generation = request.params?.generation ?? null;
      }
      if (start === undefined && /^service\.[A-Za-z.]+$/.test(message.method)) warmupMethods.add(message.method);
      if (isSubscription(message.method)) subscribeRequests++;
      const kind = classify(message);
      if (start === undefined) {
        if (kind === 'fullRefresh') warmupRefreshes++;
      } else if (end === undefined) {
        if (++activeRequests > 8192) throw new Error('request observation overflow');
        const method = ['workspace','native.query','machine.call'].includes(message.method)
          ? message.method + '/' + (message.params?.operation ?? message.params?.op) : message.method;
        if (!/^[a-z][a-z0-9._/]{0,99}$/.test(method)) throw new Error('unrecognized RPC method name');
        requests.set(method, (requests.get(method) ?? 0) + 1);
        if (request.method === FOLLOW) stream.opens++;
        if (kind) events.push({kind, time});
      }
    });
    res = decoder((message, time, bytes) => {
      const active = start !== undefined && end === undefined;
      if (request?.method === FOLLOW && message.id === request.id) {
        if (follow.ack || message.result?.subscribed !== true) throw new Error('private history subscription refused or replayed');
        follow.ack = true;
        if (active) stream.acknowledgements++;
      }
      const event = message.method === 'service.event' ? message.params : undefined;
      if (peerTrace && event && isSubscription(request?.method) && JSON.stringify(event).includes(PEER_MARKER)) {
        throw new Error('private fixture transcript appeared on the general notification socket');
      }
      if (event?.type === 'native_history_page' || event?.type === 'native_history_resync') {
        if (follow.terminal) throw new Error('private event after terminal resync');
        const row = followFrame(request, event, follow);
        follow.terminal = event.type === 'native_history_resync';
        if (row && start === undefined) throw new Error('private marker arrived before measurement');
        if (event.observation) {
          for (const key of ['projectionReads','decisionReads','emptyReads']) {
            const n = event.observation[key], previous = follow.observation?.[key] ?? 0;
            if (!Number.isSafeInteger(n) || n < previous) throw new Error('invalid private projection counter');
            if (active) {
              stream.projectionCountersThroughLastFrame ??= {projectionReads:0,decisionReads:0,emptyReads:0};
              stream.projectionCountersThroughLastFrame[key] += n - previous;
            }
          }
          follow.observation = event.observation;
        }
        if (active) {
          stream.bytes += bytes;
          if (follow.terminal) {
            stream.resyncs++;
            const reason = event.reason;
            if (typeof reason !== 'string' || !/^[a-z_]{1,64}$/.test(reason)) throw new Error('unrecognized stream resync reason');
            stream.resyncReasons[reason] = (stream.resyncReasons[reason] ?? 0) + 1;
          }
          else {
            stream.pages++;
            stream.rows += event.page.items.length;
            if (event.decisions != null) stream.decisionPages++;
          }
          if (row) {
            if (event.taskId !== expectedTask || ++stream.markerPushes > 1) throw new Error('unexpected or replayed pushed marker');
            firstPush ??= time;
            if (!arrival) {
              scope = {delivery:'native_history_page',projectId:event.projectId,taskId:event.taskId,executionId:event.executionId,
                generation:event.generation,throughSequence:event.throughSequence,historyId:row.historyId,historySequence:row.sequence,rpcId:request.id};
              arrival = time;
            }
          }
        }
      }
      if (isSubscription(request?.method) && message.id === request.id) {
        subscribeReplies++;
        if (message.result?.subscribed) {
          subscribed = true;
          subscriptions++;
        }
      }
      if (end === undefined && request && classify(request) === 'history'
          && message.id === request.id && (peerTrace || markerIn(message.result))) {
        const page = request.method === 'native.query'
          ? (message.result?.projectId === request.params.projectId && message.result?.data?.type === 'history'
            ? message.result.data.value : undefined)
          : message.result;
        const row = peerTrace && exactHistoryRow(page);
        if (peerTrace && !row) return;
        if (start === undefined) throw new Error('text marker already arrived before measurement');
        const p = request.params;
        const taskId = p.payload?.target ?? p.target;
        if (active && row && taskId !== expectedTask) throw new Error('private fixture marker in another task history');
        if (active && row && taskId === expectedTask) { stream.markerPulls++; firstPull ??= time; }
        if (taskId === expectedTask && !arrival) {
          scope = {delivery:'history.page',projectId: p.projectId, taskId, rpcId: request.id};
          if (!scope.projectId) throw new Error('missing history correlation identity');
          if (row) {
            if (!row.historyId || !row.sequence) throw new Error('missing fixture history row identity');
            scope.historyId = row.historyId;
            scope.historySequence = row.sequence;
          }
          arrival = time;
        }
      }
    });
    // Pipe supplies backpressure and preserves half-close and payloads. The
    // extra listeners observe exactly those chunks, not a second connection.
    for (const [socket, decode] of [[client, req], [service, res]]) {
      socket.on('data', chunk => {
        const time = now();
        try { decode.push(chunk, time); } catch (error) { fail(error.message); }
      });
      socket.on('end', () => { if (!decode.complete()) fail('partial IPC frame'); });
      socket.on('error', error => {
        if (end === undefined) fail('observed IPC socket failure: ' + (socket === service ? 'backend ' : 'desktop ') + error.code);
        client.destroy();
        service.destroy();
      });
    }
    client.on('close', () => {
      clients.delete(client);
      if (follow.ack && start !== undefined && end === undefined) stream.closes++;
      if (subscribed) {
        subscribeClosed++;
        subscriptions--;
        if (start !== undefined && end === undefined) fail('subscription lost during window');
      }
      service.destroy();
    });
    service.on('close', () => client.end());
    client.pipe(service);
    if (arm) {
      // Only the capability response changes in the control. Both arms use the
      // same buffering and framing; all subscription bytes are forwarded intact.
      const capability = decoder(message => {
        gate.push(capabilityFrame(message, request, arm));
        capabilitiesCompared++;
      });
      const gate = new Transform({
        transform(chunk, encoding, next) {
          try {
            if (request?.method === 'service.capabilities') { capability.push(chunk, 0); next(); }
            else next(null,chunk);
          } catch (error) { next(error); }
        },
        flush(next) { next(capability.complete() ? undefined : new Error('partial capability frame')); },
      });
      gate.on('error', error => { fail(error.message); client.destroy(); service.destroy(); });
      service.pipe(gate).pipe(client);
    } else service.pipe(client);
  });
  server.on('error', () => fail('observer socket failed'));
  server.listen(front, () => {
    try {
      // The real desktop connector refuses sockets with group/other access.
      chmodSync(front, 0o600);
      publish('ready', true);
    } catch { fail('observer could not make its socket private'); }
  });
  try {
    while (!existsSync(join(dir, 'finish')) && !failure) {
      if (!sampler && existsSync(join(dir, 'begin'))) {
        if (subscriptions < 1 || warmupRefreshes < 1) {
          throw new Error('missing desktop IPC warmup: subscriptions=' + subscriptions
            + ', refreshes=' + warmupRefreshes + ', connections=' + connections
            + ', subscribe sent/reply/closed=' + [subscribeRequests, subscribeReplies, subscribeClosed].join('/')
            + ', methods=' + [...warmupMethods].sort().join(','));
        }
        const {window, taskId} = JSON.parse(readFileSync(join(dir, 'begin'), 'utf8'));
        if (!/^\d+$/.test(window) || !/^wt_[0-9a-f]{32}$/.test(taskId)) throw new Error('missing owned window or task identity');
        expectedTask = taskId;
        sampler = capture(window, () => {
          if (!peerTrace) return arrival;
          try { liveEmission ??= readTrace(); }
          catch (error) { fail(error.message); }
          return liveEmission?.end;
        }, () => {
          start = now();
          publish('started', true);
        }, fail, Boolean(peerTrace));
      }
      await new Promise(resolve => setTimeout(resolve, 5));
    }
    end = now();
    await sampler?.stop();
    if (failure) throw new Error(failure);
    if (!sampler || !start || subscriptions < 1 || activeRequests === 0 || !arrival) {
      throw new Error('incomplete observation: subscriptions=' + subscriptions + ', requests='
        + activeRequests + ', history=' + events.filter(event => event.kind === 'history').length
        + ', fragment=' + Boolean(arrival));
    }
    const emission = peerTrace && readTrace();
    if (peerTrace && !emission) throw new Error('no complete fixture ACP emission was observed');
    const peerTiming = emission ? {
      emission,
      fragment: PEER_MARKER,
      binding: bindPeer(emission, JSON.parse(readFileSync(join(dir, 'finish'), 'utf8')), scope, start, arrival),
      historyArrivalNs: arrival,
      isolation: 'loopback-only network, masked checkout stores, authenticated private Xvfb',
    } : undefined;
    const summary = emission
      ? await sampler.analyze(emission.begin, emission.end)
      : await sampler.analyze(arrival);
    if (emission) {
      const push = firstPush != null && firstPush <= summary.firstVisible.end;
      const pull = firstPull != null && firstPull <= summary.firstVisible.end;
      stream.firstPixelSource = push && pull ? 'ambiguous_push_and_pull'
        : push && firstPush <= summary.firstVisible.begin ? 'private_push'
        : pull && firstPull <= summary.firstVisible.begin ? 'history_reply'
        : push || pull ? 'overlapping_frame_capture' : 'unobserved';
      stream.firstMarkerPushNs = firstPush ?? null;
      stream.firstMarkerPullNs = firstPull ?? null;
    }
    if (arm && (!capabilitiesCompared || stream.firstPixelSource !== (arm === 'push' ? 'private_push' : 'history_reply')
        || (arm === 'pull' && stream.opens !== 0) || stream.resyncs)) {
      throw new Error('matched delivery failed: arm=' + arm + ', source=' + stream.firstPixelSource
        + ', opens=' + stream.opens + ', resyncs=' + stream.resyncs);
    }
    publish('result', {...summary, ...rateSummary(events, start, end), scope, peerTiming,
      deliveryComparison:{arm:arm ?? 'transparent',capabilitiesCompared},
      stream, requestsByMethod:Object.fromEntries(requests),
      sqlite:sqliteTrace ? sqliteWindow(readFileSync(sqliteTrace), start, end) : {coverage:'uninstrumented'},
      arrivalNsFromWindowStart: arrival - start, warmupNs: start - warmupStart,
      warmupRefreshes, subscriptions, activeRequests, connections});
    // Rust stops this observer only after it closes the measured desktop window.
    await new Promise(resolve => process.once('SIGTERM', resolve));
  } catch (error) {
    // Keep the measured prefix even when a timeout, refusal, wrong source or
    // incomplete page disqualifies this trial. It is never a passing sample.
    throw new Error(incomplete(error.message));
  } finally {
    await sampler?.stop();
    for (const connection of clients) connection.destroy();
    server.close();
  }
}

if (process.argv[1] && pathToFileURL(process.argv[1]).href === import.meta.url) {
  const [dir, backend, front, peerTrace, sqliteTrace, arm] = process.argv.slice(2);
  observe(dir, backend, front, peerTrace, sqliteTrace, arm).catch(error => {
    writeFileSync(join(dir, 'failure'), JSON.stringify(error.message), {mode: 0o600});
    process.exitCode = 1;
  });
}
